#!/usr/bin/env bash
# Run a cargo command against the engine working checkouts instead of the pins.
#
#     scripts/local-engine.sh check -p chuzz-gui
#     scripts/local-engine.sh run -p chuzz-gui -- --wasm demo.wasm
#
# Everything after the script name is passed through to cargo unchanged.
#
# This is opt-in on purpose, and the reason is written in this repository's
# history. The engine used to be a `path = "../ps-blitz/..."` dependency, which
# meant every build was already redirected and no ordinary build ever fetched
# the revision a release would use. The revision lived somewhere else entirely,
# in a `BLITZ_REF` env var in release.yml, and it sat 44 commits behind before
# anyone noticed — because the only thing that resolves it is the release job,
# and by then it had been broken for days.
#
# Now the default exercises the pins in Cargo.toml and this is reached for only
# while engine work is in flight.
#
# Ported from AgencyZero's scripts/local-renderer.sh, which solved the same
# problem for the same reason.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
config="$root/.cargo/local-engine.toml"

if [[ ! -f "$config" ]]; then
    cat >&2 <<MSG
$config does not exist.

It is deliberately untracked: it names paths that exist on one machine, and
committing them fails every build everywhere else. Create it with a [patch]
table per git source, pointing at your checkouts:

[patch."https://github.com/pathscale/ps-blitz.git"]
ps-blitz-dom = { path = "../ps-blitz/packages/blitz-dom" }
ps-blitz-script = { path = "../ps-blitz/packages/blitz-script" }
ps-blitz-shell = { path = "../ps-blitz/packages/blitz-shell" }
ps-blitz-traits = { path = "../ps-blitz/packages/blitz-traits" }
blitz-html = { path = "../ps-blitz/packages/blitz-html" }
blitz-net = { path = "../ps-blitz/packages/blitz-net" }
blitz-paint = { path = "../ps-blitz/packages/blitz-paint" }
blitz-wasm = { path = "../ps-blitz/packages/blitz-wasm" }
dioxus-native = { path = "../ps-blitz/packages/dioxus-native" }

[patch."https://github.com/pathscale/tauri-runtime-blitz.git"]
tauri-runtime-blitz = { path = "../tauri-runtime-blitz/crates/tauri-runtime-blitz" }

Patch only what you are actually working on. Every entry you add is a crate
whose pin stops being tested.
MSG
    exit 1
fi

# Cargo rewrites Cargo.lock when a `[patch]` redirects a dependency to a path:
# the registry versions are replaced by path entries. No lockfile is committed
# here, but the file cargo writes stays on disk, and the next plain `cargo
# build` would reuse it and keep building against one machine's directories
# without saying so.
#
# Snapshot and restore on every exit path, including a failed build and a
# Ctrl-C, so the redirect cannot outlive the command. There may be no lockfile
# to snapshot on a fresh checkout, in which case the restore removes whatever
# the redirected build created.
lock="$root/Cargo.lock"
snapshot="$(mktemp)"
had_lock=0
if [ -f "$lock" ]; then
    had_lock=1
    cp "$lock" "$snapshot"
fi
restore() {
    if [ "$had_lock" -eq 1 ]; then
        cp "$snapshot" "$lock"
    else
        rm -f "$lock"
    fi
    rm -f "$snapshot"
}
trap restore EXIT INT TERM

cargo "$1" --config "$config" "${@:2}"
