// Live video sources stay native until a consumer transforms their pixels.
(function () {
  var originalSource = globalThis.__chuzzGpuImageSource;
  var originalMake2D = globalThis.__chuzzMake2D;

  function frame(source) {
    return typeof globalThis.__chuzzVideoFrameSource === 'function'
      ? globalThis.__chuzzVideoFrameSource(source) : null;
  }

  globalThis.__chuzzGpuImageSource = function (source) {
    var video = frame(source);
    if (video) return { kind: 'video', handle: video.handle };
    return originalSource(source);
  };

  globalThis.__chuzzMake2D = function (canvas, id, pixelsOf) {
    return originalMake2D(canvas, id, function (source) {
      var video = frame(source);
      if (video) {
        return {
          nativeVideo: video.handle,
          width: video.width,
          height: video.height
        };
      }
      return pixelsOf(source);
    });
  };
})();

