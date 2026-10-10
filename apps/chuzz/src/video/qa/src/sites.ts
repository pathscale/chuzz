// UI fixtures use directly executable JavaScript syntax.
(function () {
  function assert(value, message) {
    if (!value) throw new Error(message);
  }

  document.cookie = 'qa_age=; Max-Age=0; Path=/';
  document.getElementById('gate').addEventListener('click', function (event) {
    try {
      var button = event.target.closest('#accept');
      if (!button) return;
      assert(button.matches('button'), 'matches did not find the consent button');
      assert(typeof Object.getOwnPropertyDescriptor(Element.prototype, 'closest').value === 'function',
        'Element prototype has no closest descriptor');
      document.cookie = 'qa_age=18; Path=/; SameSite=Lax';
      assert(document.cookie.split(';').some(function (cookie) {
        return cookie.trim() === 'qa_age=18';
      }), 'cookie read after write was stale');
      document.getElementById('gate').remove();
      document.getElementById('gate-result').textContent = 'READY: consent click cookie PASS';
    } catch (error) {
      document.getElementById('gate-result').textContent = 'FAIL: ' + error.message;
    }
  });

  function post(xhr) {
    xhr.open('POST', '/api/sites');
    xhr.setRequestHeader('Content-Type', 'application/x-www-form-urlencoded; charset=UTF-8');
    xhr.setRequestHeader('X-Fixture', 'one');
    xhr.setRequestHeader('X-Fixture', 'two');
    xhr.withCredentials = true;
  }

  document.getElementById('run-network').onclick = async function () {
    var result = document.getElementById('network-result');
    result.textContent = 'Running network';
    try {
      await new Promise(function (resolve, reject) {
        var xhr = new XMLHttpRequest();
        var states = [];
        var events = [];
        xhr.onreadystatechange = function (event) {
          try {
            assert(this === xhr && event.target === xhr, 'readystatechange has no receiver or event');
            states.push(xhr.readyState);
          } catch (error) { reject(error); }
        };
        xhr.addEventListener('load', function (event) {
          try {
            assert(event.target === xhr, 'load event target missing');
            assert(xhr.status === 201 && xhr.statusText === 'Created', 'HTTP status was discarded');
            assert(xhr.getResponseHeader('X-Fixture') === 'received', 'response header missing');
            assert(xhr.getAllResponseHeaders().includes('content-type: application/json'),
              'response headers missing');
            assert(xhr.response && xhr.response.accepted === true, 'JSON response missing');
            assert(states.join(',') === '1,2,3,4', 'XHR state ordering: ' + states.join(','));
            events.push('load');
          } catch (error) { reject(error); }
        }, { once: true });
        xhr.onloadend = function (event) {
          try {
            assert(event.target === xhr && events.join(',') === 'load', 'loadend ordering');
            resolve();
          } catch (error) { reject(error); }
        };
        xhr.onerror = function () { reject(new Error('POST failed')); };
        post(xhr);
        xhr.responseType = 'json';
        xhr.send('age=18');
      });

      var synchronous = new XMLHttpRequest();
      synchronous.open('POST', '/api/sites', false);
      synchronous.setRequestHeader('Content-Type', 'application/x-www-form-urlencoded');
      synchronous.setRequestHeader('X-Fixture', 'one');
      synchronous.setRequestHeader('X-Fixture', 'two');
      synchronous.send('age=18');
      assert(synchronous.readyState === 4 && synchronous.status === 201,
        'synchronous send returned before the response');
      assert(JSON.parse(synchronous.responseText).accepted === true,
        'synchronous response body missing');

      await new Promise(function (resolve, reject) {
        var xhr = new XMLHttpRequest();
        xhr.open('GET', '/api/invalid-json');
        xhr.responseType = 'json';
        xhr.onload = function () {
          if (xhr.status === 200 && xhr.response === null) resolve();
          else reject(new Error('invalid JSON did not return null'));
        };
        xhr.onerror = function () { reject(new Error('invalid JSON became a network failure')); };
        xhr.send();
      });

      var missing = await fetch('/api/missing');
      assert(missing.status === 404 && !missing.ok, 'HTTP error became a transport error');
      assert(await missing.text() === 'not found', 'HTTP error body was discarded');

      await new Promise(function (resolve, reject) {
        var xhr = new XMLHttpRequest();
        var events = [];
        xhr.open('GET', '/api/slow');
        xhr.onabort = function (event) {
          if (event.target !== xhr) reject(new Error('abort event target missing'));
          events.push('abort');
        };
        xhr.onload = function () { reject(new Error('aborted response was delivered')); };
        xhr.onloadend = function () {
          if (events.join(',') === 'abort') resolve();
          else reject(new Error('abort ordering'));
        };
        xhr.send();
        xhr.abort();
        assert(xhr.readyState === 0 && xhr.status === 0, 'abort did not reset XHR');
      });

      result.textContent = 'READY: XHR headers status events sync abort PASS';
    } catch (error) {
      result.textContent = 'FAIL: ' + error.message;
    }
  };

  document.getElementById('run-window').onclick = async function () {
    var result = document.getElementById('window-result');
    var worker = null;
    result.textContent = 'Waiting for delayed fetch';
    try {
      // No page timer or animation loop keeps the window awake during this chain.
      var response = await fetch('/api/slow');
      var payload = await response.json();
      result.textContent = 'Waiting for worker';
      worker = new Worker('data:text/javascript,' + encodeURIComponent(
        'onmessage = event => postMessage(event.data * 2);'
      ));
      var answer = await new Promise(function (resolve, reject) {
        worker.onmessage = function (event) { resolve(event.data); };
        worker.onerror = function (event) { reject(new Error(event.message)); };
        worker.postMessage(payload.value);
      });
      assert(answer === 42, 'worker result missing');
      worker.terminate();
      worker = null;

      result.textContent = 'Waiting for GPU';
      var adapter = await navigator.gpu.requestAdapter();
      assert(adapter, 'WebGPU adapter unavailable');
      var device = await adapter.requestDevice();
      device.pushErrorScope('validation');
      var canvas = document.getElementById('window-canvas');
      var context = canvas.getContext('webgpu');
      assert(context, 'WebGPU canvas unavailable');
      context.configure({
        device: device,
        format: 'rgba8unorm',
        alphaMode: 'opaque',
        usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_SRC
      });
      var encoder = device.createCommandEncoder();
      var pass = encoder.beginRenderPass({
        colorAttachments: [{
          view: context.getCurrentTexture().createView(),
          clearValue: { r: 0.1, g: 0.8, b: 0.2, a: 1 },
          loadOp: 'clear',
          storeOp: 'store'
        }]
      });
      pass.end();
      device.queue.submit([encoder.finish()]);
      await device.queue.onSubmittedWorkDone();
      var error = await device.popErrorScope();
      assert(!error, error ? error.message : 'GPU validation failed');

      var copy = document.createElement('canvas');
      copy.width = canvas.width;
      copy.height = canvas.height;
      var drawing = copy.getContext('2d');
      drawing.drawImage(canvas, 0, 0);
      var pixel = drawing.getImageData(80, 50, 1, 1).data;
      assert(pixel[1] > 180 && pixel[0] < 60 && pixel[2] < 80 && pixel[3] === 255,
        'GPU canvas pixels were not presented');
      result.textContent = 'READY: idle fetch worker GPU presentation PASS';
    } catch (error) {
      result.textContent = 'FAIL: ' + error.message;
    } finally {
      if (worker) worker.terminate();
    }
  };
})();
