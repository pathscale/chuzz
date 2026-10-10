// JavaScript syntax is intentional: fixture scripts execute without a bundle.
(function (g) {
  'use strict';

  function assert(value, message) {
    if (!value) throw new Error(message);
  }

  function sleep(milliseconds) {
    return new Promise(function (resolve) { setTimeout(resolve, milliseconds); });
  }

  async function until(predicate, message, check) {
    var deadline = Date.now() + 12000;
    while (true) {
      if (check) check();
      if (await predicate()) return;
      assert(Date.now() < deadline, message);
      await sleep(20);
    }
  }

  function paint(id, value) {
    var element = document.getElementById(id);
    element.setAttribute('aria-label', value);
    element.setAttribute('data-state', value.startsWith('FAIL:') ? 'fail' : 'pass');
    element.textContent = value;
  }

  async function request(path, options) {
    options = Object.assign({}, options || {});
    options.headers = Object.assign({
      'X-Chuzz-Echo': 'w09',
      'Content-Type': 'application/json'
    }, options.headers || {});
    options.cache = 'no-store';
    var response = await fetch(path, options);
    var value = await response.json();
    if (!response.ok) {
      var failure = new Error('signaling ' + response.status + ': ' + value.error);
      failure.status = response.status;
      throw failure;
    }
    return value;
  }

  function Channel(session, side) {
    this.path = '/api/echo/' + session + '/' + side;
    this.after = 0;
    this.failure = null;
    this.stopped = false;
    this.controller = null;
    this.outgoing = Promise.resolve();
    this.messages = [];
    this.loop = null;
  }

  Channel.prototype.check = function () {
    if (this.failure) throw this.failure;
  };

  Channel.prototype.send = function (payload) {
    var self = this;
    var operation = self.outgoing.then(function () {
      self.check();
      assert(!self.stopped, 'signaling channel stopped');
      return request(self.path, {
        method: 'POST',
        body: JSON.stringify(payload)
      });
    });
    self.outgoing = operation.catch(function (error) { self.failure = error; });
    return operation;
  };

  Channel.prototype.remember = function (message) {
    if (message.kind === 'error') {
      throw new Error('helper: ' + message.message);
    }
    assert(this.messages.length < 128, 'page signaling message limit');
    this.messages.push(message);
  };

  Channel.prototype.wait = async function (kind, phase) {
    var self = this;
    var found = null;
    await until(function () {
      var index = self.messages.findIndex(function (message) {
        return message.kind === kind && message.phase === phase;
      });
      if (index < 0) return false;
      found = self.messages.splice(index, 1)[0];
      return true;
    }, 'missing ' + kind + ' for ' + phase, function () { self.check(); });
    return found;
  };

  Channel.prototype.start = function (handler) {
    var self = this;
    assert(self.loop === null, 'signaling reader already started');
    self.loop = (async function () {
      while (!self.stopped) {
        self.controller = new AbortController();
        var batch = await request(self.path + '?after=' + self.after, {
          signal: self.controller.signal
        });
        for (var message of batch.messages) {
          if (self.stopped) return;
          assert(message.sequence === self.after + 1, 'signaling sequence gap');
          await handler(message.payload);
          self.after = message.sequence;
        }
      }
    })().catch(function (error) {
      if (!self.stopped) self.failure = error;
    });
  };

  Channel.prototype.stop = async function () {
    this.stopped = true;
    if (this.controller) this.controller.abort();
    if (this.loop) await this.loop;
  };

  function attribute(description, name) {
    var prefix = 'a=' + name + ':';
    var line = description.sdp.split(/\r?\n/).find(function (line) {
      return line.startsWith(prefix);
    });
    assert(line, 'missing SDP ' + name);
    return line.slice(prefix.length);
  }

  function credentials(description) {
    return {
      ufrag: attribute(description, 'ice-ufrag'),
      password: attribute(description, 'ice-pwd'),
      fingerprint: attribute(description, 'fingerprint')
    };
  }

  function withoutCandidates(description) {
    return {
      type: description.type,
      sdp: description.sdp.split(/\r?\n/).filter(function (line) {
        return !line.startsWith('a=candidate:') && line !== 'a=end-of-candidates';
      }).join('\r\n')
    };
  }

  function sdpCandidates(description) {
    var sections = [];
    var current = null;
    var defaults = { ufrag: null };
    description.sdp.split(/\r?\n/).forEach(function (line) {
      if (line.startsWith('m=')) {
        current = {
          index: sections.length, mid: null, ufrag: defaults.ufrag, candidates: []
        };
        sections.push(current);
      } else if (line.startsWith('a=ice-ufrag:')) {
        (current || defaults).ufrag = line.slice(12);
      } else if (current && line.startsWith('a=mid:')) {
        current.mid = line.slice(6);
      } else if (current && line.startsWith('a=candidate:')) {
        current.candidates.push(line.slice(2));
      }
    });
    var result = [];
    sections.forEach(function (section) {
      section.candidates.forEach(function (candidate) {
        result.push({
          candidate: candidate,
          sdpMid: section.mid,
          sdpMLineIndex: section.index,
          usernameFragment: section.ufrag
        });
      });
    });
    return result;
  }

  function Peer(channel, prepare) {
    this.channel = channel;
    this.prepare = prepare;
    this.pc = new RTCPeerConnection({ iceServers: [] });
    this.native = typeof g.__chuzzWebrtcInspect === 'function';
    this.localCandidates = [];
    this.lastRemoteCandidate = null;
    this.finalCandidates = 0;
    this.sentCandidates = 0;
    this.receivedCandidates = 0;
    this.remotePhase = null;
    this.iceChecks = 0;
    var self = this;
    self.pc.onicecandidate = function (event) {
      if (event.candidate === null) {
        self.finalCandidates++;
      } else {
        self.localCandidates.push(event.candidate.toJSON());
      }
    };
    self.pc.addEventListener('iceconnectionstatechange', function () {
      if (self.pc.iceConnectionState === 'checking') self.iceChecks++;
    });
  }

  Peer.prototype.signal = async function (description, phase) {
    var self = this;
    var finalBefore = self.finalCandidates;
    self.localCandidates = [];
    await self.pc.setLocalDescription(description);
    await until(function () {
      return self.native
        ? self.finalCandidates > finalBefore
        : self.pc.iceGatheringState === 'complete';
    }, 'local candidate gathering did not finish', function () { self.channel.check(); });
    if (!self.native) {
      self.localCandidates = sdpCandidates(self.pc.localDescription);
    }
    assert(self.localCandidates.length > 0, 'missing local host candidates');
    await self.channel.send({
      kind: 'description',
      phase: phase,
      description: withoutCandidates(self.pc.localDescription)
    });
    for (var candidate of self.localCandidates) {
      assert(candidate.sdpMid !== null && candidate.usernameFragment,
        'candidate lacks MID or ICE generation');
      var parsed = new RTCIceCandidate(candidate);
      assert(parsed.protocol === 'udp' && parsed.type === 'host', 'expected host UDP');
      assert(parsed.address !== '0.0.0.0' && parsed.address !== '::',
        'wildcard candidate');
      await self.channel.send({ kind: 'candidate', phase: phase, candidate: candidate });
      self.sentCandidates++;
    }
    await self.channel.send({ kind: 'candidate', phase: phase, candidate: null });
  };

  Peer.prototype.offer = async function (phase) {
    await this.signal(await this.pc.createOffer(), phase);
  };

  Peer.prototype.handle = async function (message) {
    if (message.kind === 'description') {
      assert(!/a=candidate:|a=end-of-candidates/.test(message.description.sdp),
        'SDP bypassed the candidate signaling route');
      this.remotePhase = message.phase;
      await this.pc.setRemoteDescription(message.description);
      if (this.prepare) await this.prepare(this, message.description.type);
      if (message.description.type === 'offer') {
        await this.signal(await this.pc.createAnswer(), message.phase);
      }
      return true;
    }
    if (message.kind === 'candidate') {
      assert(message.phase === this.remotePhase, 'candidate negotiation phase mismatch');
      await this.pc.addIceCandidate(message.candidate);
      if (message.candidate === null) {
        assert(this.pc.signalingState === 'stable', 'candidate completion before stable SDP');
        await this.channel.send({ kind: 'settled', phase: message.phase });
      } else {
        this.lastRemoteCandidate = message.candidate;
        this.receivedCandidates++;
      }
      return true;
    }
    return false;
  };

  Peer.prototype.report = async function () {
    if (this.native) {
      return Object.assign({ native: true }, g.__chuzzWebrtcInspect(this.pc));
    }
    var stats = await this.pc.getStats();
    var result = {
      native: false,
      inboundPackets: 0,
      outboundPackets: 0,
      bytesReceived: 0,
      bytesSent: 0,
      dtlsConnected: false,
      rxCodec: null,
      txCodec: null
    };
    stats.forEach(function (row) {
      if (row.type === 'transport' && row.dtlsState === 'connected') {
        result.dtlsConnected = true;
      }
      if ((row.kind || row.mediaType) !== 'audio') return;
      var codec = stats.get(row.codecId);
      var name = codec ? codec.mimeType.toLowerCase().replace('audio/', '') : null;
      if (row.type === 'inbound-rtp') {
        result.inboundPackets += row.packetsReceived || 0;
        result.bytesReceived += row.bytesReceived || 0;
        if (row.packetsReceived > 0) result.rxCodec = name;
      } else if (row.type === 'outbound-rtp') {
        result.outboundPackets += row.packetsSent || 0;
        result.bytesSent += row.bytesSent || 0;
        if (row.packetsSent > 0) result.txCodec = name;
      }
    });
    return result;
  };

  function received(report) {
    return report.native ? report.samplesReceived : report.inboundPackets;
  }

  function transport(report, codec) {
    assert(report.dtlsConnected, 'DTLS is not authenticated and connected');
    assert(report.bytesSent > 0 && report.bytesReceived > 0, 'no bidirectional traffic');
    assert(report.txCodec === codec && report.rxCodec === codec,
      'actual RTP codec differs from ' + codec);
    if (report.native) {
      assert(report.udpSent > 0 && report.udpReceived > 0, 'no real UDP');
      assert(report.dtlsSent > 0 && report.dtlsReceived > 0, 'no real DTLS traffic');
      assert(report.dtlsVersion === 'DTLS1.2' && report.remoteFingerprint,
        'DTLS 1.2 certificate authentication unavailable');
      assert(report.samplesSent > 0 && report.samplesReceived > 0,
        'no bidirectional audio PCM');
      assert(report.codecErrors === 0, 'audio codec error');
    }
  }

  function analyser(context, stream) {
    var source = context.createMediaStreamSource(stream);
    var node = new AnalyserNode(context, {
      fftSize: 1024, minDecibels: -100, maxDecibels: 0, smoothingTimeConstant: 0
    });
    source.connect(node);
    return { source: source, node: node };
  }

  function rms(node) {
    var samples = new Float32Array(node.fftSize);
    node.getFloatTimeDomainData(samples);
    var energy = 0;
    for (var sample of samples) energy += sample * sample;
    return Math.sqrt(energy / samples.length);
  }

  function tone(node, context) {
    var bins = new Uint8Array(node.frequencyBinCount);
    node.getByteFrequencyData(bins);
    var peak = 1;
    for (var index = 2; index < bins.length; index++) {
      if (bins[index] > bins[peak]) peak = index;
    }
    return rms(node) > 0.03 && bins[peak] > 120
      && Math.abs(peak * context.sampleRate / node.fftSize - 440)
        < context.sampleRate / node.fftSize;
  }

  async function rejectsCandidate(pc, candidate) {
    try {
      await pc.addIceCandidate(candidate);
    } catch (error) {
      assert(error.name === 'OperationError', 'wrong stale candidate error: ' + error.name);
      return;
    }
    throw new Error('stale ICE candidate was accepted');
  }

  g.ChuzzEcho = {
    assert: assert,
    sleep: sleep,
    until: until,
    paint: paint,
    request: request,
    Channel: Channel,
    Peer: Peer,
    credentials: credentials,
    received: received,
    transport: transport,
    analyser: analyser,
    rms: rms,
    tone: tone,
    rejectsCandidate: rejectsCandidate
  };
})(globalThis);

