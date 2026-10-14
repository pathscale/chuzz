#!/bin/bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
app=${1:-"$root/target/release/bundle/macos/Chuzz.app"}
# ps-qa must speak the same debug protocol as the Chuzz build under test. A
# sibling ps-observability checkout is built in place (its own target dir, never
# installed), so local engine work and the harness move together. PS_QA_BIN
# overrides; without a checkout, a ps-qa on PATH is used (CI installs one).
observability_dir=${PS_OBSERVABILITY_DIR:-"$root/../ps-observability"}
if [[ -n "${PS_QA_BIN:-}" ]]; then
  qa_bin=$PS_QA_BIN
elif [[ -f "$observability_dir/crates/ps-qa/Cargo.toml" ]]; then
  cargo build --release --quiet -p ps-qa \
    --manifest-path "$observability_dir/Cargo.toml" \
    --target-dir "$observability_dir/target"
  qa_bin="$observability_dir/target/release/ps-qa"
else
  qa_bin=$(command -v ps-qa || true)
fi
fixture_bin=${CHUZZ_QA_FIXTURE_BIN:-"$root/target/release/chuzz-qa-fixture"}
if [[ -n "${QA_ARTIFACT_DIR:-}" ]]; then
  artifacts=$QA_ARTIFACT_DIR
  mkdir -p "$artifacts"
else
  mkdir -p "$root/patches/out"
  artifacts=$(mktemp -d "$root/patches/out/visible-qa-XXXXXX")
fi
qa_home="$artifacts/home"
# A short private TMPDIR: the control socket lives under it, and a unix socket
# path longer than about 104 bytes is refused, which a checkout artifact dir exceeds.
private_tmp=$(mktemp -d /tmp/chuzz-qa-XXXXXX)
chmod 700 "$private_tmp"
descriptor=""
app_pid=""
server_pid=""
front_pid=""
active_qa_pid=""
active_tee_pid=""
active_qa_fifo=""
run_guarded_failure=""
qa_command_timeout=${QA_COMMAND_TIMEOUT_SECONDS:-600}
if ! [[ "$qa_command_timeout" =~ ^[1-9][0-9]*$ ]]; then
  printf 'QA_COMMAND_TIMEOUT_SECONDS must be a positive integer\n' >&2
  exit 2
fi

if [[ ! -x "$app/Contents/MacOS/chuzz-gui" ]]; then
  printf 'Chuzz executable is missing: %s\n' "$app/Contents/MacOS/chuzz-gui" >&2
  exit 1
fi
if [[ -z "$qa_bin" || ! -x "$qa_bin" ]]; then
  printf "ps-qa is missing; set PS_QA_BIN, set PS_OBSERVABILITY_DIR to a ps-observability checkout, or install ps-qa '^0.9.4'\n" >&2
  exit 1
fi
if [[ ! -x "$fixture_bin" ]]; then
  printf 'Chuzz QA fixture is missing: %s\n' "$fixture_bin" >&2
  exit 1
fi

stop_pid() {
  local process_pid=$1
  [[ -n "$process_pid" ]] || return 0

  if kill -0 "$process_pid" 2>/dev/null; then
    kill -TERM "$process_pid" 2>/dev/null || true
    for _ in {1..5}; do
      kill -0 "$process_pid" 2>/dev/null || break
      sleep 0.1
    done
    kill -KILL "$process_pid" 2>/dev/null || true
  fi
  wait "$process_pid" 2>/dev/null || true
}

stop_active_qa() {
  if [[ -n "$active_qa_pid" ]]; then
    stop_pid "$active_qa_pid"
    active_qa_pid=""
  fi
  if [[ -n "$active_tee_pid" ]]; then
    stop_pid "$active_tee_pid"
    active_tee_pid=""
  fi
}

cleanup() {
  local exit_status=$?
  trap - EXIT INT TERM
  stop_active_qa
  if [[ -n "$active_qa_fifo" ]]; then
    rm -f "$active_qa_fifo"
    active_qa_fifo=""
  fi
  if [[ -n "$app_pid" ]]; then
    stop_pid "$app_pid"
    app_pid=""
  fi
  if [[ -n "$front_pid" ]]; then
    stop_pid "$front_pid"
    front_pid=""
  fi
  if [[ -n "$server_pid" ]]; then
    stop_pid "$server_pid"
    server_pid=""
  fi
  rm -rf "$private_tmp"
  exit "$exit_status"
}

