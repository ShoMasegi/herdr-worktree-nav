# 15. Read git in one language

Status: accepted

## Context

Three things this plugin decides are decided by reading git's own words rather than its exit
code.

Whether a pane's directory is a repository at all: git answers `fatal: not a git repository
(or any of the parent directories): .git`, and `NOT_A_REPOSITORY` is that sentence as a
literal. Whether a ref was dropped from a walk: `for-each-ref` leaves the ref out, says
`warning: ignoring broken ref refs/heads/x` on stderr, and exits 0 — `dropped_refs` matches
that prefix and the neighbouring one for a ref with an illegal name. Whether `status` could
open every directory it was asked about: it leaves the directory's contents out, says
`warning: could not open directory 'x/': Permission denied` on stderr, and exits 0 —
`unread_paths` matches that prefix.

git ships translations of each message and picks one from the environment. That environment
is whatever herdr was started in, which nothing here chooses and nothing here can predict.

Under a translation, neither literal matches, and neither failure is loud. A dropped ref goes
unnoticed: the walk still exits 0, the refs it did list still build the markers, and the
checkout on the dropped ref carries none — which reads exactly like a checkout with nothing
to report, with nothing anywhere saying why. A pane that is simply not in a repository is
read as git refusing. That silence is what issue #21 is about.

## Decision

The one git this crate starts runs under `LC_ALL=C`. `GIT_LOCALE` is that pair, and
`GitCli::command` is the only place that applies it.

**`LC_ALL` rather than `LC_MESSAGES`,** because it outranks the other `LC_*` variables and
`LANG`, and the environment being overridden is one this plugin did not set.

**`LC_ALL` alone is enough, although `LANGUAGE` outranks it.** `LANGUAGE=fr
LC_ALL=de_DE.UTF-8` prints French, so on precedence alone `LANGUAGE` would need clearing
too — but `LANGUAGE` is ignored when the locale is `C`, which is the locale being set.
Measured with `LANGUAGE=de LC_ALL=de_DE.UTF-8` in the parent and `LC_ALL=C` on the child,
which printed git's English.

**Exactly one place may start a git.** A second one is a call whose stderr comes back
translated and whose warnings therefore go unread, and nothing else would notice: giving one
caller a git of its own left every unit test and every other gate green. `check-invariants.sh`
counts occurrences of `Command::new("git")` in `src/` and fails at anything but one.

## Consequences

**git's words reach the prompt line and `dump` in English**, whatever language the user
reads. Everything else this plugin writes is English too, and a sentence this side cannot
read is a sentence it cannot act on.

**The pin is testable and is tested.** `tests/git_locale.rs` runs a git that would otherwise
answer in another language and checks that it does not. It needs a locale the host's git has
a translation for, and where there is none it fails rather than skipping, because libtest
keeps a passing test's output to itself and a skip would read as a pass.

**A ref git drops in silence is still silent.** An unreadable directory under `refs/heads`,
a dangling symref: the walk exits 0 with nothing said, in any language. That is issue #32,
and it is what is left of #21 once the language is settled.

## Measured

Against git 2.55.0:

```text
LC_ALL=C            warning: ignoring broken ref refs/heads/broken
LC_ALL=de_DE.UTF-8  Warnung: Ignoriere fehlerhafte Referenz refs/heads/broken
LC_ALL=C            fatal: not a git repository (or any of the parent directories): .git
LC_ALL=de_DE.UTF-8  Schwerwiegend: Kein Git-Repository (oder irgendeines der …): .git
LC_ALL=C            warning: could not open directory 'notes/': Permission denied
LC_ALL=de_DE.UTF-8  Warnung: konnte Verzeichnis 'notes/' nicht öffnen: Permission denied
```

`GIT_TRACE`, `GIT_TRACE_PERFORMANCE` and `GIT_TRACE2` are read from that same environment and
each writes a timestamped line per call to stderr while listing every ref correctly, so
`dropped_refs` matches prefixes rather than asking whether stderr said anything.
