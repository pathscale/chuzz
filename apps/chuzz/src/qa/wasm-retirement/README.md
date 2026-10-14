# S11 functional witness

Run `bun scripts/qa-wasm-retirement.ts` from an integrated Chuzz checkout.
The runner builds nothing and edits no source. It requires the patched
prebuilt GUI and headless binaries, ps-qa, a GUI binary with capture disabled,
and separate binaries carrying each defect below. It creates evidence under
`target/qa-s11/evidence`, overridable with `QA_WASM_OUTPUT_DIR`.

The patched ps-blitz-script must be linked into every normal Chuzz artifact.
The owner should publish that release or supply an opt-in local path override
during integration. Default Cargo resolution of an older published engine
does not prove part1. No remaining ps-blitz public signatures were removed.

| Artifact | Default path | Override |
|---|---|---|
| ps-qa | sibling ps-observability/target/release/ps-qa | PS_QA_BIN |
| Patched GUI | target/release/chuzz-gui | QA_WASM_GUI_BIN |
| Patched host | target/release/chuzz-headless | QA_HOST_BIN |
| GUI without capture | target/qa-s11/minimal/chuzz-gui | QA_WASM_MINIMAL_GUI_BIN |
| wasm flag defect GUI | target/qa-s11/wasm-flag-name/chuzz-gui | QA_WASM_DEFECT_GUI_BIN |
| wasm flag defect host | target/qa-s11/wasm-flag-name/chuzz-headless | QA_WASM_DEFECT_HOST_BIN |
| capture flag defect GUI | target/qa-s11/capture-wasm-flag-name/chuzz-gui | QA_CAPTURE_WASM_DEFECT_GUI_BIN |
| capture flag defect host | target/qa-s11/capture-wasm-flag-name/chuzz-headless | QA_CAPTURE_WASM_DEFECT_HOST_BIN |
| global defect host | target/qa-s11/webassembly-global/chuzz-headless | QA_WEBASSEMBLY_DEFECT_HOST_BIN |

For the minimal GUI artifact, select gui and javascript but omit capture
and all defaults that imply it. The flag guard itself has no feature gate.

## Plant procedure

`plants.json` contains exact source replacements against the integrated
parts. In disposable integration builds, apply exactly one recipe at a time,
requiring its SEARCH to match once. Build the designated artifact outside
this lane, restore the source, and provide the artifact to the runner.
The wasm flag plant and capture flag plant each keep exit 2 and remove only
the named diagnostic for their own flag. The global plant exposes an
undefined property, so direct instantiation raises TypeError rather than
ReferenceError. It introduces no guest executor or mock WebAssembly engine.

Normal CLI runs exercise each flag with no operand, a nonexistent module,
equals syntax and a competing capture flag in both binaries. They assert
exit 2, wasm-unsupported on stderr, empty stdout and no PNG/tree creation.
The observed outcomes become a page inspected through the real Chuzz host.
ps-qa must pass wasm-cli-refusal and capture-wasm-cli-refusal for normal
artifacts. Each flag plant must fail exactly its corresponding named check.

The page witness clicks a real DOM button and calls WebAssembly.instantiate
with the bytes of an empty module. It must render the named ReferenceError
outcome and report no WebAssembly property. The global plant must fail
webassembly-reference-error. Neither fixtures nor expected verdicts change
between normal and planted runs. Missing binaries, timeouts, missing or
duplicate report rows, wrong exit statuses and not-run verdicts are errors.

Keep the raw CLI observations and ps-qa TOON reports emitted by the runner.
No runtime proof was executed in the read-only patch lane.
