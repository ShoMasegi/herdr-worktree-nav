# 4. Look like herdr's session navigator, without its palette

Status: accepted; the placement decision below is superseded by
[ADR 0005](./0005-popup-placement.md)

## Context

herdr ships a session navigator (`prefix+g`): a bordered modal listing workspaces, tabs and
panes as a tree, with a search line, a breadcrumb, and a key hint. This plugin's pickers do
the same job over a different grouping — repository, worktree, pane — and originally looked
nothing like it. Two tools solving the same problem side by side, in the same terminal, with
different visual languages.

The ask was to display things the way the navigator does.

## Decision

Adopt the navigator's presentation and keep this plugin's grouping.

Grouping by workspace and tab is already what the built-in navigator does; if that were what
you wanted, you would press `prefix+g`. The value here is seeing the session by repository
and worktree, so that stays. Everything about *how* it is drawn is taken from
`src/ui/navigator.rs`: the panel, the search line with its total on the right, the rule, the
gutter carrying `◆` for where the session currently is, the connected `├──`/`└──` tree
glyphs, the status glyph, the label, the right-hand meta column at 28/20/14 columns, the
blank line between groups, the breadcrumb drawn over a rule, the key hint, and the scrollbar.

The meta column is the one place the navigator's *meaning* is not followed. It was, at first:
the navigator puts activity there — an agent and its state, a count of panes — and checkout
paths moved to the breadcrumb. In use that turned out to be the wrong trade for this picker.
The navigator's rows are workspaces and tabs, which the user named and can already find;
these rows are checkouts, and where one is on disk is the thing you cannot infer. So the
column carries the checkout path and the pane id, as it did before, shortened to `~` and
middle-elided when it will not fit. The breadcrumb keeps the whole path, which is where an
elision can be read in full. Everything else about the column — its fixed 28/20/14 width, its
position, its quiet styling — is the navigator's.

Filtering also follows it. A row that did not match but is kept for context — the repository
above a result, the panes under a matching branch — stays in the list, dimmed. Hiding it
would leave a result with nothing to explain where it is.

## The palette

Only two values come from herdr's configuration: the accent, and the status glyph set.

herdr's navigator draws from a ten-token palette. That palette is not reachable from a
plugin — every method in `herdr api schema` was checked and none exposes the theme — so
mirroring it means copying another project's design tokens and hand-following them forever.
One accent per built-in theme is worth that; ten tokens across eighteen themes, plus
`[theme.custom]` and light/dark switching, is not. Everything else uses the terminal's own
sixteen colours, which follow whatever theme the terminal is set to.

An unknown theme name falls back to cyan rather than guessing, so a herdr that ships a new
theme keeps working with an accent that is merely wrong rather than a picker that is broken.

## What is deliberately not copied

- **Row order while filtering.** The navigator keeps session order; this orders by match
  score. Fuzzy matching is permissive enough that unrelated repositories match weakly, and
  the navigator's rows are ordered by something the user already knows — a workspace list
  they built — while a repository list assembled from panes is not.
- ~~**The floating proportions.**~~ Superseded by [ADR 0005](./0005-popup-placement.md): the
  pickers are popups, herdr frames them, and the session shows through as it does under the
  navigator. The measurement that argued against this was faulty; the record explains how.
- **Hiding a single middle row.** The navigator omits the tab row when a workspace has one
  tab. A worktree row carries the branch name, which is the information the picker exists
  for; a tab number is not.
