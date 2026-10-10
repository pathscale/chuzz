// JavaScript syntax is intentional: Boa evaluates this source directly.
(function (g) {
  'use strict';

  var peers = new Map();
  var states = new WeakMap();
  var transceivers = new WeakMap();
  var senders = new WeakMap();
  var receivers = new WeakMap();
  var listeners = new WeakMap();
  var secret = {};
  var nextToken = 0;
  var directions = ['sendrecv', 'sendonly', 'recvonly', 'inactive'];

  function copy(value) { return JSON.parse(JSON.stringify(value)); }
  function error(text) {
    text = String(text);
    var separator = text.indexOf(': ');
    var name = separator < 0 ? 'OperationError' : text.slice(0, separator);
    var message = separator < 0 ? text : text.slice(separator + 2);
    if (name === 'TypeError') return new TypeError(message);
    return new DOMException(message, name);
  }
  function native(op, fields) {
    var request = Object.assign({ op: op }, fields || {});
    var reply = JSON.parse(g.__chuzzPeerControl(JSON.stringify(request)));
    if (!reply.ok) throw error(reply.error);
    return reply.value;
  }
  function state(pc) {
    var value = states.get(pc);
    if (!value) throw new TypeError('expected an RTCPeerConnection');
    return value;
  }
  function open(pc) {
    var value = state(pc);
    if (value.closed) throw error('InvalidStateError: peer connection is closed');
    return value;
  }
  function trackHandle(track) {
    if (!(track instanceof g.MediaStreamTrack)) {
      throw new TypeError('expected a MediaStreamTrack');
    }
    if (track.kind !== 'audio') {
      throw error('NotSupportedError: video encoding is not enabled');
    }
    if (track.readyState !== 'live') {
      throw error('InvalidStateError: track has ended');
    }
    return g.__chuzzFakeMedia.handle(track);
  }

  function Target() { listeners.set(this, new Map()); }
  if (typeof g.EventTarget === 'function') {
    Object.setPrototypeOf(Target.prototype, g.EventTarget.prototype);
  }
  Target.prototype.addEventListener = function (type, callback, options) {
    if (callback == null) return;
    type = String(type);
    var capture = typeof options === 'boolean' ? options : !!(options && options.capture);
    var signal = options && options.signal;
    if (signal && signal.aborted) return;
    var entries = listeners.get(this).get(type) || [];
    if (entries.some(function (entry) {
      return entry.callback === callback && entry.capture === capture;
    })) return;
    entries.push({ callback: callback, capture: capture, once: !!(options && options.once) });
    listeners.get(this).set(type, entries);
    if (signal) {
      var self = this;
      signal.addEventListener('abort', function () {
        self.removeEventListener(type, callback, capture);
      }, { once: true });
    }
  };
  Target.prototype.removeEventListener = function (type, callback, options) {
    var capture = typeof options === 'boolean' ? options : !!(options && options.capture);
    var map = listeners.get(this);
    map.set(String(type), (map.get(String(type)) || []).filter(function (entry) {
      return entry.callback !== callback || entry.capture !== capture;
    }));
  };
  Target.prototype.dispatchEvent = function (event) {
    if (!event || !event.type) throw new TypeError('expected an Event');
    var self = this;
    var map = listeners.get(this);
    var stopped = false;
    var immediate = event.stopImmediatePropagation;
    Object.defineProperty(event, 'target', { configurable: true, value: self });
    Object.defineProperty(event, 'currentTarget', { configurable: true, value: self });
    event.stopImmediatePropagation = function () {
      stopped = true;
      if (typeof immediate === 'function') immediate.call(event);
    };
    function invoke(callback) {
      try {
        if (typeof callback === 'function') callback.call(self, event);
        else if (callback && typeof callback.handleEvent === 'function') callback.handleEvent(event);
      } catch (caught) {
        setTimeout(function () { throw caught; }, 0);
      }
    }
    (map.get(event.type) || []).slice().forEach(function (entry) {
      if (stopped || !(map.get(event.type) || []).includes(entry)) return;
      if (entry.once) self.removeEventListener(event.type, entry.callback, entry.capture);
      invoke(entry.callback);
    });
    if (!stopped) invoke(self['on' + event.type]);
    Object.defineProperty(event, 'currentTarget', { configurable: true, value: null });
    event.stopImmediatePropagation = immediate;
    return !event.defaultPrevented;
  };

  function later(pc, type, fields) {
    var value = state(pc);
    var generation = value.generation;
    setTimeout(function () {
      if (state(pc).generation !== generation) return;
      var event = new Event(type);
      Object.keys(fields || {}).forEach(function (key) {
        Object.defineProperty(event, key, { enumerable: true, value: fields[key] });
      });
      pc.dispatchEvent(event);
    }, 0);
  }
  function change(pc, field, value, event) {
    var data = state(pc);
    if (data[field] === value) return;
    data[field] = value;
    later(pc, event);
  }
  function negotiate(pc) {
    var data = open(pc);
    if (data.negotiationQueued) return;
    data.negotiationQueued = true;
    var generation = data.generation;
    setTimeout(function () {
      data.negotiationQueued = false;
      if (!data.closed && generation === data.generation && data.signalingState === 'stable') {
        pc.dispatchEvent(new Event('negotiationneeded'));
      }
    }, 0);
  }

  function RTCSessionDescription(init) {
    if (!init || !['offer', 'answer', 'pranswer', 'rollback'].includes(init.type)) {
      throw new TypeError('invalid session description type');
    }
    this.type = String(init.type);
    this.sdp = init.sdp == null ? '' : String(init.sdp);
  }
  RTCSessionDescription.prototype.toJSON = function () {
    return { type: this.type, sdp: this.sdp };
  };

  function RTCIceCandidate(init) {
    init = init || {};
    this.candidate = init.candidate == null ? '' : String(init.candidate);
    this.sdpMid = init.sdpMid == null ? null : String(init.sdpMid);
    this.sdpMLineIndex = init.sdpMLineIndex == null ? null : Number(init.sdpMLineIndex);
    if (this.sdpMLineIndex !== null
        && (!Number.isInteger(this.sdpMLineIndex) || this.sdpMLineIndex < 0)) {
      throw new TypeError('invalid candidate m-line index');
    }
    this.usernameFragment = init.usernameFragment == null ? null : String(init.usernameFragment);
    var fields = this.candidate.replace(/^a=/, '').split(/\s+/);
    this.foundation = fields[0] && fields[0].startsWith('candidate:')
      ? fields[0].slice(10) : null;
    this.component = fields[1] === '1' ? 'rtp' : fields[1] === '2' ? 'rtcp' : null;
    this.protocol = fields[2] ? fields[2].toLowerCase() : null;
    this.priority = fields[3] ? Number(fields[3]) : null;
    this.address = fields[4] || null;
    this.port = fields[5] ? Number(fields[5]) : null;
    this.type = fields[6] === 'typ' ? fields[7] : null;
    this.relatedAddress = null;
    this.relatedPort = null;
    this.tcpType = null;
    for (var index = 8; index + 1 < fields.length; index += 2) {
      if (fields[index] === 'raddr') this.relatedAddress = fields[index + 1];
      if (fields[index] === 'rport') this.relatedPort = Number(fields[index + 1]);
      if (fields[index] === 'tcptype') this.tcpType = fields[index + 1];
      if (fields[index] === 'ufrag' && this.usernameFragment === null) {
        this.usernameFragment = fields[index + 1];
      }
    }
  }
  RTCIceCandidate.prototype.toJSON = function () {
    return {
      candidate: this.candidate,
      sdpMid: this.sdpMid,
      sdpMLineIndex: this.sdpMLineIndex,
      usernameFragment: this.usernameFragment
    };
  };

  function capabilities(kind) {
    if (kind === 'video') return { codecs: [], headerExtensions: [] };
    if (kind !== 'audio') return null;
    return {
      codecs: [{ mimeType: 'audio/PCMU', clockRate: 8000, channels: 1 }],
      headerExtensions: []
    };
  }

  function RTCRtpSender(token, pc, slot, track) {
    if (token !== secret) throw new TypeError('Illegal constructor');
    senders.set(this, { pc: pc, slot: slot, track: track, active: true, transaction: 0 });
  }
  RTCRtpSender.getCapabilities = capabilities;
  Object.defineProperty(RTCRtpSender.prototype, 'track', {
    get: function () { return senders.get(this).track; }
  });
  RTCRtpSender.prototype.replaceTrack = function (track) {
    var self = this;
    var data = senders.get(self);
    if (!data) return Promise.reject(new TypeError('expected an RTCRtpSender'));
    return queued(data.pc, function () {
      var handle = track == null ? null : trackHandle(track);
      return request(data.pc, 'replace', { slot: data.slot, handle: handle }).then(function () {
        data.track = track == null ? null : track;
      });
    });
  };
  RTCRtpSender.prototype.getParameters = function () {
    var data = senders.get(this);
    return {
      transactionId: String(data.transaction),
      codecs: [{ payloadType: 0, mimeType: 'audio/PCMU', clockRate: 8000, channels: 1 }],
      headerExtensions: [],
      rtcp: { reducedSize: true },
      encodings: [{ active: data.active }]
    };
  };
  RTCRtpSender.prototype.setParameters = function (parameters) {
    var data = senders.get(this);
    var self = this;
    return queued(data.pc, function () {
      if (!parameters || parameters.transactionId !== String(data.transaction)
          || !Array.isArray(parameters.encodings) || parameters.encodings.length !== 1) {
        throw error('InvalidModificationError: parameters do not match the sender transaction');
      }
      var encoding = parameters.encodings[0];
      if (Object.keys(encoding).some(function (key) { return key !== 'active'; })) {
        throw error('NotSupportedError: bitrate, scaling and frame-rate budgets belong to W08');
      }
      var current = self.getParameters();
      ['codecs', 'headerExtensions', 'rtcp'].forEach(function (key) {
        if (JSON.stringify(parameters[key]) !== JSON.stringify(current[key])) {
          throw error('InvalidModificationError: negotiated sender parameters are immutable');
        }
      });
      var active = encoding.active === undefined ? true : Boolean(encoding.active);
      return request(data.pc, 'active', { slot: data.slot, active: active }).then(function () {
        data.active = active;
        data.transaction++;
      });
    });
  };

  function RTCRtpReceiver(token, pc, slot) {
    if (token !== secret) throw new TypeError('Illegal constructor');
    var metadata = native('receiver', { peer: open(pc).id });
    var stream = g.__chuzzMediaCreateAudioStream(metadata);
    var track = stream.getAudioTracks()[0];
    Object.defineProperty(track, 'label', { value: 'Remote WebRTC audio', enumerable: true });
    receivers.set(this, {
      pc: pc, slot: slot, track: track, audio: metadata.track,
      handle: g.__chuzzFakeMedia.handle(track), streams: [], announced: false
    });
  }
  RTCRtpReceiver.getCapabilities = capabilities;
  Object.defineProperty(RTCRtpReceiver.prototype, 'track', {
    get: function () { return receivers.get(this).track; }
  });
  RTCRtpReceiver.prototype.getParameters = function () {
    return {
      codecs: [{ payloadType: 0, mimeType: 'audio/PCMU', clockRate: 8000, channels: 1 }],
      headerExtensions: [], rtcp: { reducedSize: true }
    };
  };

  function RTCRtpTransceiver(token, pc, slot, direction, track) {
    if (token !== secret) throw new TypeError('Illegal constructor');
    var sender = new RTCRtpSender(secret, pc, slot, track || null);
    var receiver = new RTCRtpReceiver(secret, pc, slot);
    transceivers.set(this, {
      pc: pc, slot: slot, mid: null, direction: direction,
      currentDirection: null, sender: sender, receiver: receiver
    });
  }
  ['mid', 'currentDirection', 'sender', 'receiver'].forEach(function (key) {
    Object.defineProperty(RTCRtpTransceiver.prototype, key, {
      get: function () { return transceivers.get(this)[key]; }
    });
  });
  Object.defineProperty(RTCRtpTransceiver.prototype, 'direction', {
    get: function () { return transceivers.get(this).direction; },
    set: function (direction) {
      var data = transceivers.get(this);
      open(data.pc);
      if (!directions.includes(direction)) throw new TypeError('invalid direction');
      native('direction', { peer: state(data.pc).id, slot: data.slot, direction: direction });
      data.direction = direction;
      negotiate(data.pc);
    }
  });
  RTCRtpTransceiver.prototype.setCodecPreferences = function (codecs) {
    var data = transceivers.get(this);
    open(data.pc);
    codecs = Array.from(codecs);
    if (codecs.length > 1 || codecs.some(function (codec) {
      return codec.mimeType.toLowerCase() !== 'audio/pcmu'
        || codec.clockRate !== 8000 || (codec.channels !== undefined && codec.channels !== 1);
    })) {
      throw error('NotSupportedError: only synchronous PCMU is enabled');
    }
  };

  function RTCPeerConnection(configuration) {
    if (!(this instanceof RTCPeerConnection)) throw new TypeError('RTCPeerConnection requires new');
    Target.call(this);
    configuration = configuration == null ? {} : copy(configuration);
    var id = native('create', { configuration: configuration });
    states.set(this, {
      id: id, configuration: configuration, closed: false, generation: 1,
      operations: Promise.resolve(), pending: new Map(), slots: [], nextSlot: 0,
      negotiationQueued: false, signalingState: 'stable',
      connectionState: 'new', iceConnectionState: 'new', iceGatheringState: 'new',
      localDescription: null, remoteDescription: null,
      pendingLocalDescription: null, pendingRemoteDescription: null,
      currentLocalDescription: null, currentRemoteDescription: null
    });
    peers.set(id, this);
    [
      'track', 'icecandidate', 'icecandidateerror', 'negotiationneeded',
      'signalingstatechange', 'iceconnectionstatechange',
      'icegatheringstatechange', 'connectionstatechange'
    ].forEach(function (name) { this['on' + name] = null; }, this);
  }
  RTCPeerConnection.prototype = Object.create(Target.prototype);
  RTCPeerConnection.prototype.constructor = RTCPeerConnection;

  [
    'signalingState', 'connectionState', 'iceConnectionState', 'iceGatheringState',
    'localDescription', 'remoteDescription', 'pendingLocalDescription',
    'pendingRemoteDescription', 'currentLocalDescription', 'currentRemoteDescription'
  ].forEach(function (key) {
    Object.defineProperty(RTCPeerConnection.prototype, key, {
      enumerable: true, get: function () { return state(this)[key]; }
    });
  });

  function request(pc, op, fields) {
    var data = open(pc);
    if (data.pending.size >= 64) {
      return Promise.reject(error('OperationError: too many pending peer operations'));
    }
    var token = ++nextToken;
    if (!Number.isSafeInteger(token)) {
      return Promise.reject(error('OperationError: peer operation identifiers exhausted'));
    }
    return new Promise(function (resolve, reject) {
      data.pending.set(token, { resolve: resolve, reject: reject });
      try {
        native(op, Object.assign({ peer: data.id, token: token }, fields || {}));
      } catch (caught) {
        data.pending.delete(token);
        reject(caught);
      }
    });
  }
  function queued(pc, operation) {
    var data = state(pc);
    var result = data.operations.then(function () {
      open(pc);
      return operation();
    });
    data.operations = result.catch(function () {});
    return result;
  }
  function receiverFields(transceiver) {
    var data = transceivers.get(transceiver);
    var receiver = receivers.get(data.receiver);
    return {
      peer: state(data.pc).id, slot: data.slot,
      audio: receiver.audio, receiverHandle: receiver.handle
    };
  }

  RTCPeerConnection.prototype.addTransceiver = function (kindOrTrack, init) {
    var data = open(this);
    init = init || {};
    var track = typeof kindOrTrack === 'string' ? null : kindOrTrack;
    var kind = track ? track.kind : kindOrTrack;
    if (kind !== 'audio') {
      throw error('NotSupportedError: video encoding is not enabled');
    }
    var handle = track ? trackHandle(track) : null;
    var direction = init.direction === undefined ? 'sendrecv' : init.direction;
    if (!directions.includes(direction)) throw new TypeError('invalid direction');
    var streams = init.streams === undefined ? [] : Array.from(init.streams);
    if (streams.some(function (stream) { return !(stream instanceof g.MediaStream); })) {
      throw new TypeError('expected MediaStream entries');
    }
    if (streams.length > 1) {
      throw error('NotSupportedError: one outgoing stream per transceiver is supported');
    }
    if (init.sendEncodings !== undefined && init.sendEncodings.length !== 0) {
      throw error('NotSupportedError: custom encodings are not enabled in W07');
    }
    if (data.slots.length >= 32) throw error('OperationError: transceiver limit reached');
    var slot = data.nextSlot + 1;
    var transceiver = new RTCRtpTransceiver(secret, this, slot, direction, track);
    try {
      var updates = native('add', Object.assign(receiverFields(transceiver), {
        direction: direction, handle: handle, stream: streams.length ? streams[0].id : null
      }));
      g.__chuzzMediaUpdate(updates);
    } catch (caught) {
      transceiver.receiver.track.stop();
      throw caught;
    }
    data.nextSlot = slot;
    data.slots.push(transceiver);
    negotiate(this);
    return transceiver;
  };
  RTCPeerConnection.prototype.addTrack = function (track) {
    var data = open(this);
    trackHandle(track);
    if (data.slots.some(function (transceiver) { return transceiver.sender.track === track; })) {
      throw error('InvalidAccessError: track is already attached to this peer');
    }
    return this.addTransceiver(track, {
      direction: 'sendrecv', streams: Array.prototype.slice.call(arguments, 1)
    }).sender;
  };
  RTCPeerConnection.prototype.removeTrack = function (sender) {
    var data = open(this);
    var transceiver = data.slots.find(function (transceiver) { return transceiver.sender === sender; });
    if (!transceiver) throw error('InvalidAccessError: sender belongs to another peer');
    var senderData = senders.get(sender);
    native('replace', { peer: data.id, slot: senderData.slot, handle: null });
    senderData.track = null;
    transceiver.direction = transceiver.direction === 'sendrecv' ? 'recvonly' : 'inactive';
  };
  RTCPeerConnection.prototype.getTransceivers = function () { return state(this).slots.slice(); };
  RTCPeerConnection.prototype.getSenders = function () {
    return state(this).slots.map(function (transceiver) { return transceiver.sender; });
  };
  RTCPeerConnection.prototype.getReceivers = function () {
    return state(this).slots.map(function (transceiver) { return transceiver.receiver; });
  };
  RTCPeerConnection.prototype.getConfiguration = function () { return copy(state(this).configuration); };

  RTCPeerConnection.prototype.createOffer = function (options) {
    var pc = this;
    return queued(pc, function () {
      if (options && options.iceRestart) native('restart', { peer: open(pc).id });
      return request(pc, 'createOffer').then(function (value) {
        return new RTCSessionDescription(value.description);
      });
    });
  };
  RTCPeerConnection.prototype.createAnswer = function () {
    var pc = this;
    return queued(pc, function () {
      return request(pc, 'createAnswer').then(function (value) {
        return new RTCSessionDescription(value.description);
      });
    });
  };
  RTCPeerConnection.prototype.setLocalDescription = function (init) {
    var pc = this;
    return queued(pc, function () {
      if (init === undefined) {
        var op = pc.signalingState === 'have-remote-offer' ? 'createAnswer' : 'createOffer';
        return request(pc, op).then(function (value) {
          return request(pc, 'setLocal', { description: value.description });
        }).then(function () {});
      }
      var description = new RTCSessionDescription(init);
      return request(pc, 'setLocal', { description: description.toJSON() }).then(function () {});
    });
  };
  RTCPeerConnection.prototype.setRemoteDescription = function (init) {
    var pc = this;
    return queued(pc, function () {
      var description = new RTCSessionDescription(init);
      return request(pc, 'setRemote', { description: description.toJSON() }).then(function () {});
    });
  };
  RTCPeerConnection.prototype.addIceCandidate = function (init) {
    var pc = this;
    return queued(pc, function () {
      var candidate = init == null ? null : new RTCIceCandidate(init).toJSON();
      return request(pc, 'candidate', { candidate: candidate }).then(function () {});
    });
  };
  RTCPeerConnection.prototype.restartIce = function () {
    var data = open(this);
    native('restart', { peer: data.id });
    negotiate(this);
  };
  RTCPeerConnection.prototype.getStats = function () {
    var pc = this;
    return queued(pc, function () {
      var data = open(pc);
      var measured = native('inspect', { peer: data.id });
      var id = 'chuzz-transport-' + data.id;
      return new Map([[id, {
        id: id, type: 'transport', timestamp: Date.now(),
        bytesSent: measured.bytesSent, bytesReceived: measured.bytesReceived,
        dtlsState: measured.dtlsConnected ? 'connected' : 'connecting',
        iceState: data.iceConnectionState
      }]]);
    });
  };

  function shutdown(pc, failed) {
    var data = state(pc);
    if (data.closed) return;
    data.closed = true;
    data.generation++;
    native('close', { peer: data.id });
    data.pending.forEach(function (pending) {
      pending.reject(error('AbortError: peer connection closed'));
    });
    data.pending.clear();
    data.slots.forEach(function (transceiver) {
      transceivers.get(transceiver).currentDirection = null;
      transceiver.receiver.track.stop();
    });
    data.pendingLocalDescription = null;
    data.pendingRemoteDescription = null;
    data.localDescription = data.currentLocalDescription;
    data.remoteDescription = data.currentRemoteDescription;
    change(pc, 'signalingState', 'closed', 'signalingstatechange');
    change(pc, 'iceConnectionState', failed ? 'failed' : 'closed', 'iceconnectionstatechange');
    change(pc, 'connectionState', failed ? 'failed' : 'closed', 'connectionstatechange');
    peers.delete(data.id);
  }
  RTCPeerConnection.prototype.close = function () { shutdown(this, false); };

  function snapshot(pc, value) {
    var data = open(pc);
    [
      'localDescription', 'remoteDescription', 'pendingLocalDescription',
      'pendingRemoteDescription', 'currentLocalDescription', 'currentRemoteDescription'
    ].forEach(function (key) {
      data[key] = value[key] ? Object.freeze(new RTCSessionDescription(value[key])) : null;
    });
    change(pc, 'signalingState', value.signalingState, 'signalingstatechange');
    value.slots.forEach(function (entry) {
      var transceiver = data.slots.find(function (transceiver) {
        return transceivers.get(transceiver).slot === entry.slot;
      });
      if (!transceiver) {
        transceiver = new RTCRtpTransceiver(secret, pc, entry.slot, entry.direction, null);
        var updates = native('bind', receiverFields(transceiver));
        g.__chuzzMediaUpdate(updates);
        data.slots.push(transceiver);
        data.nextSlot = Math.max(data.nextSlot, entry.slot);
      }
      var local = transceivers.get(transceiver);
      local.mid = entry.mid;
      local.currentDirection = entry.currentDirection;
      var receiver = receivers.get(local.receiver);
      if (entry.remote && !receiver.announced) {
        receiver.announced = true;
        if (entry.remote.track) {
          Object.defineProperty(receiver.track, 'id', {
            value: entry.remote.track, enumerable: true, configurable: true
          });
        }
        receiver.streams = [];
        if (entry.remote.stream) {
          var stream = new g.MediaStream([receiver.track]);
          Object.defineProperty(stream, 'id', { value: entry.remote.stream, enumerable: true });
          receiver.streams.push(stream);
        }
        later(pc, 'track', {
          receiver: local.receiver, transceiver: transceiver,
          track: receiver.track, streams: receiver.streams.slice()
        });
      }
    });
  }

  Object.defineProperty(g, '__chuzzPeerEvents', {
    value: function (entries) {
      entries.forEach(function (entry) {
        var pc = peers.get(entry.peer);
        if (!pc) return;
        var data = state(pc);
        var event = entry.event;
        try {
          if (event.type === 'reply') {
            var pending = data.pending.get(event.token);
            if (!pending) return;
            data.pending.delete(event.token);
            if (event.error) {
              pending.reject(error(event.error));
            } else {
              try {
                if (event.value && event.value.snapshot) snapshot(pc, event.value.snapshot);
                pending.resolve(event.value);
              } catch (caught) {
                pending.reject(caught);
                shutdown(pc, true);
              }
            }
          } else if (event.type === 'snapshot') {
            snapshot(pc, event.snapshot);
          } else if (event.type === 'candidate') {
            later(pc, 'icecandidate', {
              candidate: event.candidate === null ? null : new RTCIceCandidate(event.candidate)
            });
          } else if (event.type === 'gathering') {
            change(pc, 'iceGatheringState', event.state, 'icegatheringstatechange');
          } else if (event.type === 'ice') {
            change(pc, 'iceConnectionState', event.state, 'iceconnectionstatechange');
          } else if (event.type === 'connection') {
            change(pc, 'connectionState', event.state, 'connectionstatechange');
          } else if (event.type === 'fatal') {
            data.pending.forEach(function (pending) { pending.reject(error(event.error)); });
            data.pending.clear();
            shutdown(pc, true);
          }
        } catch (caught) {
          shutdown(pc, true);
          setTimeout(function () { throw caught; }, 0);
        }
      });
    }
  });
  Object.defineProperty(g, '__chuzzWebrtcInspect', {
    value: function (pc) { return native('inspect', { peer: state(pc).id }); }
  });

  g.RTCSessionDescription = RTCSessionDescription;
  g.RTCIceCandidate = RTCIceCandidate;
  g.RTCRtpSender = RTCRtpSender;
  g.RTCRtpReceiver = RTCRtpReceiver;
  g.RTCRtpTransceiver = RTCRtpTransceiver;
  g.RTCPeerConnection = RTCPeerConnection;
})(globalThis);

