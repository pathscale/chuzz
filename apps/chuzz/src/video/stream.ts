// JavaScript syntax is intentional. This runs after the MSE srcObject shim.
(function () {
  var prototypes = [
    Object.getPrototypeOf(document.createElement('video')),
    Object.getPrototypeOf(document.createElement('audio'))
  ];
  var seen = new Set();
  prototypes.forEach(function (prototype) {
    if (seen.has(prototype)) return;
    seen.add(prototype);
    var owner = prototype;
    var original;
    while (owner && !original) {
      original = Object.getOwnPropertyDescriptor(owner, 'srcObject');
      owner = Object.getPrototypeOf(owner);
    }
    Object.defineProperty(prototype, 'srcObject', {
      configurable: true,
      enumerable: true,
      get: function () {
        return globalThis.__chuzzVideoGetStream(this)
          || (original && original.get ? original.get.call(this) : null);
      },
      set: function (value) {
        if (value instanceof MediaStream) {
          if (original && original.set) original.set.call(this, null);
          globalThis.__chuzzVideoSetStream(this, value);
          return;
        }
        if (value !== null
            && !(typeof MediaSource === 'function' && value instanceof MediaSource)) {
          throw new TypeError('srcObject requires a MediaStream, MediaSource or null');
        }
        globalThis.__chuzzVideoSetStream(this, null);
        if (original && original.set) original.set.call(this, value);
        else if (value !== null) {
          throw new TypeError('MediaSource is unavailable on this media element');
        } else {
          this.load();
        }
      }
    });
  });
})();

