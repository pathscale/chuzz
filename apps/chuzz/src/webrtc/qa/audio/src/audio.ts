(function () {
  "use strict";

  const result = document.getElementById("result");
  const measurements = document.getElementById("measurements");

  function assert(value, message) {
    if (!value) throw new Error(message);
  }

  function sleep(milliseconds) {
    return new Promise(resolve => setTimeout(resolve, milliseconds));
  }

  async function waitFor(predicate, message) {
    const deadline = Date.now() + 3000;
    while (!predicate()) {
      if (Date.now() >= deadline) throw new Error(message);
      await sleep(20);
    }
  }

  function floats(analyser) {
    const samples = new Float32Array(analyser.fftSize);
    analyser.getFloatTimeDomainData(samples);
    return samples;
  }

  function rms(analyser) {
    const samples = floats(analyser);
    let squares = 0;
    for (const sample of samples) squares += sample * sample;
    return Math.sqrt(squares / samples.length);
  }

  function peak(analyser, context) {
    const bins = new Uint8Array(analyser.frequencyBinCount);
    analyser.getByteFrequencyData(bins);
    let index = 1;
    for (let current = 2; current < bins.length; current++) {
      if (bins[current] > bins[index]) index = current;
    }
    return { frequency: index * context.sampleRate / analyser.fftSize, level: bins[index] };
  }

  function analyser(context, size) {
    return new AnalyserNode(context, {
      fftSize: size,
      minDecibels: -100,
      maxDecibels: 0,
      smoothingTimeConstant: 0,
    });
  }

  document.getElementById("run").onclick = async function () {
    const owners = [];
    result.textContent = "Running Web Audio";
    measurements.textContent = "";
    try {
      const context = new AudioContext({ sampleRate: 48000, latencyHint: "interactive" });
      owners.push(context);
      assert(context.state === "suspended", "context did not start suspended");
      assert(context.currentTime === 0, "new context clock is not zero");
      let changes = 0;
      context.onstatechange = () => { changes++; };

      const first = new OscillatorNode(context, { frequency: 440 });
      const second = new OscillatorNode(context, { frequency: 880 });
      const gain = new GainNode(context, { gain: 0.4 });
      const mixed = context.createMediaStreamDestination();
      first.connect(gain);
      second.connect(gain);
      gain.connect(mixed);
      const source = context.createMediaStreamSource(mixed.stream);
      const monitor = analyser(context, 1024);
      source.connect(monitor);
      first.start();
      second.start();
      await context.resume();
      await waitFor(() => context.currentTime > 0.08 && rms(monitor) > 0.2,
        "analyser-only graph did not render samples");

      assert(monitor.frequencyBinCount === 512, "1024 FFT bin count");
      const initial = rms(monitor);
      assert(initial > 0.25 && initial < 0.55, "mixed destination RMS is implausible");
      const initialStats = __chuzzAudioInspect(context);
      assert(initialStats.samples > 0, "headless sink did not consume frames");
      assert(initialStats.sumSquares === 0, "disconnected context destination was audible");

      source.connect(context.destination);
      await waitFor(() => __chuzzAudioInspect(context).sumSquares > 10,
        "stream source did not reach the headless sink");

      second.disconnect(gain);
      await sleep(60);
      const tone = peak(monitor, context);
      assert(Math.abs(tone.frequency - 440) < 2 * context.sampleRate / monitor.fftSize,
        "analyser peak is not the rendered 440 Hz tone");
      assert(tone.level > 120, "frequency analyser returned no tone energy");

      monitor.fftSize = 256;
      assert(monitor.frequencyBinCount === 128, "256 FFT bin count");
      const short = new Uint8Array(9);
      monitor.getByteFrequencyData(short);
      const long = new Uint8Array(300);
      long.fill(37);
      monitor.getByteTimeDomainData(long);
      assert(long[299] === 37, "analyser overwrote the array tail");
      monitor.fftSize = 1024;

      gain.gain.setValueAtTime(0.4, context.currentTime);
      gain.gain.linearRampToValueAtTime(0.04, context.currentTime + 0.08);
      await sleep(150);
      const reduced = rms(monitor);
      assert(reduced > 0.015 && reduced < 0.05, "gain ramp did not affect real samples");
      gain.gain.setTargetAtTime(0.2, context.currentTime, 0.02);
      await sleep(150);
      assert(rms(monitor) > reduced * 3, "setTargetAtTime did not affect samples");
      gain.gain.cancelAndHoldAtTime(context.currentTime);
      const held = rms(monitor);
      await sleep(60);
      assert(Math.abs(rms(monitor) - held) < 0.02, "cancelAndHoldAtTime did not hold");

      const original = mixed.stream.getAudioTracks()[0];
      const clone = original.clone();
      assert(clone !== original && clone.id !== original.id, "track clone identity");
      original.stop();
      assert(original.readyState === "ended" && clone.readyState === "live",
        "stopping one generated track stopped its clone");
      const cloneStream = new MediaStream([clone]);
      const clonedSource = context.createMediaStreamSource(cloneStream);
      const cloneMonitor = analyser(context, 1024);
      clonedSource.connect(cloneMonitor);
      await waitFor(() => rms(cloneMonitor) > 0.08, "clone lost shared PCM");
      clone.enabled = false;
      await sleep(60);
      assert(rms(cloneMonitor) === 0, "disabled clone did not render silence");
      clone.enabled = true;
      await waitFor(() => rms(cloneMonitor) > 0.08, "re-enabled clone stayed silent");

      let cycleRejected = false;
      try { clonedSource.connect(gain); }
      catch (error) { cycleRejected = error.name === "NotSupportedError"; }
      assert(cycleRejected, "implicit destination/source feedback cycle was accepted");

      await context.suspend();
      const paused = context.currentTime;
      const pausedSamples = __chuzzAudioInspect(context).samples;
      await sleep(60);
      assert(context.currentTime === paused, "suspended clock advanced");
      assert(__chuzzAudioInspect(context).samples === pausedSamples, "suspended sink consumed samples");
      await context.resume();
      await waitFor(() => context.currentTime > paused, "resumed clock did not advance");

      const placeholder = new AudioContext({ sampleRate: 8000 });
      owners.push(placeholder);
      const carrier = new OscillatorNode(placeholder, { frequency: 440 });
      const quietGain = new GainNode(placeholder, { gain: 0.0005 });
      const quietDestination = placeholder.createMediaStreamDestination();
      const quietMonitor = analyser(placeholder, 256);
      carrier.connect(quietGain);
      quietGain.connect(quietDestination);
      quietGain.connect(quietMonitor);
      carrier.start();
      await placeholder.resume();
      await waitFor(() => rms(quietMonitor) > 0.0001, "near-silent placeholder has no PCM");
      const quietRms = rms(quietMonitor);
      assert(quietRms > 0.0002 && quietRms < 0.0005, "placeholder amplitude is incorrect");

      const resampled = context.createMediaStreamSource(quietDestination.stream);
      const amplification = new GainNode(context, { gain: 1000 });
      const resampleMonitor = analyser(context, 1024);
      resampled.connect(amplification);
      amplification.connect(resampleMonitor);
      await waitFor(() => rms(resampleMonitor) > 0.2, "8 kHz stream was not resampled");
      const resampledPeak = peak(resampleMonitor, context);
      assert(Math.abs(resampledPeak.frequency - 440) < 100, "resampling changed tone frequency");

      const scheduled = context.createOscillator();
      const stopped = new Promise((resolve, reject) => {
        scheduled.addEventListener("ended", resolve, { once: true });
        setTimeout(() => reject(new Error("scheduled oscillator ended event missing")), 2000);
      });
      scheduled.start(context.currentTime + 0.02);
      scheduled.stop(context.currentTime + 0.05);
      await stopped;

      await context.setSinkId("chuzz-fake-output");
      assert(context.sinkId === "chuzz-fake-output", "fake output selection");
      let badSink = false;
      try { await context.setSinkId("missing-output"); }
      catch (error) { badSink = error.name === "NotFoundError"; }
      assert(badSink, "unknown output was accepted");

      const stats = __chuzzAudioInspect(context);
      assert(stats.samples > initialStats.samples && stats.energy > 0,
        "headless sample or energy counters did not advance");
      assert(changes >= 3, "context statechange events missing");
      await placeholder.close();
      await context.close();
      assert(context.state === "closed" && clone.readyState === "ended",
        "closing context did not end generated capture");

      measurements.textContent = JSON.stringify({
        mixedRms: initial,
        reducedRms: reduced,
        quietRms,
        tone,
        resampledPeak,
        sink: stats,
      }, null, 2);
      result.textContent = "READY: Web Audio graph PASS";
    } catch (error) {
      result.textContent = "FAIL: " + error.name + ": " + error.message;
    } finally {
      for (const context of owners) {
        if (context.state !== "closed") await context.close();
      }
    }
  };
})();

