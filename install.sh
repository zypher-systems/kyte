#!/bin/sh
# Install Kyte from its GitHub releases.
#
#   curl -fsSL https://raw.githubusercontent.com/zypher-systems/kyte/main/install.sh | sh
#
#   install.sh --from FILE   install from a downloaded kyte-*.tar.gz, as it is
#   install.sh --uninstall   remove it (your settings and file history stay)
#
# Environment:
#   KYTE_VERSION       a release tag like v0.6.0 (default: the latest release)
#   KYTE_INSTALL_DIR   where the binary goes (default: ~/.local/bin)
#   KYTE_DOWNLOAD_URL  where release files are (default: GitHub; for mirrors)
#
# `kyte --update` runs this script from the release it installs, with --from
# and KYTE_INSTALL_DIR, so both have to keep working.
#
# Linux only (x86_64, arm64; static, any distro). Every download is checked
# against signed SHA256SUMS using the key below. Needs OpenSSL 3+.

set -eu

REPO="zypher-systems/kyte"
DIR="${KYTE_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
die() { printf 'kyte install: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "needs $1"; }

from=""
while [ $# -gt 0 ]; do
    case "$1" in
        --from)
            [ $# -ge 2 ] || die "--from needs a file"
            from=$2
            shift 2
            ;;
        --uninstall)
            rm -f "$DIR/kyte"
            say "removed $DIR/kyte"
            exit 0
            ;;
        *) die "unknown option: $1" ;;
    esac
done

[ "$(uname -s)" = Linux ] || die "Kyte runs on Linux only"
case "$(uname -m)" in
    x86_64 | amd64) cpu="x86_64" ;;
    arm64 | aarch64) cpu="aarch64" ;;
    *) die "no prebuilt binary for $(uname -m); build from source: cargo install --git https://github.com/$REPO" ;;
esac
target="$cpu-unknown-linux-musl"
asset="kyte-$target.tar.gz"
need tar
need mkdir

tmp=$(mktemp -d)
stage=""
cleanup() {
    [ -z "$stage" ] || rm -f "$stage"
    rm -rf "$tmp"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

if [ -n "$from" ]; then
    [ -f "$from" ] || die "no such file: $from"
    cp "$from" "$tmp/$asset"
else
    openssl_bin="${KYTE_INSTALL_OPENSSL:-openssl}"
    need "$openssl_bin"
    if command -v curl >/dev/null 2>&1; then
        fetch() { curl -fsSL "$1" -o "$2"; }
    elif command -v wget >/dev/null 2>&1; then
        fetch() { wget -q "$1" -O "$2"; }
    else
        die "needs curl or wget"
    fi
    if [ -n "${KYTE_DOWNLOAD_URL:-}" ]; then
        base="$KYTE_DOWNLOAD_URL"
    elif [ -n "${KYTE_VERSION:-}" ]; then
        base="https://github.com/$REPO/releases/download/$KYTE_VERSION"
    else
        base="https://github.com/$REPO/releases/latest/download"
    fi

    # The trust anchor. A mirror can't replace it.
    cat > "$tmp/release-key.pem" <<'KYTE_RELEASE_KEY'
-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAM9K8TsFeIW95cW0nMBT1bgkaAxpFHw1Dr8QN4PWOhmM=
-----END PUBLIC KEY-----
KYTE_RELEASE_KEY
    grep -q 'BEGIN PUBLIC KEY' "$tmp/release-key.pem" || die "this installer has no release key"

    say "downloading $asset ${KYTE_VERSION:-(latest)}"
    fetch "$base/$asset" "$tmp/$asset" || die "download failed: $base/$asset"
    fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || die "download failed: $base/SHA256SUMS"
    fetch "$base/SHA256SUMS.sig" "$tmp/SHA256SUMS.sig" || die "release has no signature: refusing to install"
    tr -d '\r\n' < "$tmp/SHA256SUMS.sig" > "$tmp/signature.b64"
    "$openssl_bin" base64 -d -A -in "$tmp/signature.b64" -out "$tmp/signature.bin" ||
        die "invalid release signature"
    "$openssl_bin" pkeyutl -verify -pubin -inkey "$tmp/release-key.pem" -rawin \
        -in "$tmp/SHA256SUMS" -sigfile "$tmp/signature.bin" >/dev/null 2>&1 ||
        die "signature verification failed: refusing to install (OpenSSL 3+ is required)"

    want=$(grep " $asset\$" "$tmp/SHA256SUMS" | cut -d ' ' -f 1)
    [ -n "$want" ] || die "$asset is not listed in SHA256SUMS"
    need sha256sum
    got=$(sha256sum "$tmp/$asset" | cut -d ' ' -f 1)
    [ "$want" = "$got" ] || die "checksum mismatch for $asset: refusing to install"
fi

# Only the expected executable comes out of the archive.
tar -xzf "$tmp/$asset" -C "$tmp" "kyte-$target/kyte" || die "$asset has no kyte in it"
binary="$tmp/kyte-$target/kyte"
[ -f "$binary" ] && [ ! -L "$binary" ] || die "release executable is missing or linked"
mkdir -p "$DIR"
# A unique staging file can't collide with another installer, and the rename
# leaves a running Kyte working.
stage=$(mktemp "$DIR/.kyte.XXXXXX")
cp "$binary" "$stage"
chmod 755 "$stage"
mv -f "$stage" "$DIR/kyte"
stage=""

say "installed $("$DIR/kyte" --version 2>/dev/null || echo kyte) to $DIR/kyte"
case ":$PATH:" in
    *":$DIR:"*) ;;
    *) say "note: $DIR is not on your PATH. Add it, e.g. to ~/.profile:"
       say "  export PATH=\"$DIR:\$PATH\"" ;;
esac
