# 4. Look like herdr's session navigator, without its palette

Status: accepted

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

The meta column follows the navigator's meaning rather than this plugin's old one: what is
happening, not where the files are. Checkout paths moved to the breadcrumb, which is where
the navigator puts that kind of context.

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
- **The floating proportions.** The navigator insets its modal and the live session shows
  through. A plugin pane with `placement = "overlay"` is a zoom: the space around an inset
  box would be blank rather than the session, so the panel fills the pane instead.
  `placement = "popup"` would float correctly, but a popup has no pane id — measured against
  0.7.4, `plugin.pane.open` returns `{"type":"ok"}` and the pane never appears in a snapshot
  — which costs the "press again to focus the picker" behaviour.
- **Hiding a single middle row.** The navigator omits the tab row when a workspace has one
  tab. A worktree row carries the branch name, which is the information the picker exists
  for; a tab number is not.
