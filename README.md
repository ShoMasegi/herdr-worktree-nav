# herdr-worktree-nav

**[日本語](./README.ja.md)**

A [herdr](https://herdr.dev) plugin for finding your way around a session that has grown too
big to hold in your head.

Run a few agents across a few repositories and a few worktrees, and the question stops being
"what is this agent doing" and becomes "where *is* it". herdr-worktree-nav answers that, and
the other direction too: take a branch that exists on GitHub and turn it into a working pane
without leaving the keyboard.

Two pickers, one keystroke each, `Tab` between them. Each opens as a popup over the live
session and is drawn the way herdr's own session navigator is — herdr's frame, the tree
glyphs, and the accent from your herdr theme — so they read as part of herdr rather than as a
different program.

## Panes — where is everything?

Every open pane, grouped by repository and by the worktree it is checked out in.

```
┌─ herdr-worktree-nav ─────────────────────────────────────────────────────────┐
│ / search panes                                                      13 panes │
│──────────────────────────────────────────────────────────────────────────────│
│ ◆ ● ShoMasegi/herdr-worktree-nav (2)                                         │
│   └── ● main                                  ~/Workspace/herdr-worktree-nav │
│ ◆    ├── ● claude                             w7:p2                          │
│      └── · shell                              w7:p3                          │
│                                                                              │
│   ○ ShoMasegi/harbour-backend (5)                                            │
│   ├── ○ feat/hbr-51-grant-table-privileges    ~/Workspace/harbour-backend    │
│   │  ├── ○ claude                             w1:p1                          │
│   │  └── · shell                              w1:p2                          │
│   └── · loop-review-fix-request  no pane      ~/.herdr/worktrees/harbour/…   │
│                                                                              │
│ ShoMasegi/herdr-worktree-nav · main · w7:p2 · working · ~/Workspace/herdr-w… │
│ ↵ jump  n new  ←→ repo  ⇥ branches  / search  b/w/i/d/a states  esc close    │
└──────────────────────────────────────────────────────────────────────────────┘
```

`●` working `○` idle `◆` blocked `·` no agent, in whichever glyph set your herdr is set to.
`◆` in the gutter marks where the session currently is. The breadcrumb under the list carries
the fuller context for the row you are on, including the checkout path.

`Enter` goes there — across spaces, across tabs, straight to the pane. A worktree with no
pane in it is listed too, and `Enter` opens it. `←`/`→` jump to the head of the previous or
next repository. `b`/`w`/`i`/`d` narrow to one agent state and `a` clears that, exactly as
they do in the navigator.

## Branches — get me onto that branch

Which repository first — every one herdr has open, with the one you came from marked and
already under the cursor:

```
┌─ herdr-worktree-nav ─────────────────────────────────────────────────────────┐
│ / search repositories█                                          4 repositories│
│──────────────────────────────────────────────────────────────────────────────│
│ ◆ ShoMasegi/herdr-worktree-nav  1 worktree, 2 panes   ~/Workspace/herdr-work…│
│   ShoMasegi/harbour-backend     3 worktrees, 5 panes  ~/Workspace/harbour-ba…│
│   nightowl/harken               1 worktree, 1 pane    ~/Workspace/nightowl/h…│
│                                                                              │
│ ShoMasegi/herdr-worktree-nav · ~/Workspace/herdr-worktree-nav ───────────────│
│ ↵ branches  j/k move  / search  ⇥ panes  q close                             │
└──────────────────────────────────────────────────────────────────────────────┘
```

Then every branch of it, whatever state it is in:

```
┌─ herdr-worktree-nav ─────────────────────────────────────────────────────────┐
│ / search branches                                   ⇅ state ↓    24 branches │
│ me/app · ~/src/app ──────────────────────────────────────────────────────────│
│   ● feat/login    running      #123 Add the login screen (draft)             │
│   ○ fix/crash     checked out  latest work on fix/crash                      │
│   · main          local        latest work on main                           │
│   ↓ feat/search   remote                                                     │
│                                                                              │
│ feat/login · open in w2:p1 · ~/.herdr/worktrees/app/feat-login ──────────────│
│ ↵ choose  j/k move  / search  n new branch  f fetch  i order  esc back       │
└──────────────────────────────────────────────────────────────────────────────┘
```

`/` filters, and typing something that does not exist yet offers to create it. `i` walks the
orders — by state, by date, by name — and `Shift-I` reverses the current one; what is in force
sits beside the count. `f` fetches the repository. Then pick where the pane should go:

```
here            split right     w1  app / agents
                split down      ┌──────────────┬──────────────┐
existing tab    w1  app / logs  │ ● claude     │ + feat/login │
                w5  harken/…    │ w1:p1        │              │
existing space  w1  app         ├──────────────┴──────────────┤
new space       on its own      │ · shell                     │
                                │ w1:p9                       │
                                └─────────────────────────────┘
```

`Enter` `Enter` is the fast path: split right, beside the pane you came from.

The picker then stays where it is and says which step it is on — `fetching origin/feat/login`,
`creating the worktree for feat/login`, moving the pane — because a fetch and a checkout are
seconds of work and an empty box for those seconds looks like a crash. If something fails, it
holds the screen and shows what git or herdr actually said.

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
herdr plugin install ShoMasegi/herdr-worktree-nav
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
key = "prefix+f"
type = "plugin_action"
command = "herdr-worktree-nav.open-panes"
description = "list open panes"

[[keys.command]]
key = "prefix+shift+b"
type = "plugin_action"
command = "herdr-worktree-nav.open-branches"
description = "open a branch as a worktree"
```

`prefix+f` and `prefix+shift+b` are free in herdr 0.7.4; `prefix+g` is its own `goto` and
`prefix+shift+g` is `new_worktree`. Then `herdr server reload-config`.

Both actions also appear in herdr's action menu, and can be run directly:

```sh
herdr plugin action invoke herdr-worktree-nav.open-panes
```

## Documentation

- [Installation](docs/en/installation.md)
- [Usage](docs/en/usage.md) — every key, and what each one does
- [Configuration](docs/en/configuration.md)
- [Architecture](docs/en/architecture.md) — how it is put together, and why
- [Troubleshooting](docs/en/troubleshooting.md)
- [Decision records](docs/adr/) — the choices a later reader would otherwise undo

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) — the gates a pull request has to pass, and what
CI will hold you to.

## Licence

[MIT](./LICENSE)
