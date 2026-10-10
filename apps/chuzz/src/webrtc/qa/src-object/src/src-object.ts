(function () {
  var run = document.getElementById('run');
  var result = document.getElementById('result');
  var canvas = document.getElementById('picture');
  var context = canvas.getContext('2d');
  var stream = null;
  var video = null;
  var audio = null;
  var device = null;
  var texture = null;

  function assert(value, message) {
    if (!value) throw new Error(message);
  }
  function wait(milliseconds) {
    return new Promise(function (resolve) { setTimeout(resolve, milliseconds); });
  }
  function text(id, value) {
    document.getElementById(id).textContent = value;
  }
  function checksum(bytes) {
    var hash = 2166136261;
    for (var index = 0; index < bytes.length; index++) {
      hash = Math.imul(hash ^ bytes[index], 16777619) >>> 0;
    }
    return hash;
  }
  function draw() {
    context.clearRect(0, 0, 64, 48);
    context.drawImage(video, 0, 0);
    return context.getImageData(0, 0, 64, 48).data;
  }
  function opaque(bytes) {
    for (var index = 3; index < bytes.length; index += 4) {
      if (bytes[index] !== 255) return false;
    }
    return true;
  }
  function black(bytes) {
    for (var index = 0; index < bytes.length; index += 4) {
      if (bytes[index] || bytes[index + 1] || bytes[index + 2]) return false;
    }
    return opaque(bytes);
  }
  function sink(element) {
    var stats = globalThis.__chuzzMediaElementStats(element);
    assert(stats && stats.sampleRate === 48000 && stats.channels === 2, 'native sink format');
    assert(stats.samples === stats.frames * 2, 'sink sample count');
    assert(Math.abs(stats.consumedSeconds - stats.frames / 48000) < 1e-9, 'sink timing');
    return stats;
  }
  async function texturePixels() {
    var buffer = device.createBuffer({
      size: 256 * 48,
      usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ
    });
    try {
      device.queue.copyExternalImageToTexture(
        { source: video },
        { texture: texture },
        { width: 64, height: 48 }
      );
      var encoder = device.createCommandEncoder();
      encoder.copyTextureToBuffer(
        { texture: texture },
        { buffer: buffer, bytesPerRow: 256, rowsPerImage: 48 },
        { width: 64, height: 48 }
      );
      device.queue.submit([encoder.finish()]);
      await buffer.mapAsync(GPUMapMode.READ);
      var pixels = new Uint8Array(buffer.getMappedRange()).slice();
      buffer.unmap();
      return pixels;
    } finally {
      buffer.destroy();
    }
  }
  function dispose() {
    if (video) {
      video.pause();
      video.srcObject = null;
    }
    if (audio) {
      audio.pause();
      audio.srcObject = null;
    }
    if (stream) stream.getTracks().forEach(function (track) { track.stop(); });
    if (texture) texture.destroy();
    if (device) device.destroy();
  }

  run.addEventListener('click', async function () {
    run.disabled = true;
    try {
      assert(context, '2D canvas unavailable');
      stream = await navigator.mediaDevices.getUserMedia({
        video: { width: { exact: 64 }, height: { exact: 48 }, frameRate: { exact: 10 } },
        audio: { sampleRate: { exact: 8000 }, channelCount: { exact: 1 } }
      });
      video = document.createElement('video');
      audio = document.createElement('audio');
      video.muted = true;
      audio.volume = 0.5;
      video.srcObject = stream;
      audio.srcObject = new MediaStream(stream.getAudioTracks());
      var events = [];
      ['loadedmetadata', 'loadeddata', 'canplay', 'playing'].forEach(function (name) {
        video.addEventListener(name, function () { events.push(name); });
      });
      await Promise.all([video.play(), audio.play()]);
      assert(!video.parentNode && !audio.parentNode, 'media must remain detached');
      assert(video.srcObject === stream && video.currentSrc === '' && audio.currentSrc === '', 'stream selection');
      assert(video.readyState >= 2 && audio.readyState >= 2, 'readiness');
      assert(video.videoWidth === 64 && video.videoHeight === 48, 'native dimensions');
      assert(video.duration === Infinity && audio.duration === Infinity, 'live duration');

      var first = draw();
      var time = video.currentTime;
      var audioTime = audio.currentTime;
      await wait(550);
      var second = draw();
      assert(opaque(first) && opaque(second), 'opaque frame pixels');
      assert(!black(first) && checksum(first) !== checksum(second), 'canvas motion');
      assert(video.currentTime > time && audio.currentTime > audioTime, 'live clocks');
      assert(events.includes('loadedmetadata') && events.includes('loadeddata')
        && events.includes('canplay') && events.includes('playing'), 'readiness events');
      assert(video.getVideoPlaybackQuality().totalVideoFrames >= 2, 'measured video frames');
      text('playback', 'PASS: detached video and audio readiness and advancing clocks');
      text('pixels', 'PASS: changing opaque canvas pixels from native frames');

      var adapter = await navigator.gpu.requestAdapter();
      assert(adapter, 'WebGPU adapter unavailable');
      device = await adapter.requestDevice();
      texture = device.createTexture({
        size: { width: 64, height: 48 },
        format: 'rgba8unorm',
        usage: GPUTextureUsage.COPY_DST | GPUTextureUsage.COPY_SRC | GPUTextureUsage.RENDER_ATTACHMENT
      });
      var uploaded = await texturePixels();
      await wait(400);
      var uploadedLater = await texturePixels();
      assert(opaque(uploaded) && !black(uploaded), 'texture content');
      assert(checksum(uploaded) !== checksum(uploadedLater), 'texture motion');
      var bitmap = await createImageBitmap(video);
      try {
        assert(bitmap.width === 64 && bitmap.height === 48, 'video bitmap dimensions');
        context.drawImage(bitmap, 0, 0);
        assert(!black(context.getImageData(0, 0, 64, 48).data), 'video bitmap content');
      } finally {
        bitmap.close();
      }
      text('texture', 'PASS: changing WebGPU texture pixels from native frames');

      var measured = sink(audio);
      assert(measured.frames > 0 && measured.energy > 0, 'actual audible PCM consumption');
      assert(sink(video).frames > 0 && sink(video).energy === 0, 'muted video consumes PCM');
      audio.muted = true;
      await wait(400);
      var muted = sink(audio);
      await wait(400);
      var mutedLater = sink(audio);
      assert(mutedLater.frames > muted.frames && mutedLater.energy === muted.energy, 'measured mute');
      audio.muted = false;
      await wait(400);
      assert(sink(audio).energy > mutedLater.energy, 'unmuted PCM');
      text('pcm', 'PASS: resampled PCM consumption and measured mute');

      video.pause();
      audio.pause();
      await wait(100);
      var pausedTime = video.currentTime;
      var pausedFrames = sink(audio).frames;
      var pausedPixels = checksum(draw());
      await wait(400);
      assert(video.currentTime === pausedTime && sink(audio).frames === pausedFrames, 'pause freezes playout');
      assert(checksum(draw()) === pausedPixels, 'pause retains frame');
      await Promise.all([video.play(), audio.play()]);
      await wait(400);
      assert(video.currentTime > pausedTime && sink(audio).frames > pausedFrames, 'resume');

      var microphone = stream.getAudioTracks()[0];
      var beforeRemoval = video.currentTime;
      stream.removeTrack(microphone);
      await wait(400);
      assert(video.currentTime > beforeRemoval && video.videoWidth === 64, 'audio removal preserves video');
      stream.addTrack(microphone);
      await wait(400);
      assert(video.currentTime > beforeRemoval, 'audio addition preserves video');

      stream.getVideoTracks()[0].enabled = false;
      await wait(400);
      assert(black(draw()), 'disabled camera renders black');
      stream.getVideoTracks()[0].enabled = true;
      await wait(400);
      assert(!black(draw()), 'enabled camera resumes');

      stream.getTracks().forEach(function (track) { track.stop(); });
      await wait(400);
      assert(video.ended && audio.ended, 'track end reaches media elements');
      video.srcObject = null;
      audio.srcObject = null;
      assert(video.srcObject === null && audio.srcObject === null, 'source release');
      var rejected = false;
      try { context.drawImage(video, 0, 0); }
      catch (error) { rejected = error.name === 'InvalidStateError'; }
      assert(rejected, 'released source cannot provide a frame');
      text('lifecycle', 'PASS: pause, resume, track changes and source disposal');
      result.textContent = 'READY: src-object PASS';
    } catch (error) {
      result.textContent = 'FAIL: src-object ' + String(error);
    } finally {
      dispose();
    }
  });
})();

