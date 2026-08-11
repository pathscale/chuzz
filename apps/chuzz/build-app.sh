#!/bin/sh
# Assemble Chuzz.app, so the browser can be launched from the Finder or the
# Dock instead of from a terminal.
#
# Ported from AgencyZero's apps/blitz-preview/build-app.sh, which does the same
# job for a Blitz app. A bundle is not cosmetic here: macOS reads the app name,
# the icon and the high-resolution flag from Info.plist, so a bare binary shows
# up as "chuzz" with a generic icon and cannot be given a Dock position.
set -eu

profile="${1:-release}"
case "$profile" in
  debug)
    cargo_args=""
    ;;
  release)
    cargo_args="--release"
    ;;
  *)
    echo "usage: $0 [debug|release]" >&2
    exit 2
    ;;
esac

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$script_dir/../.." && pwd)
bundle_dir="$repo_dir/target/$profile/bundle/macos/Chuzz.app"
contents_dir="$bundle_dir/Contents"
macos_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"

cd "$repo_dir"
# shellcheck disable=SC2086
cargo build -p chuzz $cargo_args

# Removed rather than overwritten: a stale file left inside Contents from an
# earlier layout would be signed along with everything else and shipped.
rm -rf "$bundle_dir"
mkdir -p "$macos_dir" "$resources_dir"
cp "$repo_dir/target/$profile/chuzz" "$macos_dir/chuzz"
cp "$script_dir/Info.plist" "$contents_dir/Info.plist"
cp "$script_dir/icons/icon.icns" "$resources_dir/icon.icns"

# Ad-hoc signing. Not a distributable signature: it satisfies the loader on
# Apple silicon, which refuses an unsigned bundle outright, and nothing more.
if command -v codesign >/dev/null 2>&1; then
  codesign --force --deep --sign - "$bundle_dir"
fi

# Finder displays the outer bundle's timestamp, but writing files only updates
# Contents. Refresh the wrapper so a rebuilt app cannot look older than the
# executable inside it.
touch "$bundle_dir"

echo "$bundle_dir"
