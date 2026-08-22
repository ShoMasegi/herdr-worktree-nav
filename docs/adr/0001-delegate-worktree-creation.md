# 1. Delegate worktree creation to herdr, then move the pane

Status: accepted

## Context

The plugin has to create a worktree "wherever herdr is configured to put worktrees" and then
open it as a pane at a destination the user picks: split the current pane, an existing tab,
an existing space, or a new space.

herdr's `worktree.create` does the first half exactly right — it honours `[worktrees]
directory` from the user's config and applies herdr's own `<directory>/<repo>/<branch-slug>`
layout. It does not do the second half at all: the response type requires `workspace`, `tab`,
and `root_pane`, so it *always* materialises a whole new workspace. There is no parameter for
placing the checkout into an existing tab.

That leaves two ways to build the feature.

## Options

**Create the checkout ourselves.** Read `[worktrees] directory` out of the user's
`config.toml`, reimplement herdr's branch-name slugification, and run `git worktree add`.
Then `pane.split` wherever we like. No workspace is ever created, so nothing has to be
cleaned up.

The problem is the slug. herdr's algorithm is not documented and not exposed, so we would be
guessing at it and re-guessing every time herdr changes it. Two tools placing the "same"
worktree in two different directories is a bad failure: the user ends up with duplicate
checkouts of one branch and no obvious reason why.

**Delegate, then move.** Call `worktree.create`, take the `root_pane` it hands back, and
`pane.move` it to the chosen destination.

## Decision

Delegate, then move. Placement and naming stay herdr's business, which is the point of the
config setting in the first place.

The obvious objection is residue: a workspace is created and then immediately abandoned.
Measured against herdr 0.7.4, there is none. `pane.move` closes the emptied tab and workspace
itself and reports them back as `closed_tab_id` and `closed_workspace_id`, and the checkout
on disk is untouched. Nothing needs to call `workspace.close`.

## Consequences

- `domain::dest` only has to answer one question — *where does the root pane go* — and
  returns `None` for "a new space", because that is already what herdr did.
- After a move, herdr reports the worktree with `open_workspace_id: None` even though a pane
  is working in it: the pane now lives in a workspace that is not a worktree workspace. The
  panes view therefore matches panes to worktrees by checkout path, never by
  `open_workspace_id`.
- The user briefly sees a workspace appear and vanish. `worktree.create` is called with
  `focus: false`, so it does not steal focus while that happens.
- If a future herdr stops closing the emptied workspace, the fix is a `workspace.close` call,
  not a redesign.
- `pane.move` is not always a move. Into a zoomed tab it answers with success, `changed:
  false`, and `reason: zoomed_tab`, leaving the pane where it was. The socket adapter turns
  an unchanged move into an error, and the destination step refuses a zoomed tab before
  asking, so the worktree is never created for a move that cannot happen.
