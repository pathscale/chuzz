(function (g) {
  'use strict';

  function assert(value, message) {
    if (!value) throw new Error(message);
  }
  function text(id, value) {
    document.getElementById(id).textContent = value;
  }
  function sleep(milliseconds) {
    return new Promise(function (resolve) { setTimeout(resolve, milliseconds); });
  }
  async function until(predicate, message) {
    var deadline = Date.now() + 12000;
    while (!predicate()) {
      if (Date.now() >= deadline) throw new Error(message);
      await sleep(20);
    }
  }
  function pixels(video, canvas) {
    var context = canvas.getContext('2d');
    context.drawImage(video, 0, 0, 64, 48);
    return context.getImageData(0, 0, 64, 48).data;
  }
  function difference(a, b) {
    var sum = 0;
    for (var index = 0; index < a.length; index++) {
      if (index % 4 !== 3) sum += Math.abs(a[index] - b[index]);
    }
    return sum / (a.length / 4 * 3);
  }
  function mean(pixels) {
    var values = [0, 0, 0];
    for (var index = 0; index < pixels.length; index += 4) {
      for (var channel = 0; channel < 3; channel++) values[channel] += pixels[index + channel];
    }
    return values.map(function (value) { return value / (pixels.length / 4); });
  }

  g.__chuzzPeerVideoQA = async function () {
    var a = new RTCPeerConnection();
    var b = new RTCPeerConnection();
    var stream = null;
    var local = document.getElementById('video-local');
    var remote = document.getElementById('video-remote');
    var canvas = document.getElementById('video-canvas');
    var reference = document.createElement('canvas');
    reference.width = 64;
    reference.height = 48;
    try {
      stream = await navigator.mediaDevices.getUserMedia({
        video: {
          width: { exact: 64 }, height: { exact: 48 }, frameRate: { exact: 10 }
        }
      });
      var camera = stream.getVideoTracks()[0];
      var sender = a.addTrack(camera, stream);
      var receiving = b.addTransceiver('video', { direction: 'recvonly' });
      var track = receiving.receiver.track;
      var clone = track.clone();
      var trackEvents = [];
      b.ontrack = function (event) { trackEvents.push(event); };
      var parameters = sender.getParameters();
      parameters.encodings[0].maxBitrate = 1000000;
      parameters.encodings[0].maxFramerate = 10;
      await sender.setParameters(parameters);

      var offer = await a.createOffer();
      assert(/m=video /i.test(offer.sdp) && /H264\/90000/i.test(offer.sdp), 'H.264 offer missing');
      assert(/packetization-mode=1/i.test(offer.sdp)
        && /profile-level-id=42e01f/i.test(offer.sdp), 'H.264 format does not match SPS');
      assert(!/VP8|VP9|AV1|m=application/i.test(offer.sdp), 'unsupported video codec advertised');
      await a.setLocalDescription(offer);
      await b.setRemoteDescription(offer);
      var answer = await b.createAnswer();
      assert(/H264\/90000/i.test(answer.sdp)
        && /profile-level-id=42e01f/i.test(answer.sdp), 'H.264 answer missing');
      await b.setLocalDescription(answer);
      await a.setRemoteDescription(answer);
      await until(function () {
        return trackEvents.length === 1 && __chuzzWebrtcInspect(b).framesDecoded >= 3;
      }, 'remote H.264 frames did not decode');
      assert(trackEvents[0].track === track && track.kind === 'video', 'remote video track identity');
      assert(trackEvents[0].transceiver === receiving, 'remote video transceiver identity');
      assert(trackEvents[0].streams.length === 1
        && trackEvents[0].streams[0].id === stream.id, 'remote video MSID');
      assert(sender.getParameters().codecs[0].mimeType === 'video/H264', 'sender codec parameters');
      assert(receiving.receiver.getParameters().codecs[0].mimeType === 'video/H264',
        'receiver codec parameters');
      text('video-sdp', 'PASS: H.264 mode 1, matching SPS profile and stable remote video track');

      local.srcObject = stream;
      remote.srcObject = new MediaStream([track]);
      await local.play();
      await remote.play();
      await until(function () {
        return local.readyState >= 2 && remote.readyState >= 2
          && remote.videoWidth === 64 && remote.videoHeight === 48;
      }, 'srcObject video did not become drawable');
      var first = new Uint8ClampedArray(pixels(remote, canvas));
      var senderColors = mean(pixels(local, reference));
      var receiverColors = mean(first);
      assert(receiverColors.some(function (value) { return value > 30; }), 'remote canvas is black');
      assert(senderColors.every(function (value, index) {
        return Math.abs(value - receiverColors[index]) < 24;
      }), 'remote canvas colors differ from the fake camera');
      var decodedBefore = __chuzzWebrtcInspect(b).framesDecoded;
      await sleep(650);
      var second = new Uint8ClampedArray(pixels(remote, canvas));
      assert(difference(first, second) > 1, 'remote moving pattern did not change canvas pixels');
      assert(__chuzzWebrtcInspect(b).framesDecoded > decodedBefore, 'framesDecoded did not advance');
      var transmitted = __chuzzWebrtcInspect(a);
      assert(transmitted.framesEncoded >= 3
        && transmitted.keyframesSent === transmitted.framesEncoded, 'intra encoder counters');
      assert(transmitted.videoPacketsSent > transmitted.framesEncoded * 3,
        'video did not exercise fragmented RTP');
      assert(transmitted.videoMaxDatagramBytes <= 1500, 'RTP exceeded the transport MTU budget');
      assert(transmitted.videoProfile === '42e01f', 'encoded SPS profile measurement');
      text('video-motion', 'PASS: decoded remote camera paints changing pixels with matching colors');

      var lossBefore = __chuzzWebrtcInspect(b);
      await __chuzzWebrtcDropVideo(b);
      await until(function () {
        var report = __chuzzWebrtcInspect(b);
        return report.videoDropped > lossBefore.videoDropped
          && report.videoRecoveries > lossBefore.videoRecoveries
          && report.framesDecoded > lossBefore.framesDecoded
          && report.keyframesDecoded > lossBefore.keyframesDecoded
          && report.lastVideoTimestamp !== report.videoLostTimestamp;
      }, 'forced video RTP loss did not recover on a newer IDR');
      var recovered = new Uint8ClampedArray(pixels(remote, canvas));
      await sleep(350);
      assert(difference(recovered, pixels(remote, canvas)) > 0.5,
        'remote canvas stopped after video loss');
      text('video-loss', 'PASS: forced video RTP loss recovers on the next keyframe');

      async function measure(milliseconds) {
        var before = __chuzzWebrtcInspect(a);
        var start = Date.now();
        await sleep(milliseconds);
        var after = __chuzzWebrtcInspect(a);
        var seconds = (Date.now() - start) / 1000;
        return {
          bitrate: (after.videoBytesSent - before.videoBytesSent) * 8 / seconds,
          fps: (after.framesEncoded - before.framesEncoded) / seconds,
          width: after.videoWidth,
          height: after.videoHeight
        };
      }
      var initial = await measure(1500);
      assert(initial.bitrate > 100000 && initial.bitrate <= 1000000 + 80000,
        'measured full-size bitrate exceeded its cap');
      assert(initial.fps > 0 && initial.fps <= 10.8, 'configured frame rate was exceeded');

      parameters = sender.getParameters();
      parameters.encodings[0].maxBitrate = 100000;
      parameters.encodings[0].maxFramerate = 5;
      parameters.encodings[0].scaleResolutionDownBy = 2;
      await sender.setParameters(parameters);
      await until(function () {
        var report = __chuzzWebrtcInspect(a);
        return report.videoWidth <= 32 && report.videoHeight <= 24;
      }, 'lower sender budget did not scale the encoded dimensions');
      await sleep(300);
      var limited = await measure(1500);
      assert(limited.bitrate > 0 && limited.bitrate <= 100000 + 20000,
        'measured reduced-size bitrate exceeded its cap');
      assert(limited.fps > 0 && limited.fps <= 5.8, 'reduced frame rate was exceeded');
      text('video-budget', 'PASS: measured video bitrate, frame rate and dimensions follow sender caps');
      text('video-measured',
        'Measured UDP video: ' + Math.round(initial.bitrate) + ' bit/s, '
        + initial.fps.toFixed(2) + ' fps, ' + initial.width + 'x' + initial.height
        + '; limited: ' + Math.round(limited.bitrate) + ' bit/s, '
        + limited.fps.toFixed(2) + ' fps, ' + limited.width + 'x' + limited.height);

      a.close();
      b.close();
      await until(function () {
        return clone.readyState === 'ended'
          && !__chuzzWebrtcInspect(a).running && !__chuzzWebrtcInspect(b).running
          && __chuzzWebrtcInspect(a).sockets === 0 && __chuzzWebrtcInspect(b).sockets === 0;
      }, 'video close retained a receiver clone or transport');
      assert(camera.readyState === 'live', 'video peer close stopped the camera');
    } finally {
      local.pause();
      remote.pause();
      local.srcObject = null;
      remote.srcObject = null;
      a.close();
      b.close();
      if (stream) stream.getTracks().forEach(function (track) { track.stop(); });
    }
  };
})(globalThis);