run_guarded() {
  local stage=$1
  local timeout_seconds=$2
  local log=$3
  local fifo
  local qa_pid
  local tee_pid
  local deadline
  local qa_status
  local tee_status
  local pipeline_status
  shift 3

  run_guarded_failure=""
  fifo="$log.fifo"
  if ! mkfifo "$fifo"; then
    printf 'Could not create QA output pipe for %s (log: %s)\n' "$stage" "$log" >&2
    return 1
  fi
  active_qa_fifo=$fifo

  tee "$log" <"$fifo" &
  active_tee_pid=$!
  "$@" >"$fifo" 2>&1 &
  active_qa_pid=$!
  qa_pid=$active_qa_pid
  tee_pid=$active_tee_pid
  deadline=$((SECONDS + timeout_seconds))

  while kill -0 "$qa_pid" 2>/dev/null; do
    if ! kill -0 "$app_pid" 2>/dev/null; then
      printf 'Chuzz exited during QA stage %s (app PID %s, ps-qa PID %s, log: %s)\n' \
        "$stage" "$app_pid" "$qa_pid" "$log" >&2
      run_guarded_failure=app-death
      stop_active_qa
      rm -f "$fifo"
      active_qa_fifo=""
      return 125
    fi
    if (( SECONDS >= deadline )); then
      printf 'ps-qa timed out during stage %s after %s seconds (app PID %s, ps-qa PID %s, log: %s)\n' \
        "$stage" "$timeout_seconds" "$app_pid" "$qa_pid" "$log" >&2
      run_guarded_failure=timeout
      stop_active_qa
      rm -f "$fifo"
      active_qa_fifo=""
      return 124
    fi
    sleep 0.1
  done

  if wait "$qa_pid"; then
    qa_status=0
  else
    qa_status=$?
  fi
  active_qa_pid=""

  if wait "$tee_pid"; then
    tee_status=0
  else
    tee_status=$?
  fi
  active_tee_pid=""
  rm -f "$fifo"
  active_qa_fifo=""

  if ! kill -0 "$app_pid" 2>/dev/null; then
    printf 'Chuzz exited during QA stage %s (app PID %s, ps-qa PID %s, log: %s)\n' \
      "$stage" "$app_pid" "$qa_pid" "$log" >&2
    run_guarded_failure=app-death
    return 125
  fi

  pipeline_status=0
  if (( qa_status != 0 )); then
    pipeline_status=$qa_status
  fi
  if (( tee_status != 0 )); then
    pipeline_status=$tee_status
  fi
  if (( pipeline_status != 0 )); then
    printf 'QA stage %s failed with status %s (ps-qa PID %s, log: %s)\n' \
      "$stage" "$pipeline_status" "$qa_pid" "$log" >&2
    return "$pipeline_status"
  fi
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

mkdir -p "$qa_home/Library/Application Support/ai.chuzz.browser" "$artifacts/pixels"
printf '%s\n' '{"inspection":true,"profiling":false}' \
  > "$qa_home/Library/Application Support/ai.chuzz.browser/diagnostics.json"

# Keep this run's descriptor and other temporary files inside its artifact
# directory, so the app, this script and ps-qa resolve the same location.
TMPDIR="$private_tmp"
export TMPDIR

"$fixture_bin" --port 49124 > "$artifacts/fixture.log" 2>&1 &
server_pid=$!
# Fixtures are HTTPS with ALPN h2 only, like the sites users load: HTTP/1.1
# is for WebSocket. The test CA lives in this run's private temp dir and is
# trusted only by the processes started here, through SSL_CERT_FILE.
mkdir -m 700 "$private_tmp/tls"
bun "$root/scripts/qa-h2-front.ts" --dir "$private_tmp/tls" --listen 49123 --upstream 49124 \
  > "$artifacts/h2-front.log" 2>&1 &
front_pid=$!
for _ in {1..100}; do
  grep -q '^h2 front' "$artifacts/h2-front.log" 2>/dev/null && break
  sleep 0.1
done
if ! grep -q '^h2 front' "$artifacts/h2-front.log"; then
  printf 'The HTTP/2 fixture front did not start (log: %s)\n' "$artifacts/h2-front.log" >&2
  exit 1
fi
SSL_CERT_FILE="$private_tmp/tls/ca.pem"
export SSL_CERT_FILE
HOME="$qa_home" "$app/Contents/MacOS/chuzz-gui" \
  https://127.0.0.1:49123/ > "$artifacts/chuzz.log" 2>&1 &
app_pid=$!

descriptor_dir=$TMPDIR/tauri-blitz-agent
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
  printf 'Chuzz published no control descriptor in %s within 20 seconds\n' "$descriptor_dir" >&2
  exit 1
fi

readiness_deadline=$((SECONDS + 10))
ready=0
for attempt in {1..100}; do
  if (( SECONDS >= readiness_deadline )); then
    break
  fi
  attempt_timeout=2
  remaining=$((readiness_deadline - SECONDS))
  if (( remaining < attempt_timeout )); then
    attempt_timeout=$remaining
  fi
  if (( attempt_timeout < 1 )); then
    break
  fi

  if run_guarded "readiness-$attempt" "$attempt_timeout" "$artifacts/ready.txt" \
    "$qa_bin" --descriptor "$descriptor" find 'Search or enter address' --painted --count; then
    if grep -q 'matched: 1' "$artifacts/ready.txt"; then
      ready=1
      break
    fi
  else
    qa_status=$?
    if [[ -n "$run_guarded_failure" ]]; then
      exit "$qa_status"
    fi
  fi
  sleep 0.1
done
if (( ready != 1 )); then
  printf 'Chuzz did not become ready within 10 seconds (log: %s)\n' "$artifacts/ready.txt" >&2
  exit 1
fi

run_guarded qa "$qa_command_timeout" "$artifacts/qa.txt" \
  "$qa_bin" --descriptor "$descriptor" --app "$root/ps-qa.ron" qa \
  --checks "$root/tests/ps-qa" --require-paint-events \
  --pixel-artifact-dir "$artifacts/pixels"

for node_attempt in {1..10}; do
  run_guarded "nodes-$node_attempt" "$qa_command_timeout" \
    "$artifacts/nodes-$node_attempt.txt" \
    "$qa_bin" --descriptor "$descriptor" nodes
done
if grep -q 'script fetch timed out' "$artifacts/chuzz.log"; then
  printf 'The redraw stress fixture reproduced a script-fetch deadlock\n' >&2
  exit 1
fi

run_guarded inventory "$qa_command_timeout" "$artifacts/inventory.txt" \
  "$qa_bin" --descriptor "$descriptor" --app "$root/ps-qa.ron" inventory \
  --require-outcomes --checks "$root/tests/ps-qa"
grep -q '^components: 74$' "$artifacts/inventory.txt"
grep -q '^  browser,true,' "$artifacts/inventory.txt"
grep -q '^  inspector,true,' "$artifacts/inventory.txt"
grep -q '^  settings,true,' "$artifacts/inventory.txt"
run_guarded reconcile "$qa_command_timeout" "$artifacts/reconcile.txt" \
  "$qa_bin" --app "$root/ps-qa.ron" reconcile "$artifacts/inventory.txt" \
  --checks "$root/tests/ps-qa"
run_guarded inventory-dismiss "$qa_command_timeout" "$artifacts/inventory-dismiss.txt" \
  "$qa_bin" --descriptor "$descriptor" click '#chuzz-settings-close'
run_guarded hidden-painted-buttons "$qa_command_timeout" \
  "$artifacts/hidden-painted-buttons.txt" \
  "$qa_bin" --descriptor "$descriptor" find '*' --role button --hidden --painted
grep -q 'matched: 0' "$artifacts/hidden-painted-buttons.txt"
run_guarded ghost "$qa_command_timeout" "$artifacts/ghost.txt" \
  "$qa_bin" --descriptor "$descriptor" ghost 64 3
run_guarded capture "$qa_command_timeout" "$artifacts/capture.txt" \
  "$qa_bin" --descriptor "$descriptor" capture '#chuzz-app' 0.25 \
  --output "$artifacts/window.ppm"

# The tab close suite leaves its own tabs behind, so it runs after the
# inventory and reconcile stages that expect the base suite's tab strip.
run_guarded tab-close "$qa_command_timeout" "$artifacts/tab-close-qa.txt" \
  "$qa_bin" --descriptor "$descriptor" --app "$root/ps-qa.ron" qa \
  --checks "$root/apps/chuzz/tests/checks/tab-close.ron" --require-paint-events \
  --pixel-artifact-dir "$artifacts/pixels"

# Agent-control Off deliberately closes the socket, so it runs after every
# shared-instance assertion. The persisted state is its durable outcome.
run_guarded lifecycle-settings-open "$qa_command_timeout" \
  "$artifacts/lifecycle-settings-open.txt" \
  "$qa_bin" --descriptor "$descriptor" click '#chuzz-settings'
run_guarded agent-control-reveal "$qa_command_timeout" \
  "$artifacts/agent-control-reveal.txt" \
  "$qa_bin" --descriptor "$descriptor" reveal '#chuzz-agent-control-off'
if run_guarded agent-control-off "$qa_command_timeout" \
  "$artifacts/agent-control-off.txt" \
  "$qa_bin" --descriptor "$descriptor" click '#chuzz-agent-control-off'; then
  :
else
  qa_status=$?
  if [[ -n "$run_guarded_failure" ]]; then
    exit "$qa_status"
  fi
fi
for _ in {1..30}; do
  grep -q '"inspection"[[:space:]]*:[[:space:]]*false' \
    "$qa_home/Library/Application Support/ai.chuzz.browser/diagnostics.json" && break
  sleep 0.1
done
grep -q '"inspection"[[:space:]]*:[[:space:]]*false' \
  "$qa_home/Library/Application Support/ai.chuzz.browser/diagnostics.json"
if run_guarded agent-control-disabled-socket "$qa_command_timeout" \
  "$artifacts/agent-control-disabled-socket.txt" \
  "$qa_bin" --descriptor "$descriptor" nodes; then
  printf 'Agent control was disabled but the inspection socket stayed open\n' >&2
  exit 1
else
  qa_status=$?
  if [[ -n "$run_guarded_failure" ]]; then
    exit "$qa_status"
  fi
fi

if ! kill -0 "$app_pid" 2>/dev/null; then
  printf 'Chuzz exited before visible QA completed (PID %s, log: %s)\n' \
    "$app_pid" "$artifacts/chuzz.log" >&2
  exit 1
fi
printf 'Visible Chuzz QA passed; artifacts: %s\n' "$artifacts"
