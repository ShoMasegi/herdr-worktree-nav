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

# 2b. The plugin reads files but never writes them. Tests use temporary files through
#     NamedTempFile, which does not weaken the source rule.
if grep -rnE 'fs::write|File::create|OpenOptions' src/ >/dev/null 2>&1; then
    fail "src/ must not write files:"
    grep -rnE 'fs::write|File::create|OpenOptions' src/ >&2
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

# 5. A doc or a comment that names something names something that exists. A sentence saying
#    "held by `some_test_name`" is a claim that something elsewhere carries it, and the way
#    that claim rots is a rename: the sentence goes on pointing, at nothing, and the next
#    reader takes it on trust.
#
#    The haystack is the code with its comment lines taken out, and that is the whole of the
#    check. Searched with them in, a name written in a Rust comment finds itself and passes
#    however long ago the thing it names was renamed away — a `//` line can say
#    `totally_made_up_helper_name` and satisfy the search by being the only place it is
#    written. Only the pages under docs/ were ever really being read.
#
#    What is taken for a name of ours: anything qualified by a path, in which case the last
#    segment is what has to exist; anything in CamelCase or SCREAMING_CASE; and any
#    snake_case word carrying two underscores or more. Prose cannot be mistaken for one of
#    those, and a single backticked word can — `gone` and `merged` are what a row says, not
#    what anything is called.
haystack=$(mktemp)
needles=$(mktemp)
trap 'rm -f "$haystack" "$needles"' EXIT
find src tests -name '*.rs' -exec cat {} + | grep -vE '^[[:space:]]*//' > "$haystack"
{ find src tests -name '*.rs' -exec grep -hE '^[[:space:]]*//' {} +
  cat docs/adr/*.md docs/en/*.md docs/ja/*.md ./*.md 2>/dev/null; } > "$needles"

# Names belonging to something other than this crate, which nothing here can keep current:
# git's and herdr's environment, herdr's socket API and the internals its pages name, the
# crates the picker is built on, and a file a release ships. Adding to this list is a claim
# that the name is somebody else's — a name of ours never belongs on it.
external='ALL Cell EnterAlternateScreen GIT_TRACE GIT_TRACE2 GIT_TRACE_PERFORMANCE
HERDR_PANE_ID HERDR_PLUGIN_ROOT LANG LC_MESSAGES LeaveAlternateScreen OpenOptions
RUSTUP_TOOLCHAIN SHA256SUMS agent_not_found closed_tab_id closed_workspace_id from_name
no_foreground_client render_panel_shell set_panic_hook'

for name in $( { grep -o '`[A-Za-z_][A-Za-z_0-9]*::[A-Za-z_0-9:]*`' "$needles" \
                   | tr -d '`' | sed 's/.*:://'
                 grep -o '`[A-Za-z_][A-Za-z_0-9]*`' "$needles" | tr -d '`' \
                   | awk '/^[A-Z]/ || gsub(/_/, "_") >= 2'; } | sort -u ); do
    case " $(echo $external) " in *" $name "*) continue ;; esac
    grep -q "\b$name\b" "$haystack" || \
        fail "a doc or comment names \`$name\`, and nothing in src or tests is called that"
done

# 7. A qualified name resolves; it does not merely exist. Check 5 asks whether the last
#    segment is the name of something anywhere in the tree, which `app::collect_repos`
#    satisfied while naming a module the function is not in. rustdoc asks the real question
#    — does this link resolve from where it is written — but only of a `///` line, and only
#    where the target is reachable from there: a private function in another module is
#    neither, and a `//` line is invisible to it. So the last two segments are asked here.
#    Whatever defines the one before the end has to define the end: for `domain::sweep::judge`
#    that is src/domain/sweep.rs, and for `Refs::Unreadable` it is whichever file declares
#    `Refs`. A holder belonging to std, crossterm or a test crate goes on the list below;
#    that list is about who owns the name, not about whether the check is convenient.
external_holders='Command ExitCode File Palette anyhow env event fs std str tempfile thread'

defines() {
    grep -qE "(^|[^A-Za-z_0-9])(fn|struct|enum|trait|type|const|static|union|mod)[[:space:]]+$2([^A-Za-z_0-9]|$)|^[[:space:]]*(pub[[:space:]]+)?$2[[:space:]]*:|^[[:space:]]{4,}$2[[:space:]]*(\{|\(|,|=|$)" "$1"
}

for path in $(grep -oE '`[A-Za-z_][A-Za-z_0-9]*(::[A-Za-z_0-9]+)+`' "$needles" \
                | tr -d '`' | sort -u); do
    last=${path##*::}
    rest=${path%::*}
    holder=${rest##*::}
    [ "$holder" = "crate" ] && continue
    case " $external_holders " in *" $holder "*) continue ;; esac
    case " $(echo $external) " in *" $last "*) continue ;; esac
    files=$( { find src tests -path "*/$holder.rs" -o -path "*/$holder/mod.rs"
               grep -rlE "(^|[^A-Za-z_0-9])(pub[[:space:]]+)?(struct|enum|trait|type|union|mod)[[:space:]]+$holder([^A-Za-z_0-9]|$)" \
                   src tests --include='*.rs'
             } 2>/dev/null | sort -u )
    # A type declared in a directory module has its methods wherever that directory put
    # them: splitting one `impl` across sibling files by responsibility is the shape this
    # tree is in, and `PanesState::handle_key` is no less resolvable for living in
    # `panes/keys.rs`. So a sibling of the declaring `mod.rs` counts as the holder too.
    for file in $files; do
        case "$file" in */mod.rs)
            files="$files $(find "${file%/mod.rs}" -maxdepth 1 -name '*.rs')" ;;
        esac
    done
    files=$(printf '%s\n' $files | sort -u)
    if [ -z "$files" ]; then
        fail "a doc or comment names \`$path\`, and nothing in src or tests defines \`$holder\`"
        continue
    fi
    ok=0
    for f in $files; do defines "$f" "$last" && { ok=1; break; }; done
    [ "$ok" = 1 ] || \
        fail "a doc or comment names \`$path\`, and \`$holder\` does not carry \`$last\`"
