# herdr-gh-nav

**[日本語](./README.ja.md)**

A [herdr](https://herdr.dev) plugin for finding your way around a session that has grown too
big to hold in your head.

Run a few agents across a few repositories and a few worktrees, and the question stops being
"what is this agent doing" and becomes "where *is* it". herdr-gh-nav answers that, and the
other direction too: take a branch that exists on GitHub and turn it into a working pane
without leaving the keyboard.

Two overlay pickers, one keystroke each, `Tab` between them.

## Panes — where is everything?

Every open pane, grouped by repository and by the worktree it is checked out in.

```
 Panes   Branches
▾ ShoMasegi/herdr-gh-nav
  ● main                                          ~/Workspace/herdr-gh-nav
    ● claude                                                         w7:p2
    · shell                                                          w7:p3
▾ ShoMasegi/harbour-backend
  ● feat/hbr-51-grant-table-privileges     ~/Workspace/harbour-backend
    ○ claude                                                         w1:p1
    ○ claude                                                         w1:p9
  ○ loop-review-fix-request  no pane   ~/.herdr/worktrees/harbour-backend/…
▾ nightowl/harken_android
  ● feature/use-presigned-url             ~/Workspace/harken_android
    ◆ claude                                                         w5:p1

press / to filter
↵ jump  n new pane  ⇥ branches  / filter  h other  r reload  q quit
```

`●` working `○` idle `◆` blocked `·` no agent.

`Enter` goes there — across spaces, across tabs, straight to the pane. A worktree with no
pane in it is listed too, and `Enter` opens it.

## Branches — get me onto that branch

Every branch of the repository you summoned it from, whatever state it is in.

```
 Panes   Branches
● feat/login   running      #123 Add the login screen (draft)
○ fix/crash    checked out  latest work on fix/crash
· main         local        latest work on main
↓ feat/search  remote

❯ █
type to filter  ↵ choose  ⇥ panes  esc quit
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
