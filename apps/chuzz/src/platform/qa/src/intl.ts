// UI fixtures use directly executable JavaScript syntax.
(function () {
  function assert(value, message) {
    if (!value) throw new Error(message);
  }

  document.getElementById('run').onclick = function () {
    var result = document.getElementById('result');
    try {
      var date = new Date(Date.UTC(2026, 9, 10, 12, 0, 0));
      var formatted = new Intl.DateTimeFormat('en-US', { timeZone: 'UTC', year: 'numeric', month: 'long', day: 'numeric' }).format(date);
      assert(formatted === 'October 10, 2026', 'DateTimeFormat: ' + formatted);
      var number = new Intl.NumberFormat('en-US').format(1234567.891);
      assert(number === '1,234,567.891', 'NumberFormat: ' + number);
      var german = new Intl.NumberFormat('de-DE').format(1234.5);
      assert(german === '1.234,5', 'NumberFormat de: ' + german);
      var plural = new Intl.PluralRules('en-US');
      assert(plural.select(1) === 'one' && plural.select(2) === 'other', 'PluralRules');
      var collator = new Intl.Collator('en');
      assert(['b', 'a', 'C'].sort(collator.compare).join('') === 'abC', 'Collator');
      assert(Intl.DateTimeFormat().resolvedOptions().locale.length > 0, 'resolvedOptions locale');
      assert((5).toLocaleString('en-US', { minimumIntegerDigits: 2 }) === '05', 'toLocaleString');
      assert(typeof date.toLocaleDateString('en-US') === 'string', 'toLocaleDateString');
      result.textContent = 'READY: intl PASS';
    } catch (error) {
      result.textContent = 'FAIL: ' + error.message;
    }
  };
})();
