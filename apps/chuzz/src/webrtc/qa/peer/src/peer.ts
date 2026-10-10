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

  var running = false;
  document.getElementById('run').addEventListener('click', async function () {
    if (running) return;
    running = true;
    var a = null;
    var b = null;
    var stream = null;
    var pendingPeer = null;

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
      assert(receiver.track.getSettings().sampleRate === 8000, 'remote PCM contract');

      var trackEvents = [];
      b.ontrack = function (event) { trackEvents.push(event); };
      var offer = await a.createOffer();
      assert(offer.type === 'offer' && offer.sdp.includes('m=audio '), 'audio offer');
      assert(!/opus|VP8|VP9|H264|m=application|m=video/i.test(offer.sdp), 'unsupported codec advertised');
      assert(offer.sdp.includes('PCMU/8000'), 'synchronous audio codec missing');
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
      await b.setLocalDescription(answer);
      await a.setRemoteDescription(withoutCandidates(answer));
      assert(a.signalingState === 'stable' && b.signalingState === 'stable', 'stable signaling');
      assert(a.pendingLocalDescription === null && b.pendingRemoteDescription === null, 'pending commit');
      assert(a.currentLocalDescription.type === 'offer', 'current local offer');
      assert(b.currentLocalDescription.type === 'answer', 'current local answer');
      assert(sending.mid === receiving.mid && sending.mid !== null, 'stable MID mapping');
      assert(sending.currentDirection === 'sendonly' && receiving.currentDirection === 'recvonly',
        'negotiated directions');
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
        return trackEvents.length === 1 && __chuzzWebrtcInspect(b).samplesReceived >= 320;
      }, 'remote microphone audio did not arrive');
      var event = trackEvents[0];
      assert(event.receiver === receiver && event.transceiver === receiving, 'stable ontrack shells');
      assert(event.track === receiver.track && event.track.kind === 'audio', 'stable ontrack track');
      assert(event.streams.length === 1 && event.streams[0].id === stream.id, 'remote MSID mapping');
      assert(event.streams[0].getAudioTracks()[0] === receiver.track, 'remote stream track identity');
      assert(__chuzzWebrtcInspect(b).receivedEnergy > 0, 'received PCM contains no microphone tone');
      assert(receiver.track.readyState === 'live', 'remote track ended early');
      text('tracks', 'PASS: shared MediaStreamTrack identity and received microphone PCM');

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
      assert(a.connectionState === 'closed' && b.iceConnectionState === 'closed', 'close state');
      assert(receiver.track.readyState === 'ended', 'receiver lifecycle after close');
      assert(microphone.readyState === 'live', 'peer close stopped local capture');
      await until(function () {
        return [a, b, pendingPeer].every(function (pc) {
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
      if (stream) stream.getTracks().forEach(function (track) { track.stop(); });
      running = false;
    }
  });
})();

