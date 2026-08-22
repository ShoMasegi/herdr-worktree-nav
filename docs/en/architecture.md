# Architecture

[日本語](../ja/architecture.md)

One Rust binary. herdr launches it three ways:

```
keybinding ──▶ herdr-gh-nav action open-panes
                   │  reads HERDR_PLUGIN_CONTEXT_JSON
                   │  forwards the invoking pane and repo as env
                   ▼
               plugin.pane.open ──▶ herdr-gh-nav pane panes   ─▶ the picker
                                    herdr-gh-nav pane branches

troubleshooting ─▶ herdr-gh-nav dump
```

The action is not the picker. It runs with the plugin directory as its working directory and
knows nothing about where you were, so its whole job is to read the context herdr gave it and
open the pane in the right place.

## Layers

```
src/
  main.rs      argv -> one of the three modes
  app/         wiring: what each herdr entry point does
  ui/          drawing and key handling
  domain/      pure logic — no I/O of any kind
  port/        the traits everything above the adapters works against
  adapter/     the only place that touches a socket or a process
```

The dependency rule runs one way: `app` and `ui` use `domain` and `port`; `domain` uses
nothing but the standard library and the port's data types; only `adapter` implements the
ports. `scripts/check-invariants.sh` enforces this, and CI runs it.

The point is that the interesting decisions are testable without a herdr server or a git
repository present. Building the tree, deciding what a branch is, and planning where a pane
goes are all pure functions over plain data.

| Module | Question it answers |
| --- | --- |
| `domain::tree` | given a snapshot and some git answers, what is the repo → worktree → pane tree? |
| `domain::rows` | which rows are visible, in what order, under this filter and these folds? |
| `domain::resolve` | what *is* this branch, and what does picking it require first? |
| `domain::order` | in what order does the branch list read, and which way round? |
| `domain::dest` | where can a pane go, and what herdr call does each choice mean? |
| `domain::preview` | what will the destination tab look like once the pane lands in it? |
| `domain::chrome` | what accent and status glyphs is herdr configured for? |

## Talking to herdr

Over the socket at `HERDR_SOCKET_PATH`, not the `herdr` CLI. The deciding factor is
`pane.focus`, which the CLI cannot express — see
[ADR 0002](../adr/0002-socket-transport.md).

The protocol is one request per connection: connect, write one JSON line, read one JSON line,
and the server closes. So `SocketHerdr` has no pooling and nothing to get wrong about
connection lifetime.

The wire types are permissive by design — every optional field defaults, unknown fields are
ignored — so a newer herdr that adds fields keeps parsing rather than failing to start.

## Building the panes view

```
herdr api snapshot ─┬─▶ workspaces (some carry .worktree: repo_key, repo_root, checkout_path)
                    ├─▶ tabs
                    └─▶ panes (cwd, agent, agent_status — but no git information)
                                │
                    ┌───────────┴────────────┐
                    ▼                        ▼
        workspace already known      git rev-parse per distinct cwd,
        to be a worktree?            eight at a time
                    └───────────┬────────────┘
                                ▼
                    worktree.list per repository
                                ▼
                          domain::tree::build
```

Two shortcuts keep it instant. Working directories are resolved once each rather than once
per pane, because several panes usually share one. And when herdr already knows a workspace
is a worktree, its answer is reused instead of running git — but only for panes still under
that checkout, since a pane is free to `cd` into a sibling repository.

Panes are matched to worktrees by checkout path, never by `open_workspace_id`. A worktree
whose pane has been moved elsewhere reports `open_workspace_id: None` while a pane is
demonstrably working in it.

## Opening a branch

Which repository comes first: the branches view lists every repository in the tree above,
marks the one the picker was summoned from, and reads that one's branches before the first
frame. The rest are read when chosen and cached for as long as the picker is open, so walking
between repositories does not re-run git. `domain::order` decides what order the branches read
in; see [ADR 0006](../adr/0006-repository-step-and-branch-order.md).

```
BranchPlan            then, whatever the plan was:
──────────
Focus      ─▶ pane.focus                     placement_for(destination)
Open       ─▶ worktree.open  ─┐                  ├─ Some ─▶ pane.move ─▶ focused
Create     ─▶ worktree.create ┼─▶ root_pane ─────┤
FetchThen… ─▶ fetch, create  ─┘                  └─ None ─▶ pane.focus
```

`worktree.create` always materialises a whole workspace — there is no way to ask for a pane
in an existing tab — so every destination except "a new space" is reached by creating and
then moving. herdr closes the emptied tab and workspace itself and leaves the checkout alone,
which is what makes this leave no residue. See
[ADR 0001](../adr/0001-delegate-worktree-creation.md).

## Looking like herdr

The pickers are drawn the way herdr's own session navigator is, reproduced from
`src/ui/navigator.rs`: the panel, the search line, the tree glyphs, the gutter, the meta
column, the breadcrumb, and the key hint. `ui::theme` holds the mapping, and the accent and
glyph set are read from herdr's configuration because its API exposes no palette. See
[ADR 0004](../adr/0004-navigator-appearance.md) for what is copied and what is not.

## Testing

| Layer | How |
| --- | --- |
| `domain` | unit tests with fake ports; every branch state and every destination |
| `ui` state | key handling is a pure state → action mapping, so the keymap is covered directly |
| `ui` drawing | `TestBackend` + `insta` snapshots of the rendered buffer |
| `adapter` git | real repositories in a `tempfile::TempDir` |
| `adapter` herdr | not testable in CI — there is no server. See [Troubleshooting](troubleshooting.md) for the manual checklist. |
