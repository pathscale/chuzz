(function () {
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
  async function rejects(operation, name) {
    try {
      await operation();
    } catch (caught) {
      assert(caught.name === name, 'wrong rejection: ' + caught.name + ': ' + caught.message);
      return;
    }
    throw new Error('operation unexpectedly succeeded');
  }
  function withoutCandidates(description) {
    return {
      type: description.type,
      sdp: description.sdp.split(/\r?\n/).filter(function (line) {
        return !line.startsWith('a=candidate:') && line !== 'a=end-of-candidates';
      }).join('\r\n')
    };
  }
  function monitor(context, remoteStream) {
    var source = context.createMediaStreamSource(remoteStream);
    var analyser = new AnalyserNode(context, {
      fftSize: 1024, minDecibels: -100, maxDecibels: 0, smoothingTimeConstant: 0
    });
    source.connect(analyser);
    return analyser;
  }
  function rms(analyser) {
    var samples = new Float32Array(analyser.fftSize);
    analyser.getFloatTimeDomainData(samples);
    var energy = 0;
    for (var sample of samples) energy += sample * sample;
    return Math.sqrt(energy / samples.length);
  }
  function tone(analyser, context) {
    var bins = new Uint8Array(analyser.frequencyBinCount);
    analyser.getByteFrequencyData(bins);
    var index = 1;
    for (var current = 2; current < bins.length; current++) {
      if (bins[current] > bins[index]) index = current;
    }
    return bins[index] > 120
      && Math.abs(index * context.sampleRate / analyser.fftSize - 440)
        < context.sampleRate / analyser.fftSize;
  }

  var running = false;
  document.getElementById('run').addEventListener('click', async function () {
    if (running) return;
    running = true;
    var a = null;
    var b = null;
    var stream = null;
    var pendingPeer = null;
    var context = null;
    var fallbackA = null;
    var fallbackB = null;

    try {
      assert(typeof RTCPeerConnection === 'function', 'peer constructor unavailable');
      stream = await navigator.mediaDevices.getUserMedia({
        audio: { sampleRate: { exact: 8000 }, channelCount: { exact: 1 } }
      });
      var microphone = stream.getAudioTracks()[0];
      a = new RTCPeerConnection();
      b = new RTCPeerConnection();

      var candidates = [[], []];
      var finalCandidates = [0, 0];
      var events = [0, 0];
      var propertyEvents = [0, 0];
      [a, b].forEach(function (pc, side) {
        pc.onicecandidate = function (event) {
          if (event.candidate === null) {
            finalCandidates[side]++;
            return;
          }
          var candidate = event.candidate;
          assert(candidate instanceof RTCIceCandidate, 'candidate identity');
          assert(candidate.protocol === 'udp' && candidate.type === 'host', 'host UDP candidate');
          assert(candidate.address !== '0.0.0.0' && candidate.address !== '::', 'wildcard candidate');
          assert(candidate.port > 0 && candidate.sdpMid !== null, 'candidate routing fields');
          assert(candidate.usernameFragment, 'candidate ICE generation');
          candidates[side].push(candidate.toJSON());
        };
        pc.addEventListener('connectionstatechange', function () { events[side]++; });
        pc.addEventListener('iceconnectionstatechange', function () { events[side]++; });
        pc.onconnectionstatechange = function () { propertyEvents[side]++; };
      });

      var sender = a.addTrack(microphone, stream);
      var sending = a.getTransceivers()[0];
      var receiving = b.addTransceiver('audio', { direction: 'recvonly' });
      var receiver = receiving.receiver;
      assert(sender instanceof RTCRtpSender && receiver instanceof RTCRtpReceiver, 'RTP shells');
      assert(sending instanceof RTCRtpTransceiver && sending.mid === null, 'unassociated MID');
      assert(sender.track === microphone, 'sender capture identity');
      assert(receiver.track instanceof MediaStreamTrack, 'shared remote track implementation');
      assert(receiver.track.getSettings().sampleRate === 48000, 'remote PCM rate');
      assert(receiver.track.getSettings().channelCount === 2, 'remote PCM channel contract');
      var parameters = sender.getParameters();
      parameters.encodings[0].maxBitrate = 32000;
      parameters.encodings[0].cbr = true;
      await sender.setParameters(parameters);
      assert(sender.getParameters().encodings[0].cbr, 'sender CBR setting was not retained');

      var trackEvents = [];
      b.ontrack = function (event) { trackEvents.push(event); };
      var offer = await a.createOffer();
      assert(offer.type === 'offer' && offer.sdp.includes('m=audio '), 'audio offer');
      assert(!/VP8|VP9|H264|m=application|m=video/i.test(offer.sdp), 'unsupported codec advertised');
      assert(/opus\/48000\/2/i.test(offer.sdp), 'Opus RTP mapping missing');
      assert(offer.sdp.includes('PCMU/8000'), 'fallback audio codec missing');
      assert(offer.sdp.includes('minptime=10') && offer.sdp.includes('useinbandfec=1')
        && offer.sdp.includes('stereo=1'), 'Opus receive preferences missing');
      var opusPayload = /a=rtpmap:(\d+) opus\/48000\/2/i.exec(offer.sdp)[1];
      var mline = offer.sdp.split(/\r?\n/).find(function (line) { return line.startsWith('m=audio '); });
      assert(mline.split(/\s+/)[3] === opusPayload, 'Opus is not the preferred audio codec');
      await a.setLocalDescription(offer);
      assert(a.signalingState === 'have-local-offer', 'local offer state');
      assert(a.pendingLocalDescription.sdp === offer.sdp, 'pending local description');
      await b.setRemoteDescription(withoutCandidates(offer));
      assert(b.signalingState === 'have-remote-offer', 'remote offer state');

      var remoteBefore = b.remoteDescription.sdp;
      var receiverBefore = b.getReceivers()[0];
      await rejects(function () {
        return b.setRemoteDescription({ type: 'answer', sdp: 'invalid' });
      }, 'InvalidStateError');
      assert(b.remoteDescription.sdp === remoteBefore, 'rejection replaced remote SDP');
      assert(b.getReceivers()[0] === receiverBefore, 'rejection replaced receiver');

      var answer = await b.createAnswer();
      assert(/opus\/48000\/2/i.test(answer.sdp), 'answer rejected Opus');
      await b.setLocalDescription(answer);
      await a.setRemoteDescription(withoutCandidates(answer));
      assert(a.signalingState === 'stable' && b.signalingState === 'stable', 'stable signaling');
      assert(a.pendingLocalDescription === null && b.pendingRemoteDescription === null, 'pending commit');
      assert(a.currentLocalDescription.type === 'offer', 'current local offer');
      assert(b.currentLocalDescription.type === 'answer', 'current local answer');
      assert(sending.mid === receiving.mid && sending.mid !== null, 'stable MID mapping');
      assert(sending.currentDirection === 'sendonly' && receiving.currentDirection === 'recvonly',
        'negotiated directions');
      assert(sender.getParameters().codecs[0].mimeType === 'audio/opus', 'sender negotiated codec');
      assert(receiver.getParameters().codecs[0].mimeType === 'audio/opus', 'receiver negotiated codec');
      text('sdp', 'PASS: audio SDP, stable slots and atomic malformed-SDP rejection');

      await until(function () {
        return finalCandidates[0] === 1 && finalCandidates[1] === 1;
      }, 'candidate gathering did not finish');
      assert(candidates[0].length > 0 && candidates[1].length > 0, 'no host candidates');
      await rejects(function () {
        return b.addIceCandidate(Object.assign({}, candidates[0][0], { sdpMid: 'missing' }));
      }, 'OperationError');
      for (var candidate of candidates[0]) await b.addIceCandidate(candidate);
      for (var candidate of candidates[1]) await a.addIceCandidate(candidate);
      await a.addIceCandidate(null);
      await b.addIceCandidate(null);
      assert(a.iceGatheringState === 'complete' && b.iceGatheringState === 'complete', 'gathering state');
      text('candidates', 'PASS: gathered host candidates, trickle exchange and final null events');

      await until(function () {
        return a.connectionState === 'connected' && b.connectionState === 'connected';
      }, 'peers did not connect');
      [a, b].forEach(function (pc) {
        var report = __chuzzWebrtcInspect(pc);
        assert(report.udpSent > 0 && report.udpReceived > 0, 'no real UDP traffic');
        assert(report.dtlsSent > 0 && report.dtlsReceived > 0, 'no real DTLS traffic');
        assert(report.dtlsConnected && report.dtlsVersion === 'DTLS1.2', 'DTLS authentication');
        assert(report.remoteFingerprint, 'remote certificate pin unavailable');
        assert(['connected', 'completed'].includes(pc.iceConnectionState), 'ICE state');
      });
      text('transport', 'PASS: real UDP and authenticated DTLS 1.2 on both peers');

      await until(function () {
        return trackEvents.length === 1 && __chuzzWebrtcInspect(b).samplesReceived >= 9600;
      }, 'remote microphone audio did not arrive');
      var event = trackEvents[0];
      assert(event.receiver === receiver && event.transceiver === receiving, 'stable ontrack shells');
      assert(event.track === receiver.track && event.track.kind === 'audio', 'stable ontrack track');
      assert(event.streams.length === 1 && event.streams[0].id === stream.id, 'remote MSID mapping');
      assert(event.streams[0].getAudioTracks()[0] === receiver.track, 'remote stream track identity');
      assert(__chuzzWebrtcInspect(b).receivedEnergy > 0, 'received PCM contains no microphone tone');
      assert(receiver.track.readyState === 'live', 'remote track ended early');
      text('tracks', 'PASS: shared MediaStreamTrack identity and received microphone PCM');

      context = new AudioContext({ sampleRate: 48000 });
      var analyser = monitor(context, event.streams[0]);
      await context.resume();
      await until(function () {
        return rms(analyser) > 0.03 && tone(analyser, context);
      }, 'remote Opus analyser did not show the fake microphone 440 Hz tone');
      var encoded = __chuzzWebrtcInspect(a);
      assert(encoded.txCodec === 'opus' && __chuzzWebrtcInspect(b).rxCodec === 'opus',
        'actual RTP codec is not Opus');
      assert(encoded.opusPacketsSent >= 10, 'too few measured Opus packets');
      assert(encoded.opusMinPacketBytes === encoded.opusMaxPacketBytes,
        'CBR produced variable packet lengths');
      assert(encoded.opusMinPacketBytes === 80, '32 kbit CBR packet budget was not applied');
      text('codec', 'PASS: negotiated Opus, 32 kbit CBR and remote 440 Hz analyser tone');

      var beforeLoss = __chuzzWebrtcInspect(b);
      await __chuzzWebrtcDropAudio(b, 1);
      await until(function () {
        var report = __chuzzWebrtcInspect(b);
        return report.audioDropped > beforeLoss.audioDropped
          && report.concealedFrames >= beforeLoss.concealedFrames + 960
          && report.fecAttempts > beforeLoss.fecAttempts;
      }, 'lost RTP packet did not exercise next-packet FEC or its PLC fallback');
      var beforeBurst = __chuzzWebrtcInspect(b);
      await __chuzzWebrtcDropAudio(b, 2);
      var quietSince = null;
      var observing = Date.now() + 500;
      while (Date.now() < observing) {
        if (rms(analyser) < 0.01) {
          if (quietSince === null) quietSince = Date.now();
          assert(Date.now() - quietSince <= 20, 'remote analyser had a silence gap over one frame');
        } else {
          quietSince = null;
        }
        await sleep(5);
      }
      await until(function () {
        var report = __chuzzWebrtcInspect(b);
        return report.audioDropped >= beforeBurst.audioDropped + 2
          && report.plcFrames >= beforeBurst.plcFrames + 960;
      }, 'consecutive RTP loss did not exercise PLC');
      var afterLoss = __chuzzWebrtcInspect(b);
      assert(afterLoss.maxSilenceFrames <= 960, 'decoded PCM silence exceeded one frame');
      assert(afterLoss.maxPlayoutGapFrames <= 960, 'receiver PCM timeline has a gap over one frame');
      assert(afterLoss.codecErrors === 0, 'Opus decoding reported an error');
      assert(tone(analyser, context), 'remote tone did not recover after RTP loss');
      text('loss', 'PASS: forced RTP loss uses FEC and PLC without a silence gap over 20 ms');

      fallbackA = new RTCPeerConnection();
      fallbackB = new RTCPeerConnection();
      fallbackA.addTrack(microphone, stream);
      fallbackA.getTransceivers()[0].setCodecPreferences(
        RTCRtpSender.getCapabilities('audio').codecs.filter(function (codec) {
          return codec.mimeType.toLowerCase() === 'audio/pcmu';
        })
      );
      var fallbackReceiving = fallbackB.addTransceiver('audio', { direction: 'recvonly' });
      var fallbackOffer = await fallbackA.createOffer();
      assert(!/opus/i.test(fallbackOffer.sdp) && fallbackOffer.sdp.includes('PCMU/8000'),
        'PCMU-only codec preference was not applied');
      await fallbackA.setLocalDescription(fallbackOffer);
      await fallbackB.setRemoteDescription(fallbackOffer);
      var fallbackAnswer = await fallbackB.createAnswer();
      await fallbackB.setLocalDescription(fallbackAnswer);
      await fallbackA.setRemoteDescription(fallbackAnswer);
      var fallbackMonitor = monitor(context, new MediaStream([fallbackReceiving.receiver.track]));
      await until(function () {
        return __chuzzWebrtcInspect(fallbackA).txCodec === 'pcmu'
          && __chuzzWebrtcInspect(fallbackB).rxCodec === 'pcmu'
          && rms(fallbackMonitor) > 0.03 && tone(fallbackMonitor, context);
      }, 'PCMU fallback did not deliver the remote microphone tone');
      text('fallback', 'PASS: PCMU fallback delivers the remote microphone tone');

      await __chuzzPeerVideoQA();

      await until(function () {
        return events.every(function (count) { return count > 0; })
          && propertyEvents.every(function (count) { return count > 0; });
      }, 'state handlers did not fire');
      var stats = await a.getStats();
      assert(stats instanceof Map && stats.size === 1, 'measured transport stats');
      text('events', 'PASS: property handlers and EventTarget state listeners');

      pendingPeer = new RTCPeerConnection();
      pendingPeer.addTransceiver('audio');
      await pendingPeer.setLocalDescription(await pendingPeer.createOffer());
      pendingPeer.close();
      a.close();
      b.close();
      fallbackA.close();
      fallbackB.close();
      assert(a.connectionState === 'closed' && b.iceConnectionState === 'closed', 'close state');
      assert(receiver.track.readyState === 'ended', 'receiver lifecycle after close');
      assert(microphone.readyState === 'live', 'peer close stopped local capture');
      await until(function () {
        return [a, b, pendingPeer, fallbackA, fallbackB].every(function (pc) {
          var report = __chuzzWebrtcInspect(pc);
          return !report.running && report.sockets === 0;
        });
      }, 'closed transport retained UDP sockets');
      await rejects(function () { return a.createOffer(); }, 'InvalidStateError');
      text('close', 'PASS: closed peers release sockets and preserve local capture');
      text('result', 'READY: peer PASS');
    } catch (caught) {
      text('result', 'FAIL: ' + caught.name + ': ' + caught.message);
      throw caught;
    } finally {
      if (a) a.close();
      if (b) b.close();
      if (pendingPeer) pendingPeer.close();
      if (fallbackA) fallbackA.close();
      if (fallbackB) fallbackB.close();
      if (context) await context.close();
      if (stream) stream.getTracks().forEach(function (track) { track.stop(); });
      running = false;
    }
  });
})();

