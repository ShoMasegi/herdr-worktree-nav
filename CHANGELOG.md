# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
  tab, into an existing space, or into a new space. `Enter` `Enter` takes the first.
- **Pull request annotations** when `gh` is installed and authenticated. Entirely optional;
  branches are read from git so the picker works offline and against non-GitHub remotes.
- `herdr-gh-nav dump`, a diagnostic that prints the resolved tree as plain text.
- **The look of herdr's session navigator.** Both pickers use its panel, search line, tree
  glyphs, current-row gutter, meta column, breadcrumb and key hint, and take the accent
  colour and status glyph set from your herdr configuration. `b`/`w`/`i`/`d`/`a` narrow the
  panes view to one agent state, as they do in the navigator, and rows kept only for context
  stay in the list dimmed rather than disappearing.

### Notes

- Requires herdr 0.7.4 or later. macOS and Linux; Windows is not supported yet.
- Worktrees are placed wherever herdr is configured to put them. This plugin never computes
  that path itself.

[Unreleased]: https://github.com/ShoMasegi/herdr-gh-nav/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ShoMasegi/herdr-gh-nav/releases/tag/v0.1.0
