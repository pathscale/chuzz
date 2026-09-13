#!/usr/bin/env bash
# Build the exact Linux browser used by ps-qa and package it as a public,
# checksum-addressed release asset. Fleet sites download this once-built binary
# through the headless-host action instead of recompiling the browser in every
# repository.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
output="${1:-$root/target/headless-release}"
asset="chuzz-headless-x86_64-unknown-linux-gnu.tar.gz"

if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
    echo "package-headless.sh requires Linux x86_64" >&2
    exit 2
fi

version=$(cargo metadata --no-deps --format-version 1 --manifest-path "$root/Cargo.toml" \
    | sed -n 's/.*"name":"chuzz","version":"\([^"]*\)".*/\1/p')
if [[ -z "$version" ]]; then
    echo "could not read chuzz's workspace version" >&2
    exit 1
fi

cargo build --release --manifest-path "$root/Cargo.toml" \
    --bin chuzz-headless --no-default-features \
    --features capture,javascript,scrollbars,webp,system-fonts

if ldd "$root/target/release/chuzz-headless" | grep -q 'not found'; then
    echo "chuzz-headless has unresolved runtime libraries" >&2
    ldd "$root/target/release/chuzz-headless" >&2
    exit 1
fi

stage=$(mktemp -d)
cleanup() {
    rm -rf "$stage"
}
trap cleanup EXIT INT TERM

mkdir -p "$output"
cp "$root/target/release/chuzz-headless" "$stage/chuzz-headless"
printf '%s\n' "$version" > "$stage/VERSION"
cp "$root/LICENSE-APACHE" "$root/LICENSE-MIT" "$stage/"

COPYFILE_DISABLE=1 tar -czf "$output/$asset" -C "$stage" \
    chuzz-headless VERSION LICENSE-APACHE LICENSE-MIT
(
    cd "$output"
    sha256sum "$asset" > "$asset.sha256"
)

printf '%s\n' "$output/$asset"
