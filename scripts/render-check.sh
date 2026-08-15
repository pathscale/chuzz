#!/bin/sh
# Capture one or more pages headlessly and write a PNG plus a tree dump per page.
#
# Usage:
#   scripts/render-check.sh                      # the whole corpus in docs/TODO.md
#   scripts/render-check.sh 24x https://24x.ai   # one labelled page
#
# Output lands in target/render-check/<label>.{png,tree.txt,log.txt}.
#
# Three things this exists to stop repeating:
#   - `CHUZZ_CAPTURE_TREE` is a *path*, not a flag. Setting it to `1` writes a
#     file called `1` into the working directory, which is easy to miss and
#     easier to commit.
#   - macOS has no `timeout(1)`, so a page that never finishes loading hangs the
#     caller. The watchdog below is the portable stand-in.
#   - `--capture` needs the `capture` feature. It is on by default now, so a
#     plain `cargo build` carries it; the build below still names it so this
#     script keeps working for anyone building with `--no-default-features`.
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
out_dir="$repo_dir/target/render-check"
binary="$repo_dir/target/release/chuzz-gui"

width=${CHUZZ_CAPTURE_WIDTH:-1440}
height=${CHUZZ_CAPTURE_HEIGHT:-960}
scale=${CHUZZ_CAPTURE_SCALE:-2}
# Generous: a cold load with no disk cache refetches every asset. Still bounded,
# because an unbounded wait is how a corpus run turns into a hung terminal.
deadline=${RENDER_CHECK_TIMEOUT:-180}

mkdir -p "$out_dir"

if [ ! -x "$binary" ]; then
  echo "building chuzz-gui with the capture feature" >&2
  ( cd "$repo_dir" && cargo build -q -p chuzz-gui --release --features capture )
fi

capture_one() {
  label=$1
  url=$2
  png="$out_dir/$label.png"
  tree="$out_dir/$label.tree.txt"
  log="$out_dir/$label.log.txt"

  printf '%-16s %s\n' "$label" "$url"

  CHUZZ_CAPTURE_WIDTH="$width" \
  CHUZZ_CAPTURE_HEIGHT="$height" \
  CHUZZ_CAPTURE_SCALE="$scale" \
  CHUZZ_CAPTURE_TREE="$tree" \
    "$binary" --capture "$png" "$url" >"$log" 2>&1 &
  pid=$!

  # Watchdog. `kill -0` is a liveness probe, not a signal, so this polls without
  # disturbing the capture.
  waited=0
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$waited" -ge "$deadline" ]; then
      kill -9 "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
      echo "  TIMEOUT after ${deadline}s, killed" >&2
      return 1
    fi
    sleep 1
    waited=$((waited + 1))
  done
  wait "$pid" || { echo "  FAILED, see $log" >&2; return 1; }

  # A capture that never leaves a process behind is the point of running
  # headless; say so loudly if one survived anyway.
  if pgrep -f "chuzz-gui --capture" >/dev/null 2>&1; then
    echo "  WARNING: a capture process is still running" >&2
  fi

  errors=$(grep -ciE 'error|uncaught|blocked' "$log" 2>/dev/null || echo 0)
  nodes=$(sed -n 's/^# *nodes  *\([0-9]*\)/\1/p' "$tree" 2>/dev/null | head -1)
  boxed=$(sed -n 's/^# *with a box  *\([0-9]*\).*/\1/p' "$tree" 2>/dev/null | head -1)
  echo "  ${nodes:-?} nodes, ${boxed:-?} with a box, ${errors} console line(s) of interest"
}

if [ "$#" -ge 2 ]; then
  capture_one "$1" "$2"
  exit $?
fi

# The corpus from docs/TODO.md, in the order that file recommends: js.software
# first, because one showcase page covers the whole component library.
status=0
capture_one js.software     https://js.software      || status=1
capture_one 24x.ai          https://24x.ai           || status=1
capture_one pathscale.com   https://pathscale.com    || status=1
capture_one promptsyntax    https://promptsyntax.org || status=1
capture_one worktables      https://worktables.dev   || status=1
capture_one support.cafe    https://support.cafe     || status=1
capture_one honey.id        https://honey.id         || status=1
capture_one nofilter.io     https://nofilter.io      || status=1
echo
echo "output: $out_dir"
exit "$status"
