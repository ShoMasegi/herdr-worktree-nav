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

### Changed

- `Esc` now goes back one step everywhere in the branches view — destination, branch,
  repository, out — rather than closing outright from the branch list.
- `Tab` in the panes view no longer refuses when the cursor is not in a repository; the
  branches view opens on its repository list either way.

### Fixed

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
