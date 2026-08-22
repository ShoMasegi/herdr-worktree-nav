#!/bin/sh
# Keep the Japanese documentation from quietly rotting behind the English.
#
# Two checks. Every English page must have a Japanese twin with the same filename, and vice
# versa. And when a change touches an English page, it must touch its twin in the same
# change — which is the rule that actually prevents drift.
#
# Pass a base ref to enable the second check: ./scripts/check-docs-sync.sh origin/main
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"
base=${1:-}
status=0

fail() { printf '%s\n' "$*" >&2; status=1; }

for en in docs/en/*.md; do
    [ -e "$en" ] || continue
    ja="docs/ja/$(basename "$en")"
    [ -f "$ja" ] || fail "missing translation: $ja (for $en)"
done

for ja in docs/ja/*.md; do
    [ -e "$ja" ] || continue
    en="docs/en/$(basename "$ja")"
    [ -f "$en" ] || fail "orphaned translation: $ja has no $en"
done

# README.ja.md is the one pair that does not live under docs/.
if [ -f README.md ] && [ ! -f README.ja.md ]; then
    fail "missing translation: README.ja.md"
fi

if [ -n "$base" ]; then
    changed=$(git diff --name-only "$base"...HEAD -- docs/en README.md || true)
    for en in $changed; do
        case "$en" in
            README.md) ja="README.ja.md" ;;
            *)         ja="docs/ja/$(basename "$en")" ;;
        esac
        if ! git diff --name-only "$base"...HEAD -- "$ja" | grep -q .; then
            fail "$en changed but $ja did not — translations ship in the same change"
        fi
    done
fi

[ "$status" -eq 0 ] && printf 'documentation is in sync\n'
exit "$status"
