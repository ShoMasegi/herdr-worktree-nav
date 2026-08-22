#!/bin/sh
# The architecture invariants from CLAUDE.md, as a gate rather than a promise.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"
status=0

fail() { printf '%s\n' "$*" >&2; status=1; }

# 1. src/domain is pure: no processes, no filesystem, no network, no environment, no clock.
if grep -rnE 'std::(process|fs|net|env)|SystemTime::now|Instant::now' src/domain/ >/dev/null 2>&1; then
    fail "src/domain must stay pure — it reached for the outside world:"
    grep -rnE 'std::(process|fs|net|env)|SystemTime::now|Instant::now' src/domain/ >&2
fi

# 2. Only adapters spawn processes. Everything else goes through a port.
offenders=$(grep -rln 'Command::new' src/ 2>/dev/null | grep -v '^src/adapter/' || true)
if [ -n "$offenders" ]; then
    fail "Command::new belongs in src/adapter/ only; found it in:"
    printf '%s\n' "$offenders" >&2
fi

# 3. The manifest and the crate agree on the version, so a release cannot ship a binary
#    whose fetch-or-build.sh looks for a different tag.
manifest=$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' herdr-plugin.toml | head -n 1)
crate=$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' Cargo.toml | head -n 1)
if [ "$manifest" != "$crate" ]; then
    fail "version mismatch: herdr-plugin.toml is $manifest, Cargo.toml is $crate"
fi

[ "$status" -eq 0 ] && printf 'invariants ok (version %s)\n' "$manifest"
exit "$status"
