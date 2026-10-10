// UI fixture. Measurements come from native generated frames and PCM.
(function () {
  'use strict';
  var running = false;

  function assert(value, message) {
    if (!value) throw new Error(message);
  }
  function text(id, value) {
    var element = document.getElementById(id);
    element.textContent = value;
    element.setAttribute('data-result', value);
  }
  function tick() {
    return new Promise(function (resolve) { setTimeout(resolve, 10); });
  }
  async function reject(promise, name, constraint) {
    try {
      await promise;
    } catch (error) {
      assert(error.name === name, 'expected ' + name + ', got ' + error.name);
      if (constraint !== undefined) {
        assert(error.constraint === constraint, 'wrong constraint: ' + error.constraint);
      }
      return;
    }
    throw new Error('expected rejection: ' + name);
  }

  document.getElementById('run').onclick = async function () {
    if (running) return;
    running = true;
    var button = document.getElementById('run');
    button.disabled = true;
    var cleanup = [];
    var md = navigator.mediaDevices;
    var fake = globalThis.__chuzzFakeMedia;
    var deviceListener;
    try {
      assert(globalThis.isSecureContext && md, 'fixture requires a trusted local origin');
      assert(new MediaStream().active === false, 'empty stream active');
      var before = await md.enumerateDevices();
      assert(before.length === 3, 'expected camera, microphone and output');
      assert(before.every(function (device) { return device.label === ''; }), 'premature labels');
      assert(before.every(function (device) { return device.groupId === ''; }), 'premature groups');
      var camera = before.find(function (device) { return device.kind === 'videoinput'; });
      var microphone = before.find(function (device) { return device.kind === 'audioinput'; });
      assert(camera instanceof InputDeviceInfo, 'camera InputDeviceInfo');
      assert(Object.keys(camera.getCapabilities()).length === 0, 'premature capabilities');
      await reject(md.getUserMedia({}), 'TypeError');
      await reject(md.getUserMedia({ audio: false, video: false }), 'TypeError');
      await reject(md.getUserMedia({
        audio: true,
        video: { deviceId: { exact: 'missing-device' } }
      }), 'OverconstrainedError', 'deviceId');
      var failed = await md.enumerateDevices();
      assert(failed.every(function (device) { return device.label === ''; }), 'partial acquire granted access');

      var stream = await md.getUserMedia({
        video: {
          deviceId: { exact: [camera.deviceId] },
          width: { exact: 64 }, height: { exact: 48 }, frameRate: { exact: 10 }
        },
        audio: {
          deviceId: { exact: microphone.deviceId },
          sampleRate: { exact: 48000 }, channelCount: { exact: 1 },
          echoCancellation: { ideal: true }
        }
      });
      cleanup.push(stream);
      var video = stream.getVideoTracks()[0];
      var audio = stream.getAudioTracks()[0];
      assert(stream instanceof MediaStream && video instanceof MediaStreamTrack, 'constructors');
      assert(stream.active && stream.getTracks().length === 2, 'initial stream');
      assert(stream.getTrackById(video.id) === video, 'track identity');
      assert(stream.getTrackById('missing') === null, 'missing track');
      var copiedStream = new MediaStream(stream);
      assert(copiedStream.id !== stream.id && copiedStream.getVideoTracks()[0] === video, 'stream copy');
      copiedStream.addTrack(video);
      assert(copiedStream.getTracks().length === 2, 'duplicate addTrack');
      copiedStream.removeTrack(video);
      assert(copiedStream.getVideoTracks().length === 0 && video.readyState === 'live', 'remove stopped capture');
      copiedStream.addTrack(video);

      var after = await md.enumerateDevices();
      assert(after.every(function (device) { return device.label.length > 0; }), 'granted labels');
      assert(after.map(function (device) { return device.deviceId; }).join() ===
        before.map(function (device) { return device.deviceId; }).join(), 'unstable device ids');
      assert(after[0].toJSON().kind === after[0].kind, 'device JSON');
      var capabilities = video.getCapabilities();
      assert(capabilities.width.max === 320 && capabilities.frameRate.max === 30, 'video capabilities');
      capabilities.width.max = 0;
      assert(video.getCapabilities().width.max === 320, 'capabilities alias');
      var supported = md.getSupportedConstraints();
      assert(supported.width && supported.channelCount && !supported.torch, 'supported constraints');
      assert(audio.getSettings().echoCancellation === false, 'unsupported ideal processing enabled');
      text('devices', 'PASS: stable fake devices and permission labels');

      var frame0 = fake.probe(video, 0);
      var frame1 = fake.probe(video, 100000);
      var again = fake.probe(video, 0);
      assert(frame0.checksum === again.checksum, 'nondeterministic camera');
      assert(frame0.checksum !== frame1.checksum, 'camera does not move');
      assert(frame1.timestamp > frame0.timestamp && frame0.current, 'frame clock or generation');
      assert(frame0.width === 64 && frame0.height === 48 && !frame0.black, 'frame dimensions');
      video.enabled = false;
      assert(fake.probe(video, 0).black && fake.probe(video, 100000).black, 'disabled video not black');
      assert(!video.muted && stream.active, 'disabled track became muted or inactive');
      video.enabled = true;
      assert(fake.probe(video, 0).checksum === frame0.checksum, 'enable reset source');
      text('frames', 'PASS: moving deterministic I420 and disabled black frames');

      var tone = fake.probe(audio, 0);
      var toneAgain = fake.probe(audio, 0);
      assert(tone.sampleCount === 960 && tone.frames === 960 && tone.sampleRate === 48000, 'PCM shape');
      assert(tone.energy === toneAgain.energy && tone.rms > 0.16 && tone.rms < 0.19, 'deterministic PCM energy');
      assert(tone.zeroCrossings >= 17 && tone.zeroCrossings <= 19, '440 Hz tone');
      assert(fake.probe(audio, 20000).timestamp > tone.timestamp, 'PCM clock');
      audio.enabled = false;
      assert(fake.probe(audio, 0).energy === 0 && !audio.muted, 'disabled microphone not silent');
      audio.enabled = true;
      assert(fake.probe(audio, 0).energy === tone.energy, 'enable reset microphone');
      text('pcm', 'PASS: measured 440 Hz PCM and exact disabled silence');

      var clone = video.clone();
      var audioClone = audio.clone();
      cleanup.push(new MediaStream([clone, audioClone]));
      assert(clone.id !== video.id && clone.readyState === 'live', 'clone identity');
      assert(fake.probe(clone, 100000).sourceId === frame1.sourceId, 'clone did not share source');
      assert(fake.probe(clone, 100000).checksum === frame1.checksum, 'clone source timeline');
      clone.enabled = false;
      assert(fake.probe(clone, 0).black && !fake.probe(video, 0).black, 'clone enabled isolation');
      clone.enabled = true;
      await clone.applyConstraints({
        width: { exact: 96 }, height: { exact: 48 }, aspectRatio: { exact: 2 },
        frameRate: { exact: 20 }
      });
      assert(clone.getSettings().width === 96 && clone.getSettings().aspectRatio === 2, 'clone settings');
      assert(video.getSettings().width === 64 && video.getSettings().frameRate === 10, 'clone constraints leaked');
      var previous = JSON.stringify(clone.getSettings());
      var previousConstraints = JSON.stringify(clone.getConstraints());
      await reject(clone.applyConstraints({ width: { exact: 321 } }), 'OverconstrainedError', 'width');
      assert(JSON.stringify(clone.getSettings()) === previous, 'failed apply changed settings');
      assert(JSON.stringify(clone.getConstraints()) === previousConstraints, 'failed apply changed constraints');
      await reject(clone.applyConstraints({
        width: { exact: 64 }, height: { exact: 48 }, aspectRatio: { exact: 2 }
      }), 'OverconstrainedError', 'aspectRatio');
      await reject(audioClone.applyConstraints({ echoCancellation: { exact: true } }),
        'OverconstrainedError', 'echoCancellation');
      await reject(audioClone.applyConstraints({ channelCount: { exact: 1.5 } }),
        'OverconstrainedError', 'channelCount');
      await clone.applyConstraints({
        width: { ideal: 10000 }, height: { ideal: 10000 },
        advanced: [{ width: { exact: 9999 } }, { frameRate: 20 }],
        unknownConstraint: { exact: 'ignored' }
      });
      assert(clone.getSettings().width === 320 && clone.getSettings().height === 240, 'ideal selection');
      assert(clone.getSettings().frameRate === 20, 'advanced selection');
      await clone.applyConstraints();
      assert(clone.getSettings().width === 64 && Object.keys(clone.getConstraints()).length === 0, 'constraint reset');
      await audioClone.applyConstraints({ sampleRate: { exact: 8000 }, channelCount: { exact: 2 } });
      var stereo = fake.probe(audioClone, 0);
      assert(stereo.frames === 160 && stereo.sampleCount === 320 && stereo.channels === 2, 'PCM constraints');
      assert(audio.getSettings().sampleRate === 48000, 'audio clone settings leaked');
      var constraintCopy = video.getConstraints();
      constraintCopy.width.exact = 1;
      assert(video.getConstraints().width.exact === 64, 'constraints alias');
      text('constraints', 'PASS: exact, ideal, advanced and atomic independent settings');

      var muteCount = 0;
      var unmuteCount = 0;
      var onceCount = 0;
      var endedCount = 0;
      var explicitEnded = 0;
      audio.onmute = function () { muteCount++; };
      audio.addEventListener('unmute', function () { unmuteCount++; });
      audio.addEventListener('mute', function () { onceCount++; }, { once: true });
      audio.addEventListener('ended', function () { explicitEnded++; });
      fake.setMuted('audio', true);
      await tick();
      assert(audio.muted && audioClone.muted && muteCount === 1 && onceCount === 1, 'mute events');
      assert(fake.probe(audio, 0).energy === 0, 'muted source not silent');
      fake.setMuted('audio', true);
      await tick();
      assert(muteCount === 1, 'duplicate mute');
      fake.setMuted('audio', false);
      await tick();
      assert(!audio.muted && unmuteCount === 1 && fake.probe(audio, 0).energy > 0, 'unmute');
      fake.setMuted('audio', true);
      await tick();
      assert(muteCount === 2 && onceCount === 1, 'once listener');
      fake.setMuted('audio', false);
      await tick();

      video.onended = function () { explicitEnded++; };
      video.stop();
      video.stop();
      await tick();
      assert(video.readyState === 'ended' && explicitEnded === 0, 'stop event semantics');
      assert(clone.readyState === 'live' && !fake.probe(clone, 0).black, 'stop disposed shared source');
      var endedClone = video.clone();
      assert(endedClone.readyState === 'ended' && endedClone.id !== video.id, 'ended clone');
      await video.applyConstraints({ width: { exact: 9999 } });
      assert(clone.readyState === 'live', 'ended apply affected clone');
      var clonedStream = new MediaStream([clone, audioClone]).clone();
      cleanup.push(clonedStream);
      assert(clonedStream.getVideoTracks()[0].id !== clone.id, 'stream clone track identity');
      clonedStream.getTracks().forEach(function (track) { track.stop(); });
      assert(!clonedStream.active && clone.readyState === 'live', 'stream clone stop isolation');
      for (var index = 0; index < 160; index++) clone.clone().stop();
      text('lifecycle', 'PASS: shared sources, independent clone disposal and released handles');

      var changes = 0;
      deviceListener = function () { changes++; };
      md.addEventListener('devicechange', deviceListener);
      clone.onended = function () { endedCount++; };
      fake.setPresent('video', false);
      await tick();
      assert(clone.readyState === 'ended' && endedCount === 1, 'device removal did not end track');
      assert(changes === 1 && stream.active, 'devicechange or surviving microphone');
      assert((await md.enumerateDevices()).length === 2, 'removed device enumerated');
      await reject(md.getUserMedia({ video: true }), 'NotFoundError');
      fake.setPresent('video', false);
      await tick();
      assert(changes === 1 && endedCount === 1, 'duplicate device events');
      fake.setPresent('video', true);
      await tick();
      assert(changes === 2 && clone.readyState === 'ended', 'device restore revived ended track');
      var replacement = await md.getUserMedia({ video: true });
      cleanup.push(replacement);
      assert(replacement.getVideoTracks()[0].getSettings().deviceId === camera.deviceId, 'restored device id');
      audio.stop();
      audioClone.stop();
      await tick();
      assert(explicitEnded === 0 && !stream.active, 'explicit stop or stream active');
      text('events', 'PASS: mute, unmute, devicechange and source ended events');
      text('result', 'READY: devices PASS');
    } catch (error) {
      text('result', 'FAIL: ' + error.name + ': ' + error.message);
    } finally {
      cleanup.forEach(function (stream) {
        stream.getTracks().forEach(function (track) { track.stop(); });
      });
      if (md && deviceListener) md.removeEventListener('devicechange', deviceListener);
      if (fake) {
        fake.setMuted('audio', false);
        fake.setMuted('video', false);
        fake.setPresent('audio', true);
        fake.setPresent('video', true);
        fake.setPresent('audiooutput', true);
      }
      running = false;
    }
  };
})();

