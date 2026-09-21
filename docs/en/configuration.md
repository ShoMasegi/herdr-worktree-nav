# Configuration

[日本語](../ja/configuration.md)

herdr-worktree-nav reads one optional configuration file of its own. It never writes this
file. herdr still controls shared behavior such as the worktree directory and appearance.

## Plugin preferences

Find the configuration directory for this plugin:

```sh
herdr plugin config-dir herdr-worktree-nav
```

Create `config.toml` in the directory that the command prints. These commands create an
empty file at the usual location:

```sh
config_dir="$(herdr plugin config-dir herdr-worktree-nav)"
mkdir -p "$config_dir"
touch "$config_dir/config.toml"
```

The usual path is `~/.config/herdr/plugins/config/herdr-worktree-nav/config.toml`.

```toml
[panes]
show_worktrees_without_panes = false
```

The default value is `true`, which preserves the current behavior. Set the value to `false`
if each panes view must start with worktrees without panes hidden.

Press `p` to reverse this preference while the picker is open. The toggle stays active
across a switch to the branches view. The plugin does not write the new value to the file.

An unknown key or invalid TOML is reported on the prompt line as `plugin config.toml:` and
then the reason, and the picker opens with the defaults. That keeps a misspelled preference
from having no visible effect, without taking the picker down.

The picker reads this file when its process starts. After each change, close the picker and
open it again. `herdr server reload-config` does not reload this plugin file.

## Keybindings

Plugins cannot set your keybindings. Add them to `~/.config/herdr/config.toml`:

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

Pick keys herdr has not already taken. In 0.7.4 `prefix+f` and `prefix+shift+b` are free,
while `prefix+g` is herdr's own `goto`, `prefix+shift+g` is `new_worktree`, `prefix+w` is
`workspace_picker`, and `prefix+b` toggles the sidebar. A binding here wins, so one that
collides silently costs you a herdr command you already had.

After a keybinding change, run `herdr server reload-config`. This command reloads herdr's
main `config.toml`, not the plugin file above.

Both actions are available from herdr's action menu without a binding, and directly:

```sh
herdr plugin action invoke herdr-worktree-nav.open-panes
herdr plugin action invoke herdr-worktree-nav.open-branches
```

A picker opens as a popup, and herdr routes every key into an open popup before its own
bindings are considered — so while one is up the keybinding cannot fire again. Close it with
`Esc` first.

## Where worktrees are created

herdr's setting, not this plugin's:

```toml
[worktrees]
directory = "~/.herdr/worktrees"
```

Checkouts are placed at `<directory>/<repo>/<branch-slug>`. This plugin asks herdr to create
them and never computes that path itself — see
[ADR 0001](../adr/0001-delegate-worktree-creation.md) for why that matters.

For sibling-style checkouts, point the directory somewhere next to your projects:

```toml
[worktrees]
directory = "~/Workspace/worktrees"
```

## Appearance

The pickers are drawn like herdr's session navigator, and two of herdr's own settings change
how they look. Neither is a setting of this plugin's: it reads yours.

```toml
[theme]
name = "catppuccin"        # the accent for the border, the selection, and repository rows

[ui]
status_indicators = "dots" # or "symbols": ● ● ● ○ ·  vs  × ◐ ✓ ○ ·
```

The accent is resolved the way herdr resolves it: an explicit `[theme.custom] accent` wins,
then a `[ui] accent` you have changed from its `cyan` default, then the theme's own accent.
A theme this plugin has not heard of falls back to cyan rather than guessing, so a herdr that
ships a new theme still works.

Everything else uses the terminal's own sixteen colours, so the pickers follow your terminal
theme instead of fighting it. herdr's palette is not reachable from a plugin — its socket API
exposes no theme at all — so matching it exactly is not on offer; see
[ADR 0004](../adr/0004-navigator-appearance.md).

`herdr-worktree-nav dump` prints what it resolved, which is the quickest way to check.

## The remote

`origin` is the remote branches are read from and never-fetched branches are fetched from.
This is not configurable in v1. A repository with no `origin` still works: the branch list is
whatever git has locally, and the `reading the remote…` line goes away.

## Pull requests

If `gh` is on `PATH` and authenticated for the repository, open pull requests are shown
against their branches and can be searched by number or title. If it is not, nothing else
changes. This layer never fails the picker — see
[ADR 0003](../adr/0003-git-first-gh-optional.md).

To check what the picker sees:

```sh
gh auth status
gh pr list --json number,title,headRefName,isDraft
```

## Environment

herdr sets these; you do not.

| Variable | Used for |
| --- | --- |
| `HERDR_SOCKET_PATH` | the API socket. Without it the binary exits with an explanation. |
| `HERDR_PLUGIN_CONTEXT_JSON` | which pane and repository the action was invoked from |
| `HERDR_PLUGIN_ROOT` | locating the binary from the pane entrypoints |
| `HERDR_PLUGIN_CONFIG_DIR` | finds the plugin file and herdr's own `config.toml` |

The action passes two of its own to the pane it opens, because a pane process cannot work
them out for itself:

| Variable | Meaning |
| --- | --- |
| `WORKTREE_NAV_FROM_PANE` | the pane the picker was summoned from |
| `WORKTREE_NAV_REPO_ROOT` | the repository it was summoned from, when herdr already knew |
