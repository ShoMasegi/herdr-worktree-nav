#!/bin/sh
# Produce bin/herdr-gh-nav. herdr runs this once, at `herdr plugin install`.
#
# Fast path: download the release binary matching this checkout's version and platform, and
# verify its SHA-256 against the checksums published alongside it. On ANY miss — no matching
# release, no network, an unsupported platform, a checksum mismatch — fall back to building
# from source. Installing therefore never needs a Rust toolchain when a matching release
# exists, and never hard-fails without one either.
set -eu

REPO="ShoMasegi/herdr-gh-nav"
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

log() { printf 'herdr-gh-nav: %s\n' "$*" >&2; }

version=$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' herdr-plugin.toml | head -n 1)
[ -n "$version" ] || { log "could not read the version from herdr-plugin.toml"; exit 1; }

target=""
case "$(uname -s)/$(uname -m)" in
    Darwin/arm64)          target="aarch64-apple-darwin" ;;
    Darwin/x86_64)         target="x86_64-apple-darwin" ;;
    Linux/x86_64)          target="x86_64-unknown-linux-musl" ;;
    Linux/aarch64|Linux/arm64) target="aarch64-unknown-linux-musl" ;;
    *)                     log "no prebuilt binary for $(uname -s)/$(uname -m); building from source" ;;
esac

mkdir -p bin

download() {
    [ -n "$target" ] || return 1
    command -v curl >/dev/null 2>&1 || return 1

    base="https://github.com/$REPO/releases/download/v$version"
    archive="herdr-gh-nav-$version-$target.tar.gz"
    tmp=$(mktemp -d) || return 1
    # Whatever happens next, do not leave the download lying around.
    trap 'rm -rf "$tmp"' EXIT

    # curl's own errors are suppressed: a miss here is expected and handled, and a raw
    # "curl: (56) ... 404" in the middle of `herdr plugin install` reads like a crash.
    curl -fsL --retry 2 -o "$tmp/$archive" "$base/$archive" 2>/dev/null || return 1
    curl -fsL --retry 2 -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" 2>/dev/null || return 1

    expected=$(grep " $archive\$" "$tmp/SHA256SUMS" | awk '{print $1}')
    [ -n "$expected" ] || { log "no checksum published for $archive"; return 1; }

    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$tmp/$archive" | awk '{print $1}')
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$tmp/$archive" | awk '{print $1}')
    else
        log "no sha256 tool available to verify the download"
        return 1
    fi
    [ "$actual" = "$expected" ] || { log "checksum mismatch for $archive"; return 1; }

    tar -xzf "$tmp/$archive" -C "$tmp" || return 1
    [ -f "$tmp/herdr-gh-nav" ] || return 1
    # Replace rather than write through: a development checkout may have symlinked this.
    rm -f bin/herdr-gh-nav
    mv "$tmp/herdr-gh-nav" bin/herdr-gh-nav
    chmod +x bin/herdr-gh-nav
    return 0
}

if download; then
    log "installed the prebuilt binary for $target (v$version)"
    exit 0
fi

log "building from source"
command -v cargo >/dev/null 2>&1 || {
    log "no prebuilt binary was available and cargo is not installed."
    log "Install Rust from https://rustup.rs and run \`herdr plugin install\` again."
    exit 1
}
cargo build --release
# Not a plain cp: bin/herdr-gh-nav is often a symlink to exactly this file in a checkout,
# and copying a file onto itself is at best an error and at worst a truncation.
rm -f bin/herdr-gh-nav
cp target/release/herdr-gh-nav bin/herdr-gh-nav
log "built v$version from source"
