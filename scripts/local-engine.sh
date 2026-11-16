#!/usr/bin/env bash
# Run a cargo command against the engine working checkouts instead of the pins.
#
#     scripts/local-engine.sh check -p chuzz
#     scripts/local-engine.sh run -p chuzz --bin chuzz-gui -- example.com
#
# Cargo arguments are passed through, with an internal config and temporary
# manifest path added by the wrapper. Set CHUZZ_LOCAL_ENGINE_CONFIG to use
# another Cargo config instead of the checkout's .cargo/local-engine.toml.
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
config="${CHUZZ_LOCAL_ENGINE_CONFIG:-$root/.cargo/local-engine.toml}"
workspace_manifest="$root/Cargo.toml"
app_manifest="$root/apps/chuzz/Cargo.toml"
# Keep outputs usable after the disposable manifests are removed. Respect a
# caller's explicit target directory when one is already set.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}"
# A caller can supply a command runner for resource and concurrency policy.
# It must forward every supplied argument to Cargo with the same boundaries
# and order, including the generated --manifest-path and --config, wait for
# Cargo to finish, and propagate Cargo's status. It must not substitute a
# checkout manifest or separately run Cargo against the checkout.
run_cargo() {
    if [[ -n "${CHUZZ_CARGO_RUNNER:-}" ]]; then
        "$CHUZZ_CARGO_RUNNER" "$@"
    else
        cargo "$@"
    fi
}

if [[ ! -f "$config" ]]; then
    cat >&2 <<MSG
$config does not exist.

It is deliberately untracked: it names paths that exist on one machine, and
committing them fails every build everywhere else. Create it with a [patch]
table per git source, pointing at your checkouts. For Ekojit, point the patches
at crates/ekojit and crates/blitz-script-ekojit inside your Ekojit checkout.

[patch.crates-io]
ps-blitz-dom = { path = "../ps-blitz/packages/blitz-dom" }
ps-blitz-html = { path = "../ps-blitz/packages/blitz-html" }
ps-blitz-net = { path = "../ps-blitz/packages/blitz-net" }
ps-blitz-paint = { path = "../ps-blitz/packages/blitz-paint" }
ps-blitz-script = { path = "../ps-blitz/packages/blitz-script" }
ps-blitz-traits = { path = "../ps-blitz/packages/blitz-traits" }

ps-dioxus-native = { path = "../ps-blitz/packages/dioxus-native" }
izumo = { path = "../izumo/crates/izumo" }

Patch only what you are actually working on. Every entry you add is a crate
whose pin stops being tested.
MSG
    exit 1
fi

# The Ekojit crates are not published yet. Keep their dependency declarations
# out of the tracked manifests, and add them only while this opt-in command is
# running. A temporary workspace contains copied manifests and symlinked
# package files, preserving package-relative source and target paths.
overlay="$(mktemp -d "${TMPDIR:-/tmp}/chuzz-local-engine.XXXXXX")"
cleanup() {
    rm -rf "$overlay"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
root_snapshot="$overlay/source-Cargo.toml"
app_snapshot="$overlay/source-chuzz-Cargo.toml"
cp "$workspace_manifest" "$root_snapshot"
cp "$app_manifest" "$app_snapshot"
overlay_root="$overlay/Cargo.toml"
overlay_app="$overlay/apps/chuzz/Cargo.toml"

# Recreate the workspace layout with copied manifests and symlinked package
# contents. Cargo therefore reads the injected declarations from temporary
# manifests while Rust sources and other package files remain the real ones.
for member in apps/chuzz crates/chuzz-control crates/chuzz-qa-fixture; do
    mkdir -p "$overlay/$member"
    find "$root/$member" -mindepth 1 -maxdepth 1 ! -name Cargo.toml \
        -exec ln -s {} "$overlay/$member/" \;
done
cp "$root/crates/chuzz-control/Cargo.toml" "$overlay/crates/chuzz-control/Cargo.toml"
cp "$root/crates/chuzz-qa-fixture/Cargo.toml" "$overlay/crates/chuzz-qa-fixture/Cargo.toml"

awk '
    /^blitz-script = \{ package = "ps-blitz-script"/ {
        print
        print "blitz-script-ekojit = { package = \"ps-blitz-script-ekojit\", version = \"^0.1\" }"
        print "ekojit = { version = \"^0.1\" }"
        found++
        next
    }
    { print }
    END { if (found != 1) exit 42 }
' "$root_snapshot" > "$overlay_root" || {
    printf 'Could not create the local Ekojit workspace manifest overlay.\n' >&2
    exit 1
}

cp "$app_snapshot" "$overlay_app"
awk '
    /^  "dep:blitz-script",$/ {
        print
        print "  \"dep:blitz-script-ekojit\","
        print "  \"dep:ekojit\","
        feature++
        next
    }
    /^system-fonts = \["blitz-dom\/system-fonts", "blitz-script\?\/system-fonts"\]$/ {
        print "system-fonts = [\"blitz-dom/system-fonts\", \"blitz-script?/system-fonts\", \"blitz-script-ekojit?/system-fonts\"]"
        system_fonts++
        next
    }
    /^blitz-script = \{ workspace = true, optional = true \}$/ {
        print
        print "blitz-script-ekojit = { workspace = true, optional = true }"
        print "ekojit = { workspace = true, optional = true }"
        dependency++
        next
    }
    { print }
    END { if (feature != 1 || system_fonts != 1 || dependency != 1) exit 42 }
' "$app_snapshot" > "$overlay_app" || {
    printf 'Could not create the local Ekojit package manifest overlay.\n' >&2
    exit 1
}

# Cargo uses the generated manifest and writes its lockfile into the overlay
# unless the selected config redirects it with resolver.lockfile-path.
command="$1"
shift
if [ "$command" = fmt ]; then
    run_cargo --config "$config" "$command" --manifest-path "$overlay/Cargo.toml" "$@"
else
    run_cargo "$command" --config "$config" --manifest-path "$overlay/Cargo.toml" "$@"
fi
