# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **A repository step in the branches view.** It now opens on every repository herdr has
  open — the same set the panes view groups by — with the one you summoned it from marked and
  under the cursor, so another repository's branches are one `↓` and one `Enter` away instead
  of a trip through the panes view. Rows carry how much of each repository is open and where
  it is on disk, and the path is searchable along with the name. With one repository open the
  step is skipped. Each repository is read once and cached while the picker is up.
- **An order for the branch list.** `Ctrl-O` walks `state` (the previous fixed order, still
  the default), `updated`, and `name`; `Ctrl-R` reverses the current one. What is in force
  sits beside the count and takes the accent once it is no longer the default. The order
  holds while a filter is typed — the fuzzy score decides what is in the list, not where it
  sits — and across a change of repository. Branches with no date stay at the bottom in both
  directions. See [ADR 0006](docs/adr/0006-repository-step-and-branch-order.md).
- **`←` and `→` in the panes view** — and `h`/`l`, as `↑`/`↓` are also `k`/`j` — move to the
  head of the previous or next repository: its first pane, or its first checkout with nothing
  running. One press is exactly one repository wherever in the current one you were, both
  ends wrap, and the panes in no repository count as a section. The arrows work while the
  search box has focus; the letters are text there.
- **`Ctrl-F` fetches the repository** whose branches are on screen: `git fetch origin
  --prune`, then a re-read. It is what fills in the date and the commit subject for branches
  never fetched — `git ls-remote` knows only their names — and what removes the ones deleted
  on the remote, which the list otherwise kept for ever. `⠙ fetching origin…` sits beside the
  prompt while it runs and the list stays usable; a fetch that cannot reach the remote says
  so and changes nothing.
- **A spinner beside anything the picker is waiting for**, `reading the remote…` as well as
  `fetching origin…`, turning on a clock rather than on redraws so it neither speeds up
  while you type nor stalls while you hold a key down.
- **A picker that says what it is doing.** Opening a branch — a fetch across the network,
  a checkout of a whole working tree, then the move — used to happen after the picker had
  closed its screen, so herdr's popup framed an empty box for the seconds it took and looked
  exactly like a plugin that had hung. The picker now stays where it is, keeps the
  destination list and its preview on screen, and names the step it is on with a spinner
  beside it. `Ctrl-C` stops it during the fetch, and says so; once herdr has been asked for
  a worktree it does not, because leaving then would strand the workspace herdr made. See
  [ADR 0007](docs/adr/0007-stay-up-while-working.md).
- **Closing the branches picker no longer waits for background work.** It used to join the
  remote listing before it could return, which with a fetch in flight would have meant a
  blank popup for as long as the network took.
- **Failures are shown instead of vanishing.** A step that fails holds the screen with git's
  or herdr's own words on it until you close it, and still reaches
  `herdr plugin log list`. Before, the popup simply disappeared, which looked the same as
  success.

### Changed

- **The panes view's cursor stops only where there is somewhere to go**: a pane, or a
  checkout with nothing running in it. Repository headings and checkouts that already have
  panes are stepped over — the panes listed directly under them are the answer — so reaching
  the one you want is fewer presses. Both still appear in the list.
- **Folding a repository is gone**, with it. `Enter` on a repository was the only way to fold
  one, and there is no cursor there to press it any more; the caret it used is gone from the
  heading rather than left promising something that no longer happens.
- `Esc` now goes back one step everywhere in the branches view — destination, branch,
  repository, out — rather than closing outright from the branch list.
- `Tab` in the panes view no longer refuses when the cursor is not in a repository; the
  branches view opens on its repository list either way.
- **Panes that are not inside a repository are always listed**, in their own section at the
  bottom, rather than hidden behind a toggle. They are still panes, and a picker that hides
  some of them makes you wonder which. `h` — which was that toggle — now moves between
  repositories.

### Fixed

- **A remote that cannot be reached is no longer reported as "not a git repository".** The
  git adapter treated exit code 128 as "not a repository"; 128 is git's catch-all for every
  fatal error, so a failed fetch was given a diagnosis about entirely the wrong thing. It now
  requires git to have actually said so. Reachable since 0.1.0, but only visible now that
  failures are put in front of the user.
- `Ctrl-U` empties the branch search, which the key hint and the documentation had claimed
  since 0.1.0 without it being implemented.
- A list of one no longer says "1 branches".

## [0.1.0]

First release. Two overlay pickers for herdr, backed by one Rust binary.

### Added

- **Panes view.** Every open pane, grouped as repository → worktree → pane, with agent state.
  `Enter` jumps to a pane across spaces and tabs; a worktree with no pane is listed and can
  be opened; `n` adds a pane to a checkout; repositories fold; `/` filters.
- **Branches view.** Every branch of the repository the picker was summoned from, whatever
  state it is in — running, checked out, local, remote-only, or not yet existing. Typing
  filters and offers to create a name that does not exist. Choosing a branch that is already
  open jumps to it instead of checking it out twice.
- **Destinations.** A worktree pane can go beside the pane you came from, into an existing
  tab, into an existing space, or into a new space. `Enter` `Enter` takes the first. Beside
  the list is a preview of the chosen tab with the arriving pane drawn into it, predicted
  exactly rather than sketched. A zoomed tab shows a warning instead: herdr answers a move
  into one with success and then does not move the pane, so the picker refuses up front.
- **Pull request annotations** when `gh` is installed and authenticated. Entirely optional;
  branches are read from git so the picker works offline and against non-GitHub remotes.
- `herdr-gh-nav dump`, a diagnostic that prints the resolved tree as plain text.
- **The look of herdr's session navigator.** Both pickers open as popups over the live
  session — framed by herdr in your accent colour, at the navigator's own proportions — and
  inside that frame use its search line, tree glyphs, current-row gutter, meta column,
  breadcrumb and key hint, taking the accent colour and status glyph set from your herdr
  configuration. The meta column holds the checkout path and the pane id — shortened to `~`,
  and placed just past the longest label rather than against the right edge, so a path stays
  beside its row on a wide pane — since where a checkout is on disk is what a repository
  picker cannot infer for you. `b`/`w`/`i`/`d`/`a` narrow the
  panes view to one agent state, as they do in the navigator, and rows kept only for context
  stay in the list dimmed rather than disappearing.

### Notes

- Requires herdr 0.7.4 or later. macOS and Linux; Windows is not supported yet.
- Worktrees are placed wherever herdr is configured to put them. This plugin never computes
  that path itself.

[Unreleased]: https://github.com/ShoMasegi/herdr-gh-nav/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ShoMasegi/herdr-gh-nav/releases/tag/v0.1.0
