// UI fixtures use directly executable JavaScript syntax.
(function () {
  var running = false;

  function assert(value, message) {
    if (!value) throw new Error(message);
  }

  function status(id, text) {
    var element = document.getElementById(id);
    element.textContent = text;
    element.setAttribute('aria-label', text);
  }

  function origin(id) {
    var value = document.getElementById(id).value;
    var parsed = new URL(value);
    assert(parsed.protocol === 'https:', 'fixture backend must use HTTPS');
    assert(!parsed.username && !parsed.password, 'credentials do not belong in fixture URLs');
    assert(parsed.pathname === '/' && !parsed.search && !parsed.hash, 'provide a backend origin');
    return parsed.origin;
  }

  async function asset(base, version) {
    var response = await fetch(base + '/asset.json?wave=W03&version=' + encodeURIComponent(version));
    assert(response.ok, version + ' HTTPS status ' + response.status);
    var body = await response.json();
    assert(body.tls === version, 'HTTPS backend did not enforce ' + version);
    assert(body.marker === 'chuzz-w03-https', 'HTTPS asset marker mismatch');
    assert(body.payload === 'h'.repeat(65536), 'HTTPS records lost or changed asset bytes');
  }

  function session(base, version, resultId) {
    return new Promise(function (resolve, reject) {
      var socket = new WebSocket(base.replace(/^https:/, 'wss:') + '/session', ['chuzz.w03.v1']);
      var token = null;
      var sequence = 0;
      var acknowledgements = 0;
      var pending = false;
      var completed = false;
      var settled = false;
      var started = 0;
      var scheduled = null;
      var payload = 'q'.repeat(32768);
      var deadline = setTimeout(function () {
        fail(new Error(version + ' session deadline expired'));
      }, 30000);

      function cleanup() {
        clearTimeout(deadline);
        if (scheduled !== null) clearTimeout(scheduled);
      }

      function fail(error) {
        if (settled) return;
        settled = true;
        cleanup();
        try { socket.close(1000); } catch (_) {}
        status(resultId, 'FAIL: ' + error.message);
        reject(error);
      }

      function send() {
        try {
          assert(socket.readyState === WebSocket.OPEN, version + ' socket closed before heartbeat');
          assert(!pending, 'heartbeat response was not received');
          sequence += 1;
          pending = true;
          socket.send(JSON.stringify({
            type: 'heartbeat',
            session: token,
            sequence: sequence,
            payload: payload
          }));
        } catch (error) {
          fail(error);
        }
      }

      socket.onopen = function () {
        try {
          assert(socket.protocol === 'chuzz.w03.v1', 'backend subprotocol was not negotiated');
          status(resultId, version + ' waiting for backend session');
        } catch (error) {
          fail(error);
        }
      };

      socket.onmessage = function (event) {
        if (settled) return;
        try {
          var message = JSON.parse(event.data);
          if (message.type === 'hello') {
            assert(token === null, 'backend opened a second session on one connection');
            assert(message.tls === version, 'WSS backend did not enforce ' + version);
            assert(typeof message.session === 'string' && /^[0-9a-f]{32}$/.test(message.session),
              'backend session identity missing');
            token = message.session;
            started = Date.now();
            send();
            return;
          }
          assert(message.type === 'ack', 'unexpected backend message');
          assert(token !== null && message.session === token, 'response belongs to another session');
          assert(message.tls === version, 'backend TLS version changed');
          assert(pending && message.sequence === sequence, 'heartbeat response reordered or duplicated');
          assert(message.payload === payload, 'WSS records lost or changed payload bytes');
          assert(typeof message.elapsedMs === 'number' && message.elapsedMs >= 0,
            'backend session duration missing');
          pending = false;
          acknowledgements += 1;
          status(resultId, version + ' heartbeat ' + acknowledgements + ' received');
          if (acknowledgements === 6) {
            assert(Date.now() - started >= 14000, 'client did not keep the session alive');
            assert(message.elapsedMs >= 14000, 'backend did not keep the same session alive');
            completed = true;
            socket.close(1000);
          } else {
            scheduled = setTimeout(send, 3000);
          }
        } catch (error) {
          fail(error);
        }
      };

      socket.onerror = function () {
        fail(new Error(version + ' WSS transport failed'));
      };

      socket.onclose = function (event) {
        if (settled) return;
        try {
          assert(completed && acknowledgements === 6 && !pending, 'backend session closed early');
          assert(event.code === 1000, 'backend session did not close normally');
          settled = true;
          cleanup();
          status(resultId, 'READY: ' + version + ' WSS heartbeat PASS');
          resolve();
        } catch (error) {
          fail(error);
        }
      };
    });
  }

  document.getElementById('run').onclick = async function () {
    if (running) return;
    running = true;
    status('result', 'TLS running');
    try {
      var tls13 = origin('tls13-origin');
      var tls12 = origin('tls12-origin');
      await Promise.all([asset(tls13, 'TLS1.3'), asset(tls12, 'TLS1.2')]);
      status('https-result', 'READY: HTTPS assets PASS');
      await Promise.all([
        session(tls13, 'TLS1.3', 'tls13-result'),
        session(tls12, 'TLS1.2', 'tls12-result')
      ]);
      status('result', 'READY: TLS HTTPS WSS PASS');
    } catch (error) {
      status('result', 'FAIL: ' + error.message);
    } finally {
      running = false;
    }
  };
})();

