# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **The branches view says when git left a local ref out of its list.** A branch whose local
  ref git would not read was listed as one only the remote has, and `Enter` fetched it from
  the remote rather than opening what is on disk, with nothing on screen to say why. The
  search line now carries git's words, and the line under the list says of a `↓ remote`
  branch that git would not read every local ref.

## [0.2.0] - 2026-10-18

### Added

- **`p` shows or hides worktrees that contain no pane.** The ordinary panes view shows
  these rows by default to preserve the current behavior. The optional plugin configuration
  can hide them each time the view opens. The choice stays active across a switch to the
  branches view. A sweep always shows them because they are its main cleanup candidates. A
  malformed plugin configuration file opens the picker on the defaults and says what was
  wrong on the prompt line, rather than refusing to start.

- **`Shift-S` opens a sweep**: which of these checkouts is nobody working on any more. The
  gutter becomes a box — `[x]` for a checkout the sweep would take, `[ ]` for one it would
  leave, none for one it never will — and the reason sits beside the name: `gone` for an
  upstream git can no longer find, `PR #<n> merged` when `gh` can say so. `gh` is asked when
  the sweep is entered rather than when the picker opens, it only ever widens the sweep, and
  a repository it could not answer for is named on the prompt line with its rows reading
  `PR unknown` rather than nothing — before and after you mark them — and one whose refs git
  would not read is named there ahead of it, with every checkout in it refused, since nothing
  can say whether two refs name its path; one whose branch no ref names at its path can read
  `no ref here`. A
  path herdr lists twice in one repository is never swept, since a mark could not say which
  of the two registrations it would remove. Two refs at one path take two registrations, so
  a contested checkout herdr lists once per registration is refused this way, and one listed
  once is refused too, since git rather than the row would pick the registration.
  `Space` marks and unmarks, and a mark keeps its reason at every width
  the picker supports. A mark is about the checkout it was put on, not its path: one made
  again at a marked path on another branch, or at another commit where it names no branch,
  goes back to what the sweep would suggest for it, and a checkout naming no branch that git
  names no commit for is never swept. `Enter` removes what is marked: it reads the list and
  the working trees again first, asks once with the count, and then removes each checkout
  the way `Shift-D` does and, where it has one, deletes its branch with `git branch -d` —
  never `-D`, so a branch git does not call merged stays and the notification says
  `branch kept` with git's words. See
  [ADR 0011](docs/adr/0011-what-may-be-swept.md).

- **`Shift-D` deletes a checkout that has panes in it**, which is what a finished worktree
  usually looks like. It closes every pane in the checkout first — wherever you moved them,
  across tabs and spaces — and then removes it. The key means this from a pane row too, since
  the cursor does not stop on a checkout that has panes. The question names every pane that
  stops: git protects uncommitted work and does not protect what a working agent has in
  flight. A checkout holding uncommitted work, or one git would not read, is refused before
  the question rather than after its panes have gone — as is one whose working tree git has
  not finished reading, since "nobody has asked yet" is not the answer "clean". On an empty
  checkout git still answers for itself, though some refusals hold with or without panes,
  among them a checkout a pane this row does not draw is standing in, which names that
  pane, and a path herdr lists twice in one repository or one more than one of its refs
  name, where git rather than the row picks the registration that goes, and a checkout in a
  repository whose refs git would not read, where nothing can say whether that is so. See [ADR 0010](docs/adr/0010-closing-the-panes-first.md).

- **Every checkout says what state it is in**, beside its branch name in the panes view:
  `✱` for a working tree holding uncommitted changes or untracked files, `↑2↓1` for how far
  it has drifted from its upstream, `gone` when git cannot find the ref it tracks, and `?`
  when git would not read the working tree at all. `contested` marks a branch at a checkout
  more than one of the repository's refs names, which the prompt line names as `refs disagree`, and the
  breadcrumb says so for the checkout under the cursor; under refs git read it says
  `no ref names this checkout` for a branch no ref names at its path. `gone` reads in the branches view too,
  beside what the branch is — `checked out gone`, `local gone`.
- Ahead, behind and `gone` are fields on the `git for-each-ref` the picker already runs, so
  they are on screen in the first frame and cost no extra process. Whether a working tree is
  dirty cannot be: git has to walk the tree, once per checkout, so those are asked in the
  background eight at a time and each row fills in as its answer lands. A checkout that has
  not answered yet carries no marker rather than a wrong one, and the prompt line turns a
  spinner so a list that is still filling in does not read as one that found nothing. `r`
  asks again; otherwise the answers are kept for as long as the picker is open. The room a
  `✱` would take is kept from the first frame, so an answer arriving never moves the paths
  beside it — and `?` takes the same room, so a working tree git would not read says so
  rather than passing for a clean one.

- **A repository whose refs git would not read says so.** Ahead, behind and `gone` come off
  one `for-each-ref` per repository, and when that fails every checkout in the repository
  used to lose its markers in silence — indistinguishable from a repository with nothing to
  report. The rows still draw nothing, since no marker beats a wrong one; the prompt line now
  names the repository with git's own words where the search hint was, and counts the rest.
  A ref under a directory in `refs/heads` git may not open — git leaves it out of the walk,
  or lists the value `packed-refs` still holds for it with no checkout, and exits 0 having
  said nothing — is named there too where a checkout herdr lists as not detached has it out
  and no ref names that checkout's path: by the checkout's directory, since git cannot name
  its branch, and in the words git gives when asked about it:
  ``me/app: feat2-x: fatal: your current branch appears to be broken (`git log -1 --format=%H`)``.
  The branches view opens on the same sentence for a checkout of the repository it opens
  on; its list shows that branch missing, or out nowhere at the packed value with nothing
  on the row to say it is stale. A branch herdr says is out where git lists no ref and finds
  a commit reads
  `me/app: git listed no ref at the checkout of moved`, and `dump` says
  `git found a commit here` under its row. A branch with no commit yet has no ref either,
  and stays quiet where git says that is why; so do a branch whose ref was deleted while it
  was out and one out through a symbolic ref to a branch that does not exist, which git says
  the same of, an unreadable ref no checkout has out, and one out at a checkout herdr lists
  without `is_detached`, never listed, or lists with no branch where a ref of the
  repository still names the path — `dump` says git was not asked there whether that
  branch has no commit yet.
