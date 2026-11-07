use std::sync::Arc;

use blitz_script_ekojit::{EkojitDocument, PageSource};

const SOURCE: &str = r#"(function (g) {
  'use strict';
  var broadcasts = Object.create(null), ports = Object.create(null), locks = Object.create(null);
  var nextLock = 1;
  function call(op) {
    var result = JSON.parse(__chuzzPlatformCall(JSON.stringify(op)));
    return result;
  }
  function fail(name, message) { var error = new Error(message); error.name = name; throw error; }
  function encode(root) {
    var refs = [], nodes = [], budget = 0;
    function visit(value) {
      budget += 1;
      if (budget > 16 * 1024 * 1024) fail('DataCloneError', 'Structured clone limit exceeded');
      if (value === null) return ['null'];
      var type = typeof value;
      if (type === 'undefined') return ['undefined'];
      if (type === 'boolean') return ['boolean', value];
      if (type === 'string') return ['string', value];
      if (type === 'number') return Number.isFinite(value) ? ['number', value] : ['number-special', String(value)];
      if (type !== 'object') fail('DataCloneError', 'Value cannot be cloned');
      var index = refs.indexOf(value);
      if (index !== -1) return ['ref', index];
      if (refs.length >= 100000) fail('DataCloneError', 'Structured clone node limit exceeded');
      index = refs.length; refs.push(value); nodes.push(null);
      var node;
      if (Array.isArray(value)) {
        node = ['array', value.length, []];
        for (var i = 0; i < value.length; i++) {
          if (Object.prototype.hasOwnProperty.call(value, i)) node[2].push([i, visit(value[i])]);
        }
      } else if (value instanceof Map) {
        node = ['map', []]; value.forEach(function (v, k) { node[1].push([visit(k), visit(v)]); });
      } else if (value instanceof Set) {
        node = ['set', []]; value.forEach(function (v) { node[1].push(visit(v)); });
      } else if (value instanceof ArrayBuffer) {
        node = ['buffer', Array.from(new Uint8Array(value))];
      } else if (ArrayBuffer.isView(value)) {
        var viewTypes = [[Uint8Array,'Uint8Array'],[Uint8ClampedArray,'Uint8ClampedArray'],[Int8Array,'Int8Array'],[Uint16Array,'Uint16Array'],[Int16Array,'Int16Array'],[Uint32Array,'Uint32Array'],[Int32Array,'Int32Array'],[Float32Array,'Float32Array'],[Float64Array,'Float64Array'],[DataView,'DataView']];
        var viewName = null;
        for (var brand = 0; brand < viewTypes.length; brand++) {
          if (value instanceof viewTypes[brand][0]) { viewName = viewTypes[brand][1]; break; }
        }
        if (viewName === null) fail('DataCloneError', 'Unsupported typed view');
        node = ['view', viewName, visit(value.buffer), value.byteOffset, value.byteLength];
      } else {
        var proto = Object.getPrototypeOf(value);
        if (proto !== Object.prototype && proto !== null) fail('DataCloneError', 'Unsupported structured clone object');
        node = ['object', []];
        Object.keys(value).forEach(function (key) {
          budget += key.length;
          if (budget > 16 * 1024 * 1024) fail('DataCloneError', 'Structured clone limit exceeded');
          node[1].push([key, visit(value[key])]);
        });
      }
      nodes[index] = node; return ['ref', index];
    }
    var top = visit(root), wire = JSON.stringify({ v: 1, top: top, nodes: nodes });
    if (wire.length > 16 * 1024 * 1024) fail('DataCloneError', 'Structured clone limit exceeded');
    return wire;
  }
  function decode(wire) {
    var graph = JSON.parse(wire);
    if (!graph || graph.v !== 1 || !Array.isArray(graph.nodes) || graph.nodes.length > 100000) fail('DataCloneError', 'Invalid structured clone graph');
    var values = new Array(graph.nodes.length);
    graph.nodes.forEach(function (node, i) {
      if (!Array.isArray(node)) fail('DataCloneError', 'Invalid structured clone node');
      if (node[0] === 'array') {
        if (!Number.isSafeInteger(node[1]) || node[1] < 0 || node[1] > 10000000 || !Array.isArray(node[2])) fail('DataCloneError', 'Invalid sparse array node');
        values[i] = new Array(node[1]);
      } else if (node[0] === 'object') values[i] = {};
      else if (node[0] === 'map') values[i] = new Map();
      else if (node[0] === 'set') values[i] = new Set();
      else if (node[0] === 'buffer') {
        if (!Array.isArray(node[1]) || node[1].length > 16777216) fail('DataCloneError', 'Invalid buffer node');
        values[i] = new Uint8Array(node[1]).buffer;
      } else if (node[0] !== 'view') fail('DataCloneError', 'Unknown structured clone node');
    });
    var constructors = { Uint8Array: Uint8Array, Uint8ClampedArray: Uint8ClampedArray, Int8Array: Int8Array, Uint16Array: Uint16Array, Int16Array: Int16Array, Uint32Array: Uint32Array, Int32Array: Int32Array, Float32Array: Float32Array, Float64Array: Float64Array, DataView: DataView };
    graph.nodes.forEach(function (node, i) {
      if (node[0] !== 'view') return;
      var C = constructors[node[1]], ref = node[2];
      if (!C || !Array.isArray(ref) || ref[0] !== 'ref' || !Number.isInteger(ref[1]) || ref[1] < 0 || ref[1] >= values.length || !(values[ref[1]] instanceof ArrayBuffer)) fail('DataCloneError', 'Invalid typed view buffer');
      var buffer = values[ref[1]], offset = node[3], length = node[4];
      if (!Number.isSafeInteger(offset) || !Number.isSafeInteger(length) || offset < 0 || length < 0 || offset + length > buffer.byteLength) fail('DataCloneError', 'Invalid typed view bounds');
      try { values[i] = C === DataView ? new DataView(buffer, offset, length) : new C(buffer, offset, length / C.BYTES_PER_ELEMENT); }
      catch (_) { fail('DataCloneError', 'Invalid typed view alignment'); }
    });
    function read(ref) {
      if (!Array.isArray(ref)) fail('DataCloneError', 'Invalid structured clone reference');
      if (ref[0] === 'ref') { if (!Number.isInteger(ref[1]) || !values[ref[1]]) fail('DataCloneError', 'Invalid structured clone reference'); return values[ref[1]]; }
      if (ref[0] === 'null') return null;
      if (ref[0] === 'undefined') return undefined;
      if (ref[0] === 'boolean' || ref[0] === 'string' || ref[0] === 'number') return ref[1];
      if (ref[0] === 'number-special') return ref[1] === 'NaN' ? NaN : ref[1] === 'Infinity' ? Infinity : -Infinity;
      fail('DataCloneError', 'Invalid structured clone primitive');
    }
    graph.nodes.forEach(function (node, i) {
      var target = values[i];
      if (node[0] === 'array') {
        var seen = Object.create(null);
        node[2].forEach(function (pair) {
          if (!Array.isArray(pair) || !Number.isSafeInteger(pair[0]) || pair[0] < 0 || pair[0] >= node[1] || seen[pair[0]]) fail('DataCloneError', 'Invalid sparse array entry');
          seen[pair[0]] = true;
          Object.defineProperty(target, pair[0], { value: read(pair[1]), enumerable: true, writable: true, configurable: true });
        });
      }
      else if (node[0] === 'object') node[1].forEach(function (pair) { Object.defineProperty(target, pair[0], { value: read(pair[1]), enumerable: true, writable: true, configurable: true }); });
      else if (node[0] === 'map') node[1].forEach(function (pair) { target.set(read(pair[0]), read(pair[1])); });
      else if (node[0] === 'set') node[1].forEach(function (v) { target.add(read(v)); });
    });
    return read(graph.top);
  }
  function eventFor(target, data) { return { type: 'message', data: data, target: target, currentTarget: target, origin: g.location && g.location.origin || 'null', ports: [] }; }
  function dispatch(target, event) {
    var list = (target._listeners.message || []).slice(), firstError = null;
    list.forEach(function (fn) {
      try { if (typeof fn === 'function') fn.call(target, event); else if (fn && typeof fn.handleEvent === 'function') fn.handleEvent(event); }
      catch (error) { if (!firstError) firstError = error; }
    });
    try { if (typeof target.onmessage === 'function') target.onmessage.call(target, event); }
    catch (error) { if (!firstError) firstError = error; }
    if (firstError) Promise.resolve().then(function () { throw firstError; });
  }
  function add(target, type, callback) { if (typeof callback !== 'function' && !(callback && typeof callback.handleEvent === 'function')) return; (target._listeners[type] || (target._listeners[type] = [])).push(callback); }
  function remove(target, type, callback) { var list = target._listeners[type] || []; target._listeners[type] = list.filter(function (item) { return item !== callback; }); }
  function BroadcastChannel(name) {
    if (!new.target) fail('TypeError', "Failed to construct 'BroadcastChannel': use new");
    this.name = String(name); this.onmessage = null; this._listeners = Object.create(null); this._closed = false;
    this._id = Number(call({ op: 'broadcast-open', name: this.name })); broadcasts[this._id] = this;
  }
  BroadcastChannel.prototype.postMessage = function (message, transfer) {
    if (this._closed) fail('InvalidStateError', 'BroadcastChannel is closed');
    if (transfer != null && (!Array.isArray(transfer) || transfer.length)) fail('DataCloneError', 'Transfer ownership is not supported');
    call({ op: 'broadcast-post', id: this._id, wire: encode(message) });
  };
  BroadcastChannel.prototype.close = function () { if (this._closed) return; this._closed = true; delete broadcasts[this._id]; call({ op: 'broadcast-close', id: this._id }); };
  BroadcastChannel.prototype.addEventListener = function (type, callback) { if (type === 'message') add(this, type, callback); };
  BroadcastChannel.prototype.removeEventListener = function (type, callback) { if (type === 'message') remove(this, type, callback); };
  function MessagePort(id) { this._id = id; this._closed = false; this._started = false; this.onmessage = null; this._listeners = Object.create(null); ports[id] = this; }
  Object.defineProperty(MessagePort.prototype, 'onmessage', {
    get: function () { return this._onmessage || null; },
    set: function (callback) { this._onmessage = callback; if (typeof callback === 'function') this.start(); }
  });
  MessagePort.prototype.postMessage = function (message, transfer) {
    if (this._closed) fail('InvalidStateError', 'MessagePort is closed');
    if (transfer != null && (!Array.isArray(transfer) || transfer.length)) fail('DataCloneError', 'Transfer ownership is not supported');
    call({ op: 'port-post', id: this._id, wire: encode(message) });
  };
  MessagePort.prototype.start = function () { if (this._closed) fail('InvalidStateError', 'MessagePort is closed'); this._started = true; call({ op: 'port-start', id: this._id }); };
  MessagePort.prototype.close = function () { if (this._closed) return; this._closed = true; delete ports[this._id]; call({ op: 'port-close', id: this._id }); };
  MessagePort.prototype.addEventListener = function (type, callback) { if (type === 'message') add(this, type, callback); };
  MessagePort.prototype.removeEventListener = function (type, callback) { if (type === 'message') remove(this, type, callback); };
  function MessageChannel() { if (!new.target) fail('TypeError', "Failed to construct 'MessageChannel': use new"); var id = Number(call({ op: 'port-pair' })); this.port1 = new MessagePort(id); this.port2 = new MessagePort(id + 1); }
  g.BroadcastChannel = BroadcastChannel; g.MessagePort = MessagePort; g.MessageChannel = MessageChannel;
  if (!g.navigator) g.navigator = {};
  if (!g.navigator.locks) g.navigator.locks = {};
  g.navigator.locks.request = function (name, options, callback) {
    if (typeof options === 'function') { callback = options; options = {}; }
    options = options || {};
    if (typeof callback !== 'function') return Promise.reject(new TypeError('Lock callback must be a function'));
    var mode = options.mode || 'exclusive';
    if (mode !== 'exclusive' && mode !== 'shared') return Promise.reject(new TypeError('Invalid lock mode'));
    var signal = options.signal;
    if (signal && signal.aborted) return Promise.reject(signal.reason || new Error('Lock request aborted'));
    var id = nextLock++;
    var request;
    try { request = call({ op: 'lock-request', name: String(name), shared: mode === 'shared', available: !!options.ifAvailable, steal: !!options.steal }); }
    catch (error) { return Promise.reject(error); }
    if (request === null) return Promise.resolve().then(function () { return callback(null); });
    id = Number(request);
    return new Promise(function (resolve, reject) {
      var entry = { id: id, name: String(name), mode: mode, callback: callback, resolve: resolve, reject: reject, started: false, done: false, signal: signal, abort: null };
      locks[id] = entry;
      if (signal && typeof signal.addEventListener === 'function') {
        entry.abort = function () {
          if (entry.started || entry.done) return;
          if (Number(call({ op: 'lock-state', id: id })) === 1) return;
          try { call({ op: 'lock-cancel', id: id }); } catch (_) { }
          entry.done = true; delete locks[id]; reject(signal.reason || new Error('Lock request aborted'));
        };
        signal.addEventListener('abort', entry.abort, { once: true });
      }
    });
  };
  g.navigator.locks.query = function () { return Promise.resolve(call({ op: 'lock-query' })); };
  function pump() {
    var items = call({ op: 'broadcast-read' });
    items.forEach(function (item) { var channel = broadcasts[item.id]; if (channel && !channel._closed) dispatch(channel, eventFor(channel, decode(item.wire))); });
    items = call({ op: 'port-read' });
    items.forEach(function (item) { var port = ports[item.id]; if (port && !port._closed) dispatch(port, eventFor(port, decode(item.wire))); });
    Object.keys(locks).forEach(function (key) {
      var entry = locks[key]; if (!entry || entry.done) return;
      var state = Number(call({ op: 'lock-state', id: entry.id }));
      if (state === 2 || state === 3) { entry.done = true; delete locks[key]; if (entry.signal && entry.abort) entry.signal.removeEventListener('abort', entry.abort); if (state === 2) { call({ op: 'lock-release', id: entry.id }); entry.reject(new Error('Lock request was stolen')); } return; }
      if (state !== 1 || entry.started) return;
      entry.started = true; if (entry.signal && entry.abort) entry.signal.removeEventListener('abort', entry.abort);
      var lock = { name: entry.name, mode: entry.mode };
      Promise.resolve().then(function () { return entry.callback(lock); }).then(function (value) {
        if (entry.done) return; entry.done = true; call({ op: 'lock-release', id: entry.id }); delete locks[key]; entry.resolve(value);
      }, function (error) {
        if (entry.done) return; entry.done = true; call({ op: 'lock-release', id: entry.id }); delete locks[key]; entry.reject(error);
      });
    });
  }
  __chuzzChannelsRegisterPump(pump);
})(globalThis);"#;

