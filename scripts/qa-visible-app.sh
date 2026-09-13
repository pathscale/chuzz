#!/bin/bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
app=${1:-"$root/target/release/bundle/macos/Chuzz.app"}
qa_bin=${PS_QA_BIN:-$(command -v ps-qa || true)}
fixture_bin=${CHUZZ_QA_FIXTURE_BIN:-"$root/target/release/chuzz-qa-fixture"}
artifacts=${QA_ARTIFACT_DIR:-$(mktemp -d /private/tmp/chuzz-visible-XXXXXX)}
qa_home="$artifacts/home"
descriptor=""
app_pid=""
server_pid=""

if [[ ! -x "$app/Contents/MacOS/chuzz-gui" ]]; then
  printf 'Chuzz executable is missing: %s\n' "$app/Contents/MacOS/chuzz-gui" >&2
  exit 1
fi
if [[ -z "$qa_bin" || ! -x "$qa_bin" ]]; then
  printf "ps-qa is missing; set PS_QA_BIN or install ps-qa '^0.7'\n" >&2
  exit 1
fi
if [[ ! -x "$fixture_bin" ]]; then
  printf 'Chuzz QA fixture is missing: %s\n' "$fixture_bin" >&2
  exit 1
fi

cleanup() {
  if [[ -n "$app_pid" ]] && kill -0 "$app_pid" 2>/dev/null; then
    kill -TERM "$app_pid" 2>/dev/null || true
    for _ in {1..40}; do
      kill -0 "$app_pid" 2>/dev/null || break
      sleep 0.1
    done
    kill -KILL "$app_pid" 2>/dev/null || true
  fi
  if [[ -n "$server_pid" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill -TERM "$server_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

mkdir -p "$qa_home/Library/Application Support/ai.chuzz.browser" "$artifacts/pixels"
printf '%s\n' '{"inspection":true,"profiling":false}' \
  > "$qa_home/Library/Application Support/ai.chuzz.browser/diagnostics.json"

"$fixture_bin" --port 49123 > "$artifacts/fixture.log" 2>&1 &
server_pid=$!
HOME="$qa_home" "$app/Contents/MacOS/chuzz-gui" \
  http://127.0.0.1:49123/ > "$artifacts/chuzz.log" 2>&1 &
app_pid=$!

descriptor_dir=${TMPDIR:-/tmp}/tauri-blitz-agent
for _ in {1..200}; do
  candidate=$(find "$descriptor_dir" -maxdepth 1 -name "$app_pid-*.json" -print -quit 2>/dev/null || true)
  if [[ -n "$candidate" ]]; then
    descriptor=$candidate
    break
  fi
  kill -0 "$app_pid" 2>/dev/null || {
    printf 'Chuzz exited before publishing its control descriptor\n' >&2
    exit 1
  }
  sleep 0.1
done
if [[ -z "$descriptor" ]]; then
  printf 'Chuzz published no control descriptor within 20 seconds\n' >&2
  exit 1
fi

for _ in {1..100}; do
  if "$qa_bin" --descriptor "$descriptor" find 'Search or enter address' --painted --count \
    > "$artifacts/ready.txt" 2>&1 && grep -q 'matched: 1' "$artifacts/ready.txt"; then
    break
  fi
  sleep 0.1
done
grep -q 'matched: 1' "$artifacts/ready.txt"

"$qa_bin" --descriptor "$descriptor" --app "$root/ps-qa.ron" qa \
  --checks "$root/tests/ps-qa" --require-paint-events \
  --pixel-artifact-dir "$artifacts/pixels" | tee "$artifacts/qa.txt"

for _ in {1..10}; do
  "$qa_bin" --descriptor "$descriptor" nodes >/dev/null
done
if grep -q 'script fetch timed out' "$artifacts/chuzz.log"; then
  printf 'The redraw stress fixture reproduced a script-fetch deadlock\n' >&2
  exit 1
fi

"$qa_bin" --descriptor "$descriptor" --app "$root/ps-qa.ron" inventory \
  --require-outcomes --checks "$root/tests/ps-qa" | tee "$artifacts/inventory.txt"
grep -q '^components: 74$' "$artifacts/inventory.txt"
grep -q '^  browser,true,' "$artifacts/inventory.txt"
grep -q '^  inspector,true,' "$artifacts/inventory.txt"
grep -q '^  settings,true,' "$artifacts/inventory.txt"
"$qa_bin" --app "$root/ps-qa.ron" reconcile "$artifacts/inventory.txt" \
  --checks "$root/tests/ps-qa" | tee "$artifacts/reconcile.txt"
"$qa_bin" --descriptor "$descriptor" click '#chuzz-settings-close' \
  > "$artifacts/inventory-dismiss.txt"
"$qa_bin" --descriptor "$descriptor" find '*' --role button --hidden --painted \
  | tee "$artifacts/hidden-painted-buttons.txt"
grep -q 'matched: 0' "$artifacts/hidden-painted-buttons.txt"
"$qa_bin" --descriptor "$descriptor" ghost 64 3 | tee "$artifacts/ghost.txt"
"$qa_bin" --descriptor "$descriptor" capture '#chuzz-app' 0.25 \
  --output "$artifacts/window.ppm" | tee "$artifacts/capture.txt"

# Agent-control Off deliberately closes the socket, so it runs after every
# shared-instance assertion. The persisted state is its durable outcome.
"$qa_bin" --descriptor "$descriptor" click '#chuzz-settings' \
  > "$artifacts/lifecycle-settings-open.txt"
"$qa_bin" --descriptor "$descriptor" reveal '#chuzz-agent-control-off' \
  > "$artifacts/agent-control-reveal.txt"
"$qa_bin" --descriptor "$descriptor" click '#chuzz-agent-control-off' \
  > "$artifacts/agent-control-off.txt" 2>&1 || true
for _ in {1..30}; do
  grep -q '"inspection"[[:space:]]*:[[:space:]]*false' \
    "$qa_home/Library/Application Support/ai.chuzz.browser/diagnostics.json" && break
  sleep 0.1
done
grep -q '"inspection"[[:space:]]*:[[:space:]]*false' \
  "$qa_home/Library/Application Support/ai.chuzz.browser/diagnostics.json"
if "$qa_bin" --descriptor "$descriptor" nodes >/dev/null 2>&1; then
  printf 'Agent control was disabled but the inspection socket stayed open\n' >&2
  exit 1
fi

printf 'Visible Chuzz QA passed; artifacts: %s\n' "$artifacts"