- **`dump` prints what the markers are drawn from.** Under each checkout: the upstream it
  tracks, where it stands against it — `level`, `gone`, `↑2↓1`, `none` when there is no
  upstream and git said nothing about where it would push, or `unreadable` where git printed
  a field this side could not read — and the working tree, with git's own words where it
  would not read one. A checkout herdr never listed, which the picker shows for a pane whose
  directory it recognized, says so rather than reading as a checkout with nothing out: the
  refs git names at its path are named without a claim about which of them is out. A
  branch at a checkout more than one of the repository's refs names reads `track contested`, which is
  the picker declining to pick one of them rather than a branch level with its upstream.
  How many refs name a checkout is the picker's count; where `dump`'s own read counts
  differently, the line says both.
- **The prompt line keeps what is wrong in view, and `!` reads it whole.** A sentence about
  a state — the list could not be read again, a repository's refs would not read — is worked
  out again every frame, so it goes when the state does and no keypress, filter chip or kept
  query takes its place. A sentence about something that just happened sits beside it until
  the next key that could have answered it. As the pane narrows the line gives up spinner
  labels first, then the sentence's words to a count such as `!2`, and never goes blank.
  `!` opens a panel over the list with every such sentence whole, wrapped to the pane, during
  a sweep too; `!`, `Esc` or `q` closes it. See
  [error handling](docs/en/error-handling.md).
- **What the picker could not read is named rather than dropped.** A repository herdr would
  not list keeps a heading of its own and reads ``app: not listed: herdr rejected
  worktree.list: …`` on the prompt line, where its checkouts used to leave the screen in
  silence. Panes git could not place — a `git` that could not be started, say — read
  `3 panes not placed: git could not be run: …` instead of passing for panes in no
  repository. `dump` gains a `not listed:` section and prints the words under each pane git
  could not place.

### Changed

- **A `gh` that never answers no longer spins the sweep for ever.** Each call to `gh` is
  given five seconds — the reading included — and then given up on: the sweep's rows read
  `PR unknown`, the prompt line says `gh did not answer within 5s`, and entering the sweep
  again asks again. The branches view loses the annotation for that repository and nothing
  else.
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
  the row leaving the list — or, where the list could not be read again, a line saying that,
  since the row has not left. A removal whose process ended without a readable word is the one
  ending nobody can settle, so the picker reads the list again and says it has — and stops
  there, because a row that has gone is not proof the checkout has. A notification herdr declines — because they are turned off, or
  no client is attached — is accepted in silence: a refused removal leaves the checkout
  standing, so the row is there next time. See
  [ADR 0014](docs/adr/0014-removing-outlives-the-picker.md).

### Fixed

- **A row the list has not caught up with no longer takes the picker down.** `Enter` on a
  pane that has closed, or a checkout that has gone, used to close the popup exactly as a
  jump that worked does. herdr's refusal is now said on the prompt line — `could not go to
  that pane`, `could not open that checkout`, `could not add a pane there`, then herdr's own
  words — and the list is read again.
- **A working tree git could not wholly look at is not clean.** `git status` exits 0 when it
  could not open a directory and leaves everything under it out, and such a checkout read as
  clean, so a sweep could offer it. It now reads `?`, like one git would not read at all.
  Untracked files count whatever `status.showUntrackedFiles` is set to, since they count to
  `git worktree remove`.
- **A repository git could not name is not one with no GitHub remote.** A `.git/config` git
  could not parse, or a repository root that has gone, reached the sweep as
  `no GitHub remote to ask about`; the sweep now says `git could not name the repository:`
  with git's words.
- **Markers are about the checkout they are drawn on.** A checkout's track is read from the
  repository that owns it, so a stale worktree entry in one repository no longer puts its
  `gone` on another's checkout. A checkout with nothing out, or one herdr never listed, draws
  no track about a branch it never names. A track field git printed and the plugin could not
  read draws nothing and `dump` says `track unreadable`, and a branch level with its upstream
  draws nothing; neither is measured against where git would push instead.
- **The prompt line no longer outlives what it says.** A line saying the list could not be
  read goes once a reading lands, and the sentence about unreadable refs is no longer blank
  at narrow widths or hidden behind a filter, a kept query or a spinner.
- **git is kept to the repository it is asked about.** Where a repository's `.git/HEAD`
  cannot be read, is empty or holds garbage, git used to walk up to a repository around it
  and answer for that one without a word, so its refs and checkouts were read as the inner
  repository's and a removal would have run in the outer one. Every call but the one that
  places a pane now stops at the directory it is handed, and the repository reads as one
  git would not read.
- **A checkout git lists on neither a branch nor a detached `HEAD` is not taken to be at a
  commit.** A registration whose `HEAD` holds a cut-off id is listed under neither line; a
  sweep now refuses it with the other checkouts git names no commit for, and `dump` says why.

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

[Unreleased]: https://github.com/ShoMasegi/herdr-worktree-nav/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/ShoMasegi/herdr-worktree-nav/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ShoMasegi/herdr-worktree-nav/releases/tag/v0.1.0
