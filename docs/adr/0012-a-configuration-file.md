# 12. Read a configuration file, and still write nothing

Status: accepted

## Context

The plugin reads herdr's `config.toml` for the accent colour and the status glyphs, and
otherwise touches nothing outside herdr's own directories. It writes nothing at all: the
branch order is forgotten when the picker closes, and `SECURITY.md` says it "stores nothing
of its own on disk — not even a preference".

Two wanted things cannot be derived from herdr or from git.

**What a fresh checkout needs before it is usable.** A worktree is a bare copy of the tracked
files: no `.env`, no `node_modules`, no `.direnv`. The same three commands get retyped in the
pane that has just opened. herdr cannot know what they are, and neither can git.

**Where the repositories are.** The branches view lists the repositories herdr has open,
because that is the set it can see. A repository nobody has opened is invisible, so starting
work in one means going to a shell first — the trip this plugin exists to remove.

## Decision

One file, `config.toml`, in the directory herdr injects as `HERDR_PLUGIN_CONFIG_DIR`. The
plugin reads it and never writes it.

**herdr chose the location.** `HERDR_PLUGIN_CONFIG_DIR` is
`<herdr config>/plugins/config/<plugin id>`, which herdr creates and removes with the plugin.
`adapter/herdr_config.rs` already walks up from it to find herdr's own configuration; this
puts the variable to the use it is named for. Nothing is invented, for the same reason
worktrees go where herdr says they go (ADR 0001).

**Read, never written.** What the user *wrote* is read. What the user *did* is not recorded:
no most-recently-used order, no remembered destination, no per-repository default learned
from use. The branch order still dies with the picker.

The line is there because of what the other side of it costs. A jump-back to the previous
pane is the obvious next thing to want, and it needs a file written on every use, shared
wrongly between two herdr sessions on one machine, filling with entries that point at
worktrees which no longer exist, and therefore needing to be pruned — a small cache with its
own bugs, none of them about finding a pane. A file the user maintains has none of that. It
is wrong only if they wrote it wrong, and uninstalling leaves nothing behind.

**Not a file inside the repository.** A `.herdr-worktree-nav.toml` at the repository root
would be shared with the team and would travel with the checkout, which is the better
feature. It also means that cloning a repository and opening a worktree in it runs commands
its author chose, on your machine, with your credentials. direnv answers that with a trust
prompt per directory — and a list of trusted directories, which is state, written to disk, by
a plugin that has just decided not to have any. The cheap version of the feature costs the
invariant, so the commands live in the user's own file, next to their keybindings.

**Setup commands run in the pane, not in the plugin.** herdr's `pane split` takes a working
directory and no command, so the new pane starts a shell either way. Sending the commands to
that shell puts their output, their failure, and `Ctrl-C` where the user already is, and
keeps a three-minute `npm install` out of the picker — which under
[ADR 0007](./0007-stay-up-while-working.md) would otherwise have to stay on screen narrating
it. They are sent once the shell's prompt has appeared, because text sent to a shell that has
not finished starting is dropped.

**Repositories are found by expanding globs, not by walking.** `repos = ["~/Workspace/*",
"~/src/*/*"]` is expanded and filtered to the entries that have a `.git`. The depth is the
one the user wrote, so nothing wanders into `node_modules`, a large home directory cannot
make the popup slow, and what appears in the list is predictable from the line that produced
it. `ghq` would need no configuration at all for the people who use it, and would be a second
external tool for everyone else; `gh` is already one of those.

## Consequences

**"It writes nothing" becomes checkable rather than merely true.**
`scripts/check-invariants.sh` gains a fourth invariant: no `fs::write`, `File::create` or
`OpenOptions` anywhere under `src/`. The promise in `SECURITY.md` is then a gate, which is
what the other three invariants are for.

**Parsing is pure and lives in `domain`; finding the file lives in `adapter`,** mirroring the
split `chrome.rs` and `herdr_config.rs` already have between them.

**A missing file is the normal state.** No setup commands, and the repository list is the one
herdr has open, which is exactly what happens today. A *malformed* file is not quietly
ignored: it is reported on the prompt line the first time something needs it, and everything
that does not depend on it keeps working. A configuration that silently does nothing because
of a typo is worse than one that says it could not be read.

**The repository list now has two kinds of row.** An unopened repository has no worktrees and
no panes to count, so where an open one says `3 worktrees, 5 panes` it says `not open` — a
row that is blank there reads as a repository whose counts failed to load.

**`SECURITY.md` gains a paragraph and keeps its promise.** "It runs no command a repository
supplies" stays exactly true: the commands come from the user's own configuration file, at
the same level of trust as their shell profile. What changes is that the plugin now runs
commands of any kind, and that is worth saying plainly rather than leaving to be inferred.
