# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Every checkout says what state it is in**, beside its branch name in the panes view:
  `✱` for a working tree holding uncommitted changes or untracked files, `↑2↓1` for how far
  it has drifted from its upstream, and `gone` when the branch it tracked is no longer on the
  remote. `gone` reads in the branches view too, beside what the branch is —
  `checked out gone`, `local gone`.
- Ahead, behind and `gone` are fields on the `git for-each-ref` the picker already runs, so
  they are on screen in the first frame and cost no extra process. Whether a working tree is
  dirty cannot be: git has to walk the tree, once per checkout, so those are asked in the
  background eight at a time and each row fills in as its answer lands. A checkout that has
  not answered yet carries no marker rather than a wrong one, and the prompt line turns a
  spinner so a list that is still filling in does not read as one that found nothing. `r`
  asks again; otherwise the answers are kept for as long as the picker is open. The room a
  `✱` would take is kept from the first frame, so an answer arriving never moves the paths
  beside it.

### Changed

- **`Shift-D` no longer holds the picker while it deletes.** `git worktree remove` walks a
  whole working tree before it deletes it, which is seconds on a repository of any size, and
  it used to run between one keypress and the next — so the picker froze for it. `y` now
  starts the removal in a session of its own and comes straight back: the row's `no pane`
  becomes `deleting` with a spinner and stops being selectable, and everything else in the
  picker keeps working. Closing the picker is free, because the removal is no longer running
  inside it.
- **A removal reports itself with a herdr notification.** `removed <branch>` with the path,
  silently; or `could not remove <branch>` with git's own words and a sound. That is the
  report which still arrives when the picker has been closed, which is now the ordinary case.
  With the picker still up, a refusal is on the prompt line as well, and a success is simply
  the row leaving the list. A notification herdr declines — because they are turned off, or
  no client is attached — is accepted in silence: a refused removal leaves the checkout
  standing, so the row is there next time. See
  [ADR 0014](docs/adr/0014-removing-outlives-the-picker.md).

## [0.1.0] - 2026-08-24

First release. Two overlay pickers for herdr, backed by one Rust binary. `<Tab>` moves
between them, and they are one UI rather than two features: the picker holds the terminal for
as long as it is up, so switching redraws the screen instead of tearing it down.

### Added

#### Panes view

- **Every open pane, grouped as repository → worktree → pane**, with each agent's state.
  `Enter` jumps to a pane across spaces and tabs. A worktree with nothing running in it is
  listed too, and `Enter` opens it.
- **The cursor stops only where there is somewhere to go** — a pane, or a checkout with
  nothing running. Repository headings and checkouts that already have panes are stepped over
  and still drawn, so reaching what you want is fewer presses.
- **`←`/`→`, and `h`/`l`**, move to the head of the previous or next repository: its first
  pane, or its first idle checkout. One press is one repository wherever you were, both ends
  wrap, and panes that are in no repository count as a section of their own — they are always
  listed rather than hidden behind a toggle.
- **`n`** adds a pane to the checkout under the cursor.
- **`Shift-D` deletes a checkout that has nothing running in it**, after asking in a box over
  the list that names the branch and the path about to go, where `y` is the only key that
  answers. It runs `git worktree remove` and nothing else: the branch stays, and there is no
  `--force`, so git's refusal to throw away uncommitted work stands and is what you read. It
  is refused before the question on a pane, on a busy checkout, and on the repository's own
  checkout. The picker stays open and reloads, because tidying up comes in batches. This is
  the one thing the plugin does that cannot be undone by doing it again the other way — see
  [ADR 0008](docs/adr/0008-removing-a-worktree.md), which also records why herdr could not be
  asked to do it.
- **`b`/`w`/`i`/`d`/`a`** narrow the list to one agent state, as they do in herdr's own
  navigator. A state filter and a typed query coexist: the chip sits beside what is being
  typed.

#### Branches view

- **A repository step.** It opens on every repository herdr has open — the same set the panes
  view groups by — with the one you summoned it from marked and under the cursor, so another
  repository's branches are one `↓` and one `Enter` away. Rows carry how much of each
  repository is open and where it is on disk, and the path is searchable along with the name.
  With one repository open the step is skipped.
- **Every branch, whatever state it is in** — running, checked out, local, remote-only, or
  not yet existing. Choosing one that is already open jumps to it rather than checking it out
  twice. A never-fetched branch is fetched and the worktree is cut from `origin/<branch>`,
  not from `HEAD`. The list is headed by the repository it belongs to and where that is on
  disk.
