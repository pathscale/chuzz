// UI fixtures use directly executable JavaScript syntax.
(function () {
  function assert(value, message) {
    if (!value) throw new Error(message);
  }

  function verify(report, family) {
    assert(report && report.serverPort > 0 && report.clientPort > 0,
      family + ': missing bound ports');
    assert(report.serverPort !== report.clientPort,
      family + ': local ports were not distinct');
    assert(Array.isArray(report.received) && report.received.join(',') === '0,1,2,3,4,5,6,7',
      family + ': sequenced receive burst did not drain');
    assert(Array.isArray(report.echoes) && report.echoes.join(',') === '7,6,5,4,2,1,0',
      family + ': reordered echoes changed');
    assert(Array.isArray(report.lost) && report.lost.join(',') === '3',
      family + ': intentional loss was not measured');
    assert(report.timeouts === 3, family + ': timeout and cancellation accounting changed');
    ['empty', 'oversized', 'cancelRecovered'].forEach(function (name) {
      assert(report[name] === true, family + ': ' + name + ' failed');
    });
    assert(typeof report.sharedPort === 'boolean',
      family + ': shared-port support was not reported');
  }

  document.getElementById('run').onclick = async function () {
    var button = document.getElementById('run');
    var result = document.getElementById('result');
    var evidence = document.getElementById('evidence');
    button.disabled = true;
    result.setAttribute('data-state', 'running');
    result.setAttribute('aria-label', 'UDP running');
    result.textContent = 'UDP running';
    evidence.textContent = '';
    try {
      var response = await fetch('/api/udp', { method: 'POST' });
      if (!response.ok) {
        throw new Error('native host: ' + response.status + ' ' + await response.text());
      }
      var report = await response.json();
      verify(report.ipv4, 'IPv4');
      verify(report.ipv6, 'IPv6');
      evidence.textContent = JSON.stringify(report, null, 2);
      result.setAttribute('data-state', 'pass');
      result.setAttribute('aria-label', 'READY: udp PASS');
      result.textContent = 'READY: udp PASS';
    } catch (error) {
      result.setAttribute('data-state', 'fail');
      result.setAttribute('aria-label', 'UDP failed');
      result.textContent = 'FAIL: ' + error.message;
    } finally {
      button.disabled = false;
    }
  };
})();

