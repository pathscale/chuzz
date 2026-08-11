#!/bin/sh
# Render icon.html with Chuzz and turn it into icon.icns.
#
# Chuzz draws its own icon. That keeps the source as markup rather than an
# opaque binary, and it means a renderer regression shows up the next time the
# icon is rebuilt.
#
# Only needed when icon.html changes; icon.icns is committed, so building the
# app does not depend on this script or on a working renderer.
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$script_dir/../../.." && pwd)
work_dir=$(mktemp -d)
trap 'rm -rf "$work_dir"' EXIT

cd "$repo_dir"
CHUZZ_CAPTURE_WIDTH=1024 CHUZZ_CAPTURE_HEIGHT=1024 \
  cargo run -q -p chuzz --features capture -- \
  --capture "$work_dir/icon-1024.png" "file://$script_dir/icon.html"

iconset="$work_dir/icon.iconset"
mkdir -p "$iconset"
# The sizes `iconutil` expects, each also at @2x. 1024 is the @2x of 512, which
# is why there is no icon_1024x1024.png in the set.
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$work_dir/icon-1024.png" \
    --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$work_dir/icon-1024.png" \
    --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done

iconutil --convert icns --output "$script_dir/icon.icns" "$iconset"
echo "$script_dir/icon.icns"