- **`n` starts a branch from the one under the cursor.** It asks what to call it —
  `+ new branch from main: …`, naming the base rather than merely highlighting it — and then
  goes through the same destination step every other branch does. The list stays on screen
  underneath, frozen: the base is settled when `n` is pressed, so nothing can move it out
  from under the question. `Esc` from the destination comes back to the name still typed. A
  name that is empty, that git would reject, or that the repository already has is refused at
  the prompt rather than after the work has started, and a base that has only ever been on
  the remote is fetched first. Typing a name that matches nothing offers `+ create` from
  `HEAD` instead — [ADR 0013](docs/adr/0013-two-ways-to-start-a-branch.md) records why those
  two cannot be one.
- **An order for the list.** `i` walks `state` (the default), `updated`, and `name`;
  `Shift-I` reverses the current one, and `Ctrl-O`/`Ctrl-R` do the same while the search
  field has the keyboard. Either puts the cursor on the first row, because what a new order
  is for is seeing what is now at the top. What is in force sits beside the count and takes
  the accent once it is no longer the default. The order holds while a filter is typed and
  across a change of repository, and branches with no date stay at the bottom in both
  directions. See [ADR 0006](docs/adr/0006-repository-step-and-branch-order.md).
- **`Ctrl-F` fetches the repository** on screen: `git fetch origin --prune`, then a re-read.
  It fills in the date and commit subject for branches only `ls-remote` knew about, and drops
  the ones deleted on the remote. `⠙ fetching origin…` sits beside the prompt while it runs
  and the list stays usable; a fetch that cannot reach the remote says so and changes
  nothing.
- **Destinations.** A worktree pane can go beside the pane you came from, into an existing
  tab, into an existing space, or into a new space. `Enter` `Enter` takes the first. Beside
  the list is a preview of the chosen tab with the arriving pane drawn into it, predicted
  exactly rather than sketched. A zoomed tab shows a warning instead: herdr answers a move
  into one with success and then does not move the pane, so the picker refuses up front.
- **Pull request annotations** when `gh` is installed and authenticated. Entirely optional —
  branches are read from git, so the picker works offline and against non-GitHub remotes.

#### Both views

- **A command mode, and a search that is entered deliberately.** Letters are commands until
  `/` gives the search field the keyboard; `Ctrl-U` empties it and `Esc` abandons it, which
  costs nothing because the `Ctrl-` form of every command keeps working while you type. The
  `/` at the left is dim while the list has the keys and takes the accent while the field
  does, and the `search …` hint gives way to what you are typing. Rows kept only for context
  stay in the list, dimmed, rather than disappearing.
- **`Esc` goes back one step**, everywhere, rather than closing outright.
- **A picker that says what it is doing.** Opening a branch can mean a fetch across the
  network and a checkout of a whole working tree. The picker stays on screen throughout,
  keeps the destination list and its preview where they were, and names the step it is on
  with a spinner beside it — as it does for `reading the remote…` and `fetching origin…`. The
  spinner turns on a clock rather than on redraws, so it neither speeds up while you type nor
  stalls while you hold a key down. `Ctrl-C` stops the work during the fetch and says so;
  once herdr has been asked for a worktree it does not, because leaving then would strand the
  workspace herdr made. See [ADR 0007](docs/adr/0007-stay-up-while-working.md).
- **Failures are shown rather than vanishing.** A step that fails holds the screen with git's
  or herdr's own words on it until you close it, and still reaches `herdr plugin log list`.
- **Each repository's remote is asked once** and remembered for as long as the picker is up,
  `Tab` between the views included. Only the local refs — milliseconds, and the half that
  changes — are read again.
- **The look of herdr's session navigator.** Both pickers open as popups over the live
  session, framed by herdr in your accent colour at the navigator's own proportions, and
  inside that frame use its search line, tree glyphs, current-row gutter, meta column,
  breadcrumb and key hint. The accent colour and status glyph set come from your herdr
  configuration. The meta column holds the checkout path and the pane id — shortened to `~`,
  and placed just past the longest label rather than against the right edge — because where a
  checkout is on disk is what a repository picker cannot infer for you. The key hint sheds
  its least useful entry as the pane narrows, and what it sheds last is the way to the other
  view.
- **`herdr-worktree-nav dump`**, a diagnostic that prints the resolved tree as plain text, to
  tell "herdr or git said something odd" apart from "the picker drew it wrong".

### Notes

- Requires herdr 0.7.4 or later. macOS and Linux; Windows is not supported yet.
- Worktrees are placed wherever herdr is configured to put them. This plugin never computes
  that path itself.
- Installing downloads the prebuilt binary for your platform and verifies its SHA-256. On any
  miss — no matching release, no network, an unsupported platform, a checksum mismatch — it
  falls back to `cargo build --release`, so a Rust toolchain is never required but never
  needed either.

[Unreleased]: https://github.com/ShoMasegi/herdr-worktree-nav/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ShoMasegi/herdr-worktree-nav/releases/tag/v0.1.0
