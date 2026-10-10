(function () {
  'use strict';

  var api = ChuzzEcho;
  var running = false;
  var evidence = [];
  var control = new URL(location.href).searchParams.get('control') === '1';

  function record(value) {
    evidence.push(value);
    document.getElementById('evidence').textContent = JSON.stringify(evidence, null, 2);
  }

  async function scenario(role, codec) {
    var opened = null;
    var channel = null;
    var peer = null;
    var context = null;
    var stream = null;
    var clone = null;
    var monitor = null;
    var oscillator = null;
    var sessionDeleted = false;
    var label = role + '-' + codec;

    async function probe(phase) {
      await channel.send({ kind: 'probe', phase: phase });
      return channel.wait('probe', phase);
    }

    async function settled(phase) {
      await channel.wait('settled', phase);
      api.assert(peer.pc.signalingState === 'stable', label + ': signaling not stable');
    }

    async function advancing(node, before) {
      var measured = null;
      await api.until(async function () {
        measured = await peer.report();
        return peer.pc.connectionState === 'connected'
          && ['connected', 'completed'].includes(peer.pc.iceConnectionState)
          && api.received(measured) > before
          && api.tone(node, context);
      }, label + ': returned tone or fresh received audio missing',
      function () { channel.check(); });
      api.transport(measured, codec);
      return measured;
    }

    try {
      api.assert(typeof __chuzzWebrtcInspect === 'function' || control,
        'ordinary browser control requires ?control=1');
      opened = await api.request('/api/echo', { method: 'POST', body: '{}' });
      api.assert(opened.session > 0 && opened.pid > 0, 'missing launched helper PID');
      channel = new api.Channel(opened.session, 'page');
      peer = new api.Peer(channel, null);
      channel.start(async function (message) {
        if (!await peer.handle(message)) channel.remember(message);
      });

      await channel.wait('ready', 'ready');
      await channel.send({
        kind: 'start',
        role: role === 'offerer' ? 'answerer' : 'offerer',
        codec: codec
      });
      await channel.wait('started', 'start');

      context = new AudioContext({ sampleRate: 48000 });
      if (peer.native) {
        stream = await navigator.mediaDevices.getUserMedia({
          audio: { sampleRate: { exact: 8000 }, channelCount: { exact: 1 } }
        });
      } else {
        var destination = context.createMediaStreamDestination();
        oscillator = context.createOscillator();
        oscillator.frequency.value = 440;
        var gain = context.createGain();
        gain.gain.value = 0.2;
        oscillator.connect(gain);
        gain.connect(destination);
        oscillator.start();
        stream = destination.stream;
      }
      var microphone = stream.getAudioTracks()[0];
      api.assert(microphone && microphone.readyState === 'live', 'missing microphone track');
      var primary = peer.pc.addTransceiver(microphone, {
        direction: 'sendrecv', streams: [stream]
      });
      var receiver = primary.receiver;
      var remote = new MediaStream([receiver.track]);
      monitor = api.analyser(context, remote);
      await context.resume();

      if (role === 'offerer') {
        await peer.offer('initial');
      } else {
        await channel.send({ kind: 'offer', phase: 'initial' });
      }
      await settled('initial');
      var initial = await advancing(monitor.node, 0);
      var helperInitial = await probe('initial-report');
      api.transport(helperInitial.report, codec);
      api.assert(helperInitial.report.native, 'helper did not use the native transport');
      api.assert(helperInitial.signalingState === 'stable'
        && helperInitial.connectionState === 'connected', 'helper did not connect');
      api.assert(peer.sentCandidates > 0 && peer.receivedCandidates > 0
        && helperInitial.sentCandidates > 0 && helperInitial.receivedCandidates > 0,
      'missing separately exchanged candidates');
      api.assert(primary.mid !== null && primary.mid === helperInitial.slots[0].mid,
        'initial MID mismatch');
      api.assert(primary.currentDirection === 'sendrecv'
        && helperInitial.slots[0].currentDirection === 'sendrecv', 'echo is not duplex');
      if (codec === 'opus' && peer.native) {
        api.assert(primary.sender.getParameters().codecs[0].mimeType === 'audio/opus'
          && receiver.getParameters().codecs[0].mimeType === 'audio/opus',
        'Opus was not negotiated');
      }

      var mutedBefore = api.received(initial);
      microphone.enabled = false;
      await api.until(function () {
        return api.rms(monitor.node) < 0.005;
      }, label + ': helper did not echo microphone silence',
      function () { channel.check(); });
      var quietUntil = Date.now() + 300;
      while (Date.now() < quietUntil) {
        channel.check();
        api.assert(api.rms(monitor.node) < 0.01, 'returned tone survived microphone mute');
        await api.sleep(20);
      }
      var muted = await peer.report();
      api.assert(api.received(muted) > mutedBefore, 'muted return stopped delivering audio');
      microphone.enabled = true;
      var unmuted = await advancing(monitor.node, api.received(muted));

      var originalMid = primary.mid;
      clone = microphone.clone();
      var extra = peer.pc.addTransceiver(clone, {
        direction: 'sendonly', streams: [stream]
      });
      await peer.offer('add');
      await settled('add');
      var added = await advancing(monitor.node, api.received(unmuted));
      var helperAdded = await probe('add-report');
      api.assert(peer.pc.getTransceivers().length === 2
        && helperAdded.slots.length === 2, 'additional audio slot was not negotiated');
      api.assert(extra.mid !== null && extra.currentDirection === 'sendonly'
        && helperAdded.slots[1].currentDirection === 'recvonly',
      'additional audio direction mismatch');
      api.assert(primary.mid === originalMid && primary.receiver === receiver,
        'addition replaced the primary MID or receiver');

      // The current SDP profile reserves established m-lines. Removing this
      // media retires its active directions while preserving its MID.
      var extraMid = extra.mid;
      peer.pc.removeTrack(extra.sender);
      extra.direction = 'inactive';
      clone.stop();
      await peer.offer('remove');
      await settled('remove');
      var removed = await advancing(monitor.node, api.received(added));
      var helperRemoved = await probe('remove-report');
      api.assert(extra.sender.track === null && extra.currentDirection === 'inactive',
        'removed sender is still active');
      api.assert(extra.mid === extraMid && helperRemoved.slots[1].mid === extraMid
        && helperRemoved.slots[1].currentDirection === 'inactive',
      'inactive slot lost its reserved MID');
      api.assert(primary.mid === originalMid && primary.receiver === receiver,
        'removal replaced the primary MID or receiver');

      var oldLocal = api.credentials(peer.pc.localDescription);
      var oldRemote = api.credentials(peer.pc.remoteDescription);
      var oldPageCandidate = peer.localCandidates[0];
      var oldHelperCandidate = peer.lastRemoteCandidate;
      var checksBefore = peer.iceChecks;
      var helperChecksBefore = helperRemoved.iceChecks;
      if (role === 'offerer') {
        peer.pc.restartIce();
        await peer.offer('restart');
      } else {
        await channel.send({ kind: 'offer', phase: 'restart', restart: true });
      }
      await settled('restart');
      var newLocal = api.credentials(peer.pc.localDescription);
      var newRemote = api.credentials(peer.pc.remoteDescription);
      var oldInitiator = role === 'offerer' ? oldLocal : oldRemote;
      var newInitiator = role === 'offerer' ? newLocal : newRemote;
      api.assert(newInitiator.ufrag !== oldInitiator.ufrag
        && newInitiator.password !== oldInitiator.password,
      'restart did not replace the initiating ICE credentials');
      api.assert(newLocal.fingerprint === oldLocal.fingerprint
        && newRemote.fingerprint === oldRemote.fingerprint,
      'ICE restart replaced a DTLS certificate');
      var restarted = await advancing(monitor.node, api.received(removed));
      var helperRestarted = await probe('restart-report');
      api.transport(helperRestarted.report, codec);
      api.assert(helperRestarted.report.samplesReceived > helperRemoved.report.samplesReceived
        && helperRestarted.report.samplesSent > helperRemoved.report.samplesSent,
      'helper audio did not advance through restart');
      api.assert(peer.iceChecks > checksBefore
        && helperRestarted.iceChecks > helperChecksBefore,
      'restart did not drive fresh ICE checking on both peers');
      api.assert(primary.mid === originalMid && primary.receiver === receiver,
        'restart replaced the primary MID or receiver');

      if (role === 'offerer') {
        await channel.send({
          kind: 'stale', phase: 'stale-generation', candidate: oldPageCandidate
        });
        await channel.wait('stale', 'stale-generation');
      } else {
        await api.rejectsCandidate(peer.pc, oldHelperCandidate);
      }

      await channel.send({ kind: 'close', phase: 'close' });
      var helperClosed = await channel.wait('closed', 'close');
      api.assert(helperClosed.connectionState === 'closed'
        && helperClosed.iceConnectionState === 'closed'
        && !helperClosed.report.running && helperClosed.report.sockets === 0,
      'helper retained its transport');
      api.assert(helperClosed.slots.every(function (slot) { return slot.ended; }),
        'helper receiver tracks remained live');

      peer.pc.close();
      if (peer.native) {
        await api.until(async function () {
          var report = await peer.report();
          return !report.running && report.sockets === 0;
        }, 'page retained UDP sockets');
        var closedReport = await peer.report();
        await api.sleep(100);
        var stableReport = await peer.report();
        api.assert(stableReport.samplesReceived === closedReport.samplesReceived
          && stableReport.udpReceived === closedReport.udpReceived
          && stableReport.udpSent === closedReport.udpSent,
        'closed page transport continued processing packets');
      }
      api.assert(peer.pc.connectionState === 'closed'
        && peer.pc.getReceivers().every(function (value) {
          return value.track.readyState === 'ended';
        }), 'page receiver cleanup failed');
      api.assert(microphone.readyState === 'live', 'peer close stopped caller-owned capture');
      monitor.source.disconnect();
      monitor.node.disconnect();
      if (oscillator) {
        oscillator.stop();
        oscillator = null;
      }
      await context.close();
      api.assert(context.state === 'closed', 'audio graph remained open');
      var closedAudio = {
        state: context.state,
        currentTime: context.currentTime
      };
      if (peer.native) closedAudio.samples = __chuzzAudioInspect(context).samples;
      // AudioNodes remain valid after close. Repeated disconnects must not
      // throw or restart the render clock, including disconnect(output).
      monitor.source.disconnect();
      monitor.node.disconnect();
      monitor.source.disconnect(0);
      monitor.node.disconnect(0);
      await api.sleep(100);
      api.assert(context.state === 'closed'
        && context.currentTime === closedAudio.currentTime,
      'disconnect after close restarted the audio graph');
      if (peer.native) {
        api.assert(__chuzzAudioInspect(context).samples === closedAudio.samples,
          'closed audio graph continued rendering');
      }
      closedAudio.disconnectAfterClose = true;
      stream.getTracks().forEach(function (track) { track.stop(); });
      api.assert(microphone.readyState === 'ended', 'capture owner failed to stop its track');

      await channel.stop();
      var deleted = await api.request('/api/echo/' + opened.session, { method: 'DELETE' });
      sessionDeleted = true;
      api.assert(deleted.pid === opened.pid && deleted.reaped
        && deleted.sessionClosed && deleted.messages === 0, 'helper process was not reaped');
      try {
        await api.request('/api/echo/' + opened.session + '/page?after=0');
        throw new Error('deleted signaling session survived');
      } catch (error) {
        api.assert(error.status === 410, 'deleted session did not return Gone');
      }

      record({
        scenario: label,
        helperPid: opened.pid,
        page: initial,
        helper: helperInitial,
        added: helperAdded,
        removed: helperRemoved,
        restartedPage: restarted,
        restartedHelper: helperRestarted,
        closedHelper: helperClosed,
        closedAudio: closedAudio,
        processCleanup: deleted,
        control: !peer.native
      });
    } finally {
      if (peer) peer.pc.close();
      if (clone) clone.stop();
      if (monitor) {
        monitor.source.disconnect();
        monitor.node.disconnect();
      }
      if (oscillator) {
        try { oscillator.stop(); } catch (_) {}
      }
      if (context && context.state !== 'closed') await context.close();
      if (stream) stream.getTracks().forEach(function (track) { track.stop(); });
      if (channel) await channel.stop();
      if (opened && !sessionDeleted) {
        await api.request('/api/echo/' + opened.session, { method: 'DELETE' });
      }
    }
  }

  document.getElementById('run').addEventListener('click', async function () {
    if (running) return;
    running = true;
    var button = document.getElementById('run');
    button.disabled = true;
    evidence = [];
    document.getElementById('evidence').textContent = '';
    try {
      for (var role of ['offerer', 'answerer']) {
        for (var codec of ['opus', 'pcmu']) {
          api.paint('result', 'Running ' + role + ' ' + codec);
          await scenario(role, codec);
        }
      }
      api.paint('process', 'PASS: fixture-launched helper processes and sequenced HTTP signaling');
      api.paint('roles', 'PASS: page offerer and page answerer');
      api.paint('candidates', 'PASS: candidates exchanged separately from SDP');
      api.paint('transport', 'PASS: real UDP and authenticated DTLS 1.2 in both processes');
      api.paint('tone', 'PASS: returned Opus 440 Hz tone follows microphone mute and unmute');
      api.paint('renegotiation', 'PASS: added and removed audio preserve the primary MID and receiver');
      api.paint('restart', 'PASS: both restart initiators change credentials and reject stale candidates');
      api.paint('fallback', 'PASS: asymmetric Opus and PCMU preferences return the microphone tone');
      api.paint('close', 'PASS: both peers release sockets, tracks and graphs; helpers are reaped');
      api.paint('result', 'READY: echo PASS');
    } catch (error) {
      api.paint('result', 'FAIL: ' + error.name + ': ' + error.message);
      throw error;
    } finally {
      running = false;
      button.disabled = false;
    }
  });
})();

