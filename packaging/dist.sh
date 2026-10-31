#!/bin/sh
# Package a built release binary: packaging/dist.sh <target> [out-dir]
#
# Expects target/<target>/release/kyte. Writes <out>/kyte-<target>.tar.gz
# (binary, docs, third-party notices) and its .sha256, and leaves the notices in
# <out>/third-party for the RPM and .deb built next, whose asset lists name
# dist/third-party. CI and local builds use the same script.
set -eu

target="${1:?usage: packaging/dist.sh <target> [out-dir]}"
out="${2:-dist}"
root=$(cd "$(dirname "$0")/.." && pwd)
bin="$root/target/$target/release/kyte"
[ -x "$bin" ] || { echo "dist: no binary at $bin" >&2; exit 1; }

name="kyte-$target"
stage="$out/$name"
rm -rf "$stage"
mkdir -p "$stage"
cp "$bin" "$stage/kyte"
cp "$root/LICENSE" "$root/README.md" "$stage/"
chmod 755 "$stage/kyte"
(cd "$root" && python3 packaging/third_party.py "$out/third-party")
cp -R "$out/third-party" "$stage/third-party"
tar -C "$out" -czf "$out/$name.tar.gz" "$name"
rm -rf "$stage"
cd "$out"
sha256sum "$name.tar.gz" > "$name.tar.gz.sha256"
echo "dist: $out/$name.tar.gz"
