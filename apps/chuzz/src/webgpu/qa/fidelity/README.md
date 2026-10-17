# Texture fidelity witness

Run from the repository root with a built headless host and a compatible QA runner:

```sh
PS_QA_HEADLESS=true "$PS_QA_BIN" qa-hosted \
  --host "$PWD/target/release/chuzz-headless" \
  --page "$PWD/apps/chuzz/src/webgpu/qa/fidelity" \
  --app "$PWD/apps/chuzz/src/webgpu/qa/fidelity/ps-qa.ron" \
  --checks "$PWD/apps/chuzz/src/webgpu/qa/fidelity/checks"
```

The page compares exact shader output bytes for synthetic 2x2 PNG, VP8L,
and VP8+ALPH images. Both WebP variants are tested with empty and explicit
MIME types. Bitmaps disable color conversion and flip vertically, with explicit
none and default premultiplication options. External copies use the default
straight-alpha destination.
Separate shader passes expose hidden RGB and alpha as opaque output pixels.

The lossless WebP and original PNG contain identical straight RGBA values.
The lossy reference PNG contains the WebP's decoded RGBA values, since VP8
encoding changes RGB. The WebP encoder's exact option retained hidden RGB;
decoding the lossy fixture produced the reference PNG without compositing.

Compression uses the first supported family: BC7 mode 6, ASTC 4x4 void extent,
or ETC2 individual mode. The page requests that feature and checks membership
and iteration on both adapter and device. Both upload methods sample known
colors at base, 2x2, and 1x1 mip levels, including explicit physical block
extents. Queue writes use compact block rows; buffer copies omit the optional
row pitch for their single block row. An adapter with no supported compression
family fails the witness instead of silently skipping it.

The final canvas displays the sampled compressed color beside black so capture
checks can distinguish rendered content from a uniform background.
