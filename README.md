# herdr-gh-nav

**[日本語](./README.ja.md)**

A [herdr](https://herdr.dev) plugin for finding your way around a session that has grown too
big to hold in your head.

Run a few agents across a few repositories and a few worktrees, and the question stops being
"what is this agent doing" and becomes "where *is* it". herdr-gh-nav answers that, and the
other direction too: take a branch that exists on GitHub and turn it into a working pane
without leaving the keyboard.

Two pickers, one keystroke each, `Tab` between them. Each opens as a popup over the live
session and is drawn the way herdr's own session navigator is — herdr's frame, the tree
glyphs, and the accent from your herdr theme — so they read as part of herdr rather than as a
different program.

## Panes — where is everything?

Every open pane, grouped by repository and by the worktree it is checked out in.

```
┌─ herdr-gh-nav ───────────────────────────────────────────────────────────────┐
│ / search panes                                                      13 panes │
│──────────────────────────────────────────────────────────────────────────────│
│ ◆ ▾ ● ShoMasegi/herdr-gh-nav (2)                          1 working          │
│   └── ● main                                              2 panes · 1 working│
│ ◆    ├── ● claude                                         claude · working   │
│      └── · shell                                          shell              │
│                                                                              │
│   ▾ ○ ShoMasegi/harbour-backend (5)                       3 idle             │
│   ├── ○ feat/hbr-51-grant-table-privileges                5 panes · 3 idle   │
│   │  ├── ○ claude                                         claude · idle      │
│   │  └── · shell                                          shell              │
│   └── · loop-review-fix-request                           no pane            │
│                                                                              │
│ ShoMasegi/herdr-gh-nav · 1 worktree · 2 panes · ~/Workspace/herdr-gh-nav ─────│
│ ↵ jump  n new pane  ⇥ branches  / search  b/w/i/d/a states  h other  esc close│
└──────────────────────────────────────────────────────────────────────────────┘
```

`●` working `○` idle `◆` blocked `·` no agent, in whichever glyph set your herdr is set to.
`◆` in the gutter marks where the session currently is. The breadcrumb under the list carries
the fuller context for the row you are on, including the checkout path.

`Enter` goes there — across spaces, across tabs, straight to the pane. A worktree with no
pane in it is listed too, and `Enter` opens it. `b`/`w`/`i`/`d` narrow to one agent state and
`a` clears that, exactly as they do in the navigator.
pane in it is listed too, and `Enter` opens it.

## Branches — get me onto that branch

Every branch of the repository you summoned it from, whatever state it is in.

```
┌─ herdr-gh-nav ───────────────────────────────────────────────────────────────┐
│ / search branches█                                                24 branches│
│──────────────────────────────────────────────────────────────────────────────│
│   ● feat/login    running      #123 Add the login screen (draft)             │
│   ○ fix/crash     checked out  latest work on fix/crash                      │
│   · main          local        latest work on main                           │
│   ↓ feat/search   remote                                                     │
│                                                                              │
│ me/app · feat/login · open in w2:p1 · ~/.herdr/worktrees/app/feat-login ──────│
│ type to filter  ↵ choose  ⇥ panes  ctrl+u clear  esc close                   │
└──────────────────────────────────────────────────────────────────────────────┘
```

Type to filter. Type something that does not exist yet and it offers to create it. Then pick
where the pane should go:

```
here            split right
                split down
existing tab    w1  app / logs
                w5  harken / android
existing space  w1  app → new tab
new space       on its own
```

`Enter` `Enter` is the fast path: split right, beside the pane you came from.

What happens next depends on what the branch already is, which is the point:

| The branch is… | What happens |
| --- | --- |
| already open in a pane | you go there — no second checkout of work you already have |
| checked out, nothing running | the checkout opens where you asked |
| a local branch | a worktree is cut from it |
| only on the remote, never fetched | it is fetched, then cut from `origin/<branch>` |
| nothing at all | it is created from `HEAD`, then cut |

Worktrees are placed wherever herdr is configured to put them — `[worktrees] directory` in
your herdr config, `~/.herdr/worktrees` by default. This plugin does not invent its own
location.

## Install

```sh
herdr plugin install ShoMasegi/herdr-gh-nav
```

Requires herdr 0.7.4 or later, and `git`. macOS and Linux.

Installing downloads a prebuilt binary and verifies its checksum; if there is no build for
your platform it falls back to `cargo build`, which needs [Rust](https://rustup.rs).

`gh` is optional. When it is installed and authenticated, branches show their open pull
request. Nothing else depends on it, and everything works offline.

## Bind the keys

herdr plugins cannot set your keybindings for you. Add these to
`~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+g"
type = "plugin_action"
command = "herdr-gh-nav.open-panes"
description = "list open panes"

[[keys.command]]
key = "prefix+shift+b"
type = "plugin_action"
command = "herdr-gh-nav.open-branches"
description = "open a branch as a worktree"
```

Then `herdr server reload-config`.

Both actions also appear in herdr's action menu, and can be run directly:

```sh
herdr plugin action invoke herdr-gh-nav.open-panes
```

## Documentation

- [Installation](docs/en/installation.md)
- [Usage](docs/en/usage.md) — every key, and what each one does
- [Configuration](docs/en/configuration.md)
- [Architecture](docs/en/architecture.md) — how it is put together, and why
- [Troubleshooting](docs/en/troubleshooting.md)
- [Decision records](docs/adr/) — the choices a later reader would otherwise undo

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md). Development conventions live in
[CLAUDE.md](./CLAUDE.md).

## Licence

[MIT](./LICENSE)
