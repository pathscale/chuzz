(function () {
  'use strict';

  var api = ChuzzEcho;
  var id = new URL(location.href).searchParams.get('session');
  api.assert(/^[1-9][0-9]*$/.test(id || ''), 'missing helper session');
  api.assert(typeof __chuzzWebrtcInspect === 'function', 'helper must be chuzz-headless');

  var channel = new api.Channel(id, 'helper');
  var peer = null;
  var started = false;
  var closed = false;

  async function prepareEcho(value, type) {
    var primary = value.pc.getTransceivers()[0];
    api.assert(primary && primary.receiver.track.kind === 'audio', 'missing echo receiver');
    if (type === 'offer') primary.direction = 'sendrecv';
    await primary.sender.replaceTrack(primary.receiver.track);
    api.assert(primary.sender.track === primary.receiver.track,
      'echo must send its received PCM track');
  }

  async function snapshot() {
    api.assert(peer, 'helper peer has not started');
    return {
      report: await peer.report(),
      signalingState: peer.pc.signalingState,
      connectionState: peer.pc.connectionState,
      iceConnectionState: peer.pc.iceConnectionState,
      iceChecks: peer.iceChecks,
      sentCandidates: peer.sentCandidates,
      receivedCandidates: peer.receivedCandidates,
      local: peer.pc.localDescription ? api.credentials(peer.pc.localDescription) : null,
      remote: peer.pc.remoteDescription ? api.credentials(peer.pc.remoteDescription) : null,
      slots: peer.pc.getTransceivers().map(function (transceiver) {
        return {
          mid: transceiver.mid,
          direction: transceiver.direction,
          currentDirection: transceiver.currentDirection,
          ended: transceiver.receiver.track.readyState === 'ended'
        };
      })
    };
  }

  async function cleanup() {
    if (!peer || closed) return;
    closed = true;
    peer.pc.close();
    await api.until(async function () {
      var report = await peer.report();
      return !report.running && report.sockets === 0;
    }, 'helper retained transport sockets');
    api.assert(peer.pc.getReceivers().every(function (receiver) {
      return receiver.track.readyState === 'ended';
    }), 'helper receiver remained live after close');
  }

  channel.start(async function (message) {
    try {
      if (message.kind === 'start') {
        api.assert(!started, 'helper already started');
        api.assert(['offerer', 'answerer'].includes(message.role), 'invalid helper role');
        api.assert(['opus', 'pcmu'].includes(message.codec), 'invalid helper codec');
        started = true;
        peer = new api.Peer(channel, prepareEcho);
        var primary = peer.pc.addTransceiver('audio', { direction: 'sendrecv' });
        primary.setCodecPreferences(
          RTCRtpSender.getCapabilities('audio').codecs.filter(function (codec) {
            return codec.mimeType.toLowerCase() === 'audio/' + message.codec;
          })
        );
        api.paint('result', 'Helper ready without capture or a local tone source');
        await channel.send({ kind: 'started', phase: 'start' });
      } else if (peer && await peer.handle(message)) {
        return;
      } else if (message.kind === 'offer') {
        api.assert(peer && !closed, 'offer requires a live helper');
        if (message.restart) peer.pc.restartIce();
        await peer.offer(message.phase);
      } else if (message.kind === 'probe') {
        await channel.send(Object.assign({
          kind: 'probe', phase: message.phase
        }, await snapshot()));
      } else if (message.kind === 'stale') {
        api.assert(peer && !closed, 'stale candidate check requires a live helper');
        await api.rejectsCandidate(peer.pc, message.candidate);
        await channel.send({ kind: 'stale', phase: message.phase });
      } else if (message.kind === 'close') {
        await cleanup();
        await channel.send(Object.assign({
          kind: 'closed', phase: message.phase
        }, await snapshot()));
        api.paint('result', 'READY: helper CLOSED');
      } else if (message.kind !== 'settled') {
        throw new Error('unknown helper command: ' + message.kind);
      }
    } catch (error) {
      await cleanup();
      api.paint('result', 'FAIL: ' + error.name + ': ' + error.message);
      await channel.send({
        kind: 'error',
        message: error.name + ': ' + error.message
      });
      throw error;
    }
  });

  channel.send({ kind: 'ready', phase: 'ready' }).catch(function (error) {
    api.paint('result', 'FAIL: ' + error.message);
  });

  var deadline = setTimeout(async function () {
    await cleanup();
    await channel.stop();
    api.paint('result', 'Helper lease expired');
  }, 110000);

  window.addEventListener('pagehide', function () {
    clearTimeout(deadline);
    if (peer) peer.pc.close();
    channel.stop();
  });
})();