done

# 6. What a comment may not say, because the code beside it already says it and a comment
#    repeating it is a second copy of the same fact to keep current. Both rules are in
#    CONTRIBUTING.md under "What a comment is for"; this is where they bite.
#
#    Nothing here reads a comment for sense. What it can do is catch the two shapes that
#    went wrong over and over, and both are shapes: a number, and a past tense.
comments=$(find src tests -name '*.rs' -exec awk '
    /^[ \t]*\/\// {
        text = $0
        gsub(/`[^`]*`/, "", text)

        # 6a. No measured layout number. A column count written into prose is a value with
        #     no test holding it, and it is usually sitting directly above an assertion
        #     carrying the same value, which does. Name the constant or state the rule; let
        #     the assertion keep the number. Issue numbers, ADR numbers, record filenames
        #     and version numbers are facts about things outside the code, so they stay.
        bare = text
        gsub(/#[0-9]+/, "", bare)
        gsub(/ADR [0-9]+/, "", bare)
        gsub(/[0-9]+\.[0-9.]+/, "", bare)
        gsub(/[0-9][0-9][0-9][0-9]-[a-z0-9-]+\.md/, "", bare)
        if (bare ~ /[0-9]/ &&
            (bare ~ /[Cc]olumns?|[Ww]idths?|wide|[Rr]ows?|tall/ || bare ~ /[0-9]+ to [0-9]+/))
            printf "%s:%d: a measured number belongs in a constant or an assertion, not in prose:\n    %s\n", FILENAME, FNR, $0

        # 6b. No narrating what the code used to be. git log carries that, and an ADR
        #     carries it where the decision was worth a record. What is left after the
        #     narration goes is the constraint itself, which is still true and still worth
        #     saying: not "both were wrong once" but what makes them easy to get wrong.
        if (tolower(text) ~ /used to be|what used to|this replaced|previously|originally|w(as|ere) wrong once|ha(s|ve) shipped|before this (rule|change)|used to (do|take|need|break|go|have)/)
            printf "%s:%d: what the code used to be belongs in git log or an ADR:\n    %s\n", FILENAME, FNR, $0
    }' {} +)
if [ -n "$comments" ]; then
    fail "comments say what the code says better:"
    printf '%s\n' "$comments" >&2
fi

# 8. A module is small enough to hold in your head. The cap is on code rather than on the
#    file: a page that is mostly `mod tests` is as long as its coverage is thorough, and
#    cutting tests to fit a number is the opposite of what this is for. So the count stops
#    at the first `#[cfg(test)]`, and a file that is nothing but test support is not counted
#    at all.
#
#    What the number is for is the question a long file stops you asking: what is this
#    module *for*. `src/ui/render.rs` reached four thousand lines by drawing two pickers and
#    everything under them, and nothing in it was wrong — it was simply no longer a module
#    with an answer. The cap does not say where to cut; it says when the cut is overdue, and
#    `docs/adr/0017-modules-split-by-responsibility.md` says what to cut along.
CODE_CAP=800
for file in $(find src -name '*.rs' | sort); do
    case "$file" in */tests.rs|*/fixtures.rs) continue ;; esac
    # The count stops at the test module, which is a `#[cfg(test)]` over a `mod` that opens
    # its block here. A `#[cfg(test)]` over an import, or over a `mod fixtures;` that lives
    # in a file of its own, is one test-only line in the middle of the code rather than the
    # end of it, and stopping there would leave the rest of the module unmeasured.
    code=$(awk '
        /^#\[cfg\(test\)\]$/ { held = NR; next }
        held && /^[[:space:]]*(pub([(][a-z]+[)])?[[:space:]]+)?mod[[:space:]]+[a-z_]+[[:space:]]*[{]/ {
            print held - 1
            found = 1
            exit
        }
        { held = 0 }
        END { if (!found) print NR }
    ' "$file")
    [ "$code" -le "$CODE_CAP" ] || \
        fail "$file carries $code lines of code, over the $CODE_CAP-line cap. Split it along what it is responsible for, not down the middle; see docs/adr/0017-modules-split-by-responsibility.md"
done

[ "$status" -eq 0 ] && printf 'invariants ok (version %s, rust %s)\n' "$manifest" "$pinned"
exit "$status"
