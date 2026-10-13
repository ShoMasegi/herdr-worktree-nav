# 15. Read git in one language

Status: accepted

## Context

Five things this plugin decides are decided by reading git's own words rather than its exit
code.

Whether a pane's directory is a repository at all: git answers `fatal: not a git repository (or
any of the parent directories): .git`, and `NOT_A_REPOSITORY` is that sentence as a literal.
Whether a ref was dropped from a walk: `for-each-ref` leaves the ref out, says `warning:
ignoring broken ref refs/heads/x` on stderr, and exits 0 — `dropped_refs` matches that prefix
and the neighbouring one for a ref with an illegal name. Whether `status` could look at all of
the working tree: it leaves out what it could not reach, or reads the tree without rules it
could not read, says `warning: could not open directory 'x/': Permission denied` for a
directory it could not open, `x/f: Permission denied` for a tracked file it could not `lstat`
(or whatever else the C library says of why), or `warning: unable to access 'x/.gitignore':
Permission denied` for a file of ignore rules it could not read, or of attribute rules it
needed, on stderr, and exits 0; `unread_paths` matches the two prefixes — the first not when it
ends `No such file or directory`, a directory gone between git listing its parent and opening
it; the second not when it ends `Too many levels of symbolic links`, which is what git says of
a `.gitignore` or `.gitattributes` in the tree that is a symlink, since it does not follow one
— and a line that is a path, `: `, and words starting with a capital letter, unless it opens
`warning: ` or `fatal: `. The words after the last colon are the C library's, not git's:
`LC_ALL` governs them where the library translates them at all (glibc ships translations; the
macOS one measured does not translate them). Whether a repository simply has no `origin`:
`remote get-url origin` says `error: No such remote 'origin'` on stderr and exits 2 —
`no_such_remote` matches that exit and the words `No such remote` together, so every other way
the call can fail goes up as a refusal instead of as "this repository has no remote".
Whether a branch no ref names has no commit yet: `log -1` in its checkout says `fatal: your
current branch 'main' does not have any commits yet` and exits 128, where a branch whose ref
git could not read gets `fatal: your current branch appears to be broken` — `NO_COMMITS_YET`
matches the exit and the words `does not have any commits yet` together, and anything else
keeps the checkout named on the prompt line, with git's words where it refused.

git ships translations of these messages — bar `cannot change to`, and the C library's words
after the last colon — and picks one from the environment. That environment is whatever
herdr was started in, which nothing here chooses and nothing here can predict.

Under a translation, none of git's own words matches. A missing `origin` reads as git refusing,
which is loud and wrong; a directory git could not open, or rules it could not read, go silent;
and a dropped ref goes unnoticed: the walk still exits 0, the refs it did list still build the
markers, and the checkout on the dropped ref carries none — which reads exactly like a checkout
with nothing to report, with nothing anywhere saying why. A pane that is simply not in a
repository is read as git refusing. That silence is what issue #21 is about. A fresh
repository's branch is named on the prompt line with git's translated refusal, which is loud
and wrong in the same way a missing `origin` is.

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

**A ref git drops in silence is noticed from the other side, where git will say so.** An
unreadable directory under `refs/heads`, a dangling symref: the walk exits 0 with nothing
said, in any language, and nothing in this decision changes that. Under the unreadable
directory a ref `packed-refs` also holds is not even dropped: the walk lists the packed
value, out at no checkout. What issue #32 added is a question asked of the checkout
instead: a checkout no ref is listed at — herdr naming a branch there, or naming none at a
checkout it says is not detached (`is_detached: false`), which is how herdr 0.9.3 lists
one whose ref is under the unreadable directory — gets a `log -1`, and a refusal is put on
the prompt line in git's words: `your current branch appears to be broken`, for that
directory. A herdr that omits `is_detached` has that checkout read as detached, and it
stays quiet: read the other way, every detached checkout would be on the prompt line. A
dangling symref a checkout has out is not caught: `log -1` there says the branch it points
to `does not have any commits yet`, which is a fresh repository's answer too, and so is a
branch whose ref was deleted while it was out. An unreadable ref that no checkout has out
is still silent.

## Measured

Against git 2.55.0:

```text
LC_ALL=C            warning: ignoring broken ref refs/heads/broken
LC_ALL=de_DE.UTF-8  Warnung: Ignoriere fehlerhafte Referenz refs/heads/broken
LC_ALL=C            fatal: not a git repository (or any of the parent directories): .git
LC_ALL=de_DE.UTF-8  Schwerwiegend: Kein Git-Repository (oder irgendeines der …): .git
LC_ALL=C            warning: could not open directory 'notes/': Permission denied
LC_ALL=de_DE.UTF-8  Warnung: konnte Verzeichnis 'notes/' nicht öffnen: Permission denied
LC_ALL=C            error: No such remote 'origin'
LC_ALL=de_DE.UTF-8  Fehler: Remote-Repository 'origin' nicht gefunden
LC_ALL=C            fatal: your current branch 'main' does not have any commits yet
LC_ALL=de_DE.UTF-8  Schwerwiegend: Ihr aktueller Branch 'main' hat noch keine Commits.
```

`GIT_TRACE`, `GIT_TRACE_PERFORMANCE` and `GIT_TRACE2` are read from that same environment and
each writes a timestamped line per call to stderr while listing every ref correctly, so
`dropped_refs` matches prefixes rather than asking whether stderr said anything.
