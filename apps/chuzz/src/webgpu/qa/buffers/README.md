# GPUBuffer inspection proof page

This page uses plain JavaScript and WGSL. Each animation frame dispatches one
64-invocation compute workgroup, then renders 64 triangles using its output
and drawIndirect. It never reads the buffers back in page JavaScript.

| Buffer | Layout | Compute output |
| --- | --- | --- |
| qa-counter | 4 bytes, atomic u32 | Incremented once per dispatch |
| qa-particles | 1024 bytes, 64 vec4<f32> | Finite time-dependent components in [-1,1] |
| qa-indirect | 16 bytes, four u32 | 3, 64, 0, 0 |

The page starts with its Start buffer fixture button. Each check prepares the
page by pressing that button unless READY: WebGPU buffers already paints.
Readiness waits for the first frame's GPU work to complete. Following frames
continue through requestAnimationFrame without waiting for queue completion.

Run the functional witness from the repository with existing binaries:

```text
bun scripts/qa-gpu-buffers.ts

Each suite first activates GPU inspection without starting the page. The next check clicks Start, so labeled buffers receive inspection usage before their first write.
