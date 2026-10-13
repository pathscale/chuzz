# GPUBuffer inspection proof page

This page uses plain JavaScript and WGSL. Each animation frame dispatches one
64-invocation compute workgroup, then renders 64 triangles using its output
and drawIndirect. It never reads the buffers back in page JavaScript.

| Buffer | Layout | Compute output |
| --- | --- | --- |
| qa-counter | 4 bytes, atomic u32 | Incremented once per dispatch |
| qa-particles | 1024 bytes, 64 vec4<f32> | Finite time-dependent components in [-1,1] |
| qa-indirect | 16 bytes, four u32 | 3, 64, 0, 0 |

Each suite first activates GPU inspection without starting the page. The next
check presses Start, so labeled buffers receive inspection usage before their
first write.

The page starts with its Start buffer fixture button. Each check prepares the
page by pressing that button unless READY: WebGPU buffers already paints.
Readiness waits for the first frame's GPU work to complete. Following frames
continue through requestAnimationFrame without waiting for queue completion.

Run the functional witness from the repository with existing binaries:

```text
bun scripts/qa-gpu-buffers.ts
```

The defaults are target/release/chuzz-headless,
target/release/chuzz-qa-fixture and the sibling ps-observability checkout's
target/release/ps-qa. Override them with QA_HOST_BIN, QA_FIXTURE and PS_QA_BIN.
The host and ps-qa must include the companion GPUBuffer inspection contract.
The fixture binary must include this page's route and readiness announcement.
The runner does not build binaries.

The runner serves the embedded page through scripts/qa-h2-front.ts, which
offers only h2 over TLS, using a temporary CA trusted through SSL_CERT_FILE.
It loads https://127.0.0.1:49123/webgpu/buffers/index.html with a fresh profile
and real headless host for each mode. QA_GPU_FRONT_PORT and
QA_GPU_UPSTREAM_PORT override the default ports 49123 and 49124.
The shared /fonts/FiraSans-Regular.woff2 route supplies the bundled font.
Child processes and temporary TLS/profile directories are cleaned up on exit.

checks/buffers.ron is the positive suite. It asserts all 256 particle
components, the atomic counter, vertexCount at byte offset 0, instanceCount
at byte offset 4, and particle changes between different completed submits.
Readiness and GpuNoErrors are additional guards.

The planted suites are outside checks so positive discovery cannot run them.

| Query | Suite | Required failure |
| --- | --- | --- |
| ?break=nan | negative/nan.ron | gpu-buffers-particles-range |
| ?break=skip | negative/skip.ron | gpu-buffers-counter |
| ?break=freeze | negative/freeze.ron | gpu-buffers-particles-changes |

NaN mode stores a quiet NaN in particle 0's z component, element index 2.
Its suite checks the counter and indirect arguments before the range failure.
Skip mode never dispatches; a completed initialization clear leaves the
counter zero and particles finite. Freeze mode keeps time at zero while
continuing to dispatch, increment the counter and rewrite unchanged particles.
Its suite checks the finite range, counter and indirect arguments before the
changes failure.

The runner requires ps-qa to exit 0 with exactly eight passing checks in normal
mode. Each planted mode must exit 1, fail its named check with a buffer label
and submit diagnostic, and pass every other check in that suite. NaN also
requires a NaN diagnostic. A missing, truncated or unexpected report fails the
runner, as does an unrelated startup, preparation or GPU validation failure.
The final PASS lines report the asserted outcomes for the real entry point.
