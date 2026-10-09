// The fixture source is directly executable JavaScript syntax.
(function () {
  var result = document.getElementById('result');
  var movie = document.getElementById('movie');

  function assert(value, message) {
    if (!value) throw new Error(message);
  }

  // Clock, buffered ranges and ready state, so a stalled wait says where playback stopped.
  function mediaState() {
    var movie = document.getElementById('movie'), ranges = [];
    for (var i = 0; i < movie.buffered.length; i++) {
      ranges.push(movie.buffered.start(i).toFixed(3) + '-' + movie.buffered.end(i).toFixed(3));
    }
    return '(t=' + movie.currentTime.toFixed(3) + ' buffered=' + ranges.join(',') + ' ready=' + movie.readyState +
      ' paused=' + movie.paused + ')';
  }

  function waitFor(predicate, message, timeout) {
    return new Promise(function (resolve, reject) {
      var deadline = Date.now() + (timeout || 15000);
      function poll() {
        try {
          if (predicate()) { resolve(); return; }
          if (Date.now() >= deadline) { reject(new Error(message + ' ' + mediaState())); return; }
          setTimeout(poll, 25);
        } catch (error) { reject(error); }
      }
      poll();
    });
  }

  function operation(buffer, action) {
    return new Promise(function (resolve, reject) {
      var failed = false;
      function error() { failed = true; }
      function done() {
        buffer.removeEventListener('error', error);
        buffer.removeEventListener('updateend', done);
        if (failed) reject(new Error('SourceBuffer operation failed'));
        else resolve();
      }
      buffer.addEventListener('error', error);
      buffer.addEventListener('updateend', done);
      try { action(); }
      catch (failure) {
        buffer.removeEventListener('error', error);
        buffer.removeEventListener('updateend', done);
        reject(failure);
      }
    });
  }

  async function bytes(path) {
    var response = await fetch(new URL(path, location.href).href);
    if (!response.ok) throw new Error('Fixture fetch failed: ' + path);
    return new Uint8Array(await response.arrayBuffer());
  }

  async function appendSnapshot(buffer, input) {
    var storage = new Uint8Array(input.length + 23);
    storage.set(input, 11);
    await operation(buffer, function () {
      buffer.appendBuffer(storage.subarray(11, 11 + input.length));
      storage.fill(0);
    });
  }

  document.getElementById('run').onclick = async function () {
    result.textContent = 'Running';
    var url = null;
    try {
      movie.pause();
      movie.srcObject = null;
      movie.removeAttribute('src');
      movie.load();
      movie.muted = true;
      assert(MediaSource.isTypeSupported('video/mp4; codecs="avc1.64001f, mp4a.40.2"'),
        'H.264 and AAC-LC capability missing');
      assert(MediaSource.isTypeSupported('audio/mp4; codecs="mp4a.40.2"'),
        'AAC-LC SourceBuffer capability missing');
      [
        'video/webm; codecs="vp9"',
        'video/mp4; codecs="av01.0.05M.08"',
        'audio/mp4; codecs="opus"',
        'audio/mp4; codecs="mp4a.40.5"',
        'video/mp2t; codecs="avc1.64001f, mp4a.40.2"'
      ].forEach(function (type) {
        assert(!MediaSource.isTypeSupported(type), 'Unsupported capability advertised: ' + type);
        assert(movie.canPlayType(type) === '', 'Unsupported canPlayType: ' + type);
      });
      var response = await fetch(new URL('./clip.json', location.href).href);
      if (!response.ok) throw new Error('Missing clip.json');
      var manifest = await response.json();
      var source = new MediaSource();
      var opened = new Promise(function (resolve) {
        source.addEventListener('sourceopen', resolve, { once: true });
      });
      url = URL.createObjectURL(source);
      movie.src = url;
      await opened;
      var tracks = [];
      for (var spec of manifest.tracks) {
        var buffer = source.addSourceBuffer(spec.type);
        assert(source.sourceBuffers[source.sourceBuffers.length - 1] === buffer,
          'SourceBufferList indexing failed');
        assert(buffer.mode === 'segments' && !buffer.updating, 'SourceBuffer initial state');
        await appendSnapshot(buffer, await bytes(spec.init));
        tracks.push({ spec: spec, buffer: buffer });
      }
      source.duration = Infinity;
      source.setLiveSeekableRange(0, 3);
      source.clearLiveSeekableRange();

      for (var track of tracks) {
        var first = await bytes(track.spec.segments[0]);
        var split = Math.min(13, first.length - 1);
        await appendSnapshot(track.buffer, first.subarray(0, split));
        await appendSnapshot(track.buffer, first.subarray(split));
        assert(track.buffer.buffered.length > 0, 'First media segment has no buffered range');
      }
      var playing = movie.play();
      playing.catch(function () {});
      await waitFor(function () {
        return movie.currentTime > 0.1 && movie.buffered.length > 0;
      }, 'Clock did not advance from the first segment');
      var firstEnd = movie.buffered.end(movie.buffered.length - 1);
      await waitFor(function () {
        return movie.currentTime >= firstEnd - 0.1;
      }, 'Playback did not reach the initial buffered boundary');
      for (var track of tracks) {
        for (var index = 1; index < track.spec.segments.length; index++) {
          await appendSnapshot(track.buffer, await bytes(track.spec.segments[index]));
        }
      }
      await playing;
      assert(tracks.every(function (track) { return !track.buffer.updating; }),
        'SourceBuffer stayed updating');
      source.endOfStream();
      assert(source.readyState === 'ended', 'endOfStream state missing');
      await waitFor(function () {
        return movie.buffered.length > 0 && movie.currentTime > firstEnd + 0.1;
      }, 'Playback did not resume after append');
      await waitFor(function () {
        return globalThis.__chuzzVideoHasColour(movie);
      }, 'Presented frame has no non-black RGB pixels');

      movie.pause();
      movie.currentTime = 0.2;
      await waitFor(function () { return !movie.seeking && movie.readyState >= 2; },
        'Buffered backward seek did not finish');
      await movie.play();
      await waitFor(function () { return movie.currentTime > 0.3; },
        'Clock did not advance after seek');
      await waitFor(function () { return movie.ended; }, 'Playback did not end after endOfStream');
      assert(Number.isFinite(movie.duration) && movie.duration > 1, 'Invalid final duration');
      assert(globalThis.__chuzzVideoHasColour(movie), 'Final displayed frame is black');

      for (var track of tracks) {
        await operation(track.buffer, function () {
          track.buffer.remove(movie.duration / 2, Infinity);
        });
        assert(source.readyState === 'open', 'remove did not reopen the ended source');
        track.buffer.abort();
        track.buffer.appendWindowStart = 0;
        track.buffer.appendWindowEnd = Infinity;
        track.buffer.timestampOffset = 0;
        track.buffer.mode = 'sequence';
        assert(track.buffer.mode === 'sequence', 'Sequence mode setter failed');
        track.buffer.mode = 'segments';
      }
      result.textContent = 'READY: MSE buffered seek clock frame EOS PASS';
    } catch (error) {
      result.textContent = 'FAIL: ' + error.message;
    } finally {
      if (url) URL.revokeObjectURL(url);
    }
  };
})();