pub(super) fn install(document: &mut EkojitDocument) -> Result<(), String> {
    document
        .eval_source(Arc::new(PageSource {
            text: Arc::from(SOURCE),
            url: Arc::from("chuzz:webapi-channels"),
        }))
        .map(|_| ())
        .map_err(|error| format!("Ekojit guest channel installation failed: {error}"))
}

#[cfg(test)]
mod tests {
    use crate::page_document::PageDocument;
    use crate::script_host::HostInstallContext;
    use blitz_dom::{Document, DocumentConfig};
    use std::sync::Arc;

    fn page(origin: &str) -> PageDocument {
        crate::install_crypto_provider();
        let base = url::Url::parse(origin).unwrap();
        let provider = crate::document_loader::NetProvider::new(None);
        let initiator = provider.browser_initiator(base.clone());
        let net = Arc::new(provider.for_document(base.clone(), &initiator));
        PageDocument::from_html(
            Arc::from("<!doctype html><html><body></body></html>"),
            DocumentConfig {
                base_url: Some(origin.into()),
                ..Default::default()
            },
            HostInstallContext {
                net,
                base: Some(base),
                deadline: std::time::Duration::from_secs(2),
                wake: crate::net_bridge::PageWake::default(),
                lifetime: blitz_dom::release::Teardown::new(),
                signal: blitz_traits::net::AbortSignal::default(),
                session_backing: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn guest_channels_deliver_cyclic_copies_with_origin_isolation() {
        let mut a = page("https://channels.invalid/");
        let mut b = page("https://channels.invalid/");
        let mut isolated = page("https://other-channels.invalid/");
        a.eval("globalThis.received=[];globalThis.channel=new BroadcastChannel('w37');channel.onmessage=e=>received.push(e.data);").unwrap();
        b.eval("globalThis.received=[];globalThis.channel=new BroadcastChannel('w37');channel.onmessage=e=>received.push(e.data);").unwrap();
        isolated.eval("globalThis.received=[];globalThis.channel=new BroadcastChannel('w37');channel.onmessage=e=>received.push(e.data);").unwrap();
        a.eval("const value={n:1};value.self=value;channel.postMessage(value);value.n=9;")
            .unwrap();
        for _ in 0..4 {
            a.poll(None);
            b.poll(None);
            isolated.poll(None);
        }
        assert_eq!(
            a.eval_json("received.length").unwrap(),
            serde_json::json!(0)
        );
        assert_eq!(
            isolated.eval_json("received.length").unwrap(),
            serde_json::json!(0)
        );
        assert_eq!(
            b.eval_json("received.length===1&&received[0].n===1&&received[0].self===received[0]")
                .unwrap(),
            serde_json::json!(true)
        );
        a.eval("globalThis.portValue=null;const pair=new MessageChannel();pair.port2.onmessage=e=>{portValue=e.data;};const item={n:3};item.self=item;pair.port1.postMessage(item);item.n=8;").unwrap();
        for _ in 0..4 {
            a.poll(None);
        }
        assert_eq!(
            a.eval_json("portValue.n===3&&portValue.self===portValue")
                .unwrap(),
            serde_json::json!(true)
        );
    }

    #[test]
    fn guest_locks_wait_for_callbacks_release_and_park_when_idle() {
        let mut a = page("https://locks-guest.invalid/");
        a.eval("globalThis.order=[];navigator.locks.request('w37',()=>{order.push(1);return 7;}).then(v=>order.push(v));navigator.locks.request('w37',()=>order.push(2));").unwrap();
        for _ in 0..12 {
            a.poll(None);
        }
        assert_eq!(
            a.eval_json("order.join(',')").unwrap(),
            serde_json::json!("1,7,2")
        );
        assert!(!a.poll(None), "idle channel pump reported work");
    }
}
