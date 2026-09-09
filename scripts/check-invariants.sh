#!/bin/sh
# The architecture invariants from CONTRIBUTING.md, as a gate rather than a promise.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"
status=0

fail() { printf '%s\n' "$*" >&2; status=1; }

# 1a. src/domain depends on nothing above it. The direction is what keeps `domain` testable
#     without a herdr or a git, and a module that drifts upward compiles perfectly well.
if grep -rnE 'use crate::(ui|app|adapter)' src/domain/ >/dev/null 2>&1; then
    fail "src/domain must not depend on a layer above it:"
    grep -rnE 'use crate::(ui|app|adapter)' src/domain/ >&2
fi

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

# 2a. Exactly one place starts a git, because that place is the only one that pins the locale
#     git's messages are matched in. A second one is a call whose stderr comes back
#     translated and whose warnings therefore go unread, and nothing else here would notice:
#     giving is_dirty a git of its own left every unit test and every other check on this
#     page green. Occurrences, not lines: two on one line would otherwise pass.
gits=$(grep -rho 'Command::new("git")' src/ 2>/dev/null | wc -l | tr -d ' ')
if [ "$gits" != "1" ]; then
    fail "src/ must start exactly one git: the one GitCli::command builds, which pins the locale. Found $gits. That function is private, so a second adapter needing a git goes through GitPort, or widens it first. Prose counts too — write it bare in comments, as gh_cli.rs does:"
    grep -rn 'Command::new("git")' src/ >&2 || true
fi

# 3. The manifest and the crate agree on the version, so a release cannot ship a binary
#    whose fetch-or-build.sh looks for a different tag.
manifest=$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' herdr-plugin.toml | head -n 1)
crate=$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' Cargo.toml | head -n 1)
if [ "$manifest" != "$crate" ]; then
    fail "version mismatch: herdr-plugin.toml is $manifest, Cargo.toml is $crate"
fi

# 4. Every workflow installs the toolchain rust-toolchain.toml pins. A floating `stable`
#    meant a Rust release could fail a pull request that changed no code at all — 1.98.0
#    turned three existing lines into clippy errors under `-D warnings`, on a branch that
#    only added documentation.
pinned=$(sed -n 's/^channel[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' rust-toolchain.toml | head -n 1)
if [ -z "$pinned" ]; then
    fail "rust-toolchain.toml pins no channel"
else
    for workflow in .github/workflows/*.yml; do
        [ -e "$workflow" ] || continue
        for used in $(sed -n 's|.*dtolnay/rust-toolchain@\([^ ]*\).*|\1|p' "$workflow"); do
            [ "$used" = "$pinned" ] || \
                fail "$workflow installs rust-toolchain@$used; rust-toolchain.toml pins $pinned"
        done
    done
fi

[ "$status" -eq 0 ] && printf 'invariants ok (version %s, rust %s)\n' "$manifest" "$pinned"
exit "$status"
