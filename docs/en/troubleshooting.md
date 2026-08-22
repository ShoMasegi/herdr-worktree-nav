# Troubleshooting

[日本語](../ja/troubleshooting.md)

## Start here

```sh
herdr plugin log list --plugin herdr-gh-nav --limit 5
```

herdr records every plugin command it ran, with its exit code and stderr. A picker that did
not appear almost always left an explanation there.

```sh
herdr-gh-nav dump
```

Prints the tree the panes view would draw. If `dump` is right and the picker is wrong, the
problem is drawing; if `dump` is already wrong, it is what herdr or git reported. Run it from
inside a herdr pane — it needs `HERDR_SOCKET_PATH`.

## Nothing happens when I press the key

Check the binding arrived:

```sh
herdr plugin action list --plugin herdr-gh-nav
```

If the actions are listed but the key does nothing, the binding is the problem, not the
plugin. Confirm the `[[keys.command]]` entry is present, then `herdr server reload-config`.
Try the action directly to separate the two:

```sh
herdr plugin action invoke herdr-gh-nav.open-panes
```

## "Unable to spawn … because it does not exist"

The binary is missing from `bin/`. In a development checkout:

```sh
cargo build --release && mkdir -p bin && ln -sf ../target/release/herdr-gh-nav bin/herdr-gh-nav
```

`herdr plugin link` does not run the build step, so a linked checkout needs the binary built
by hand.

For an installed plugin, reinstall — the build step will fetch or build it:

```sh
herdr plugin uninstall herdr-gh-nav && herdr plugin install ShoMasegi/herdr-gh-nav
```

## "HERDR_SOCKET_PATH is not set"

The binary was run from a shell rather than launched by herdr. It is not a standalone tool:
it needs the socket herdr injects into plugin commands. Run it from a pane inside a herdr
session, or use the action.

## A pane is in the wrong repository, or missing

The panes view groups by the pane's working directory — the foreground process's, falling
back to the shell's. A pane that has `cd`-ed elsewhere is grouped where it now is, which is
usually what you want and occasionally surprising.

Check what herdr reports for it:

```sh
herdr pane get <pane_id>
```

If `cwd` and `foreground_cwd` are both absent, herdr cannot see into that pane and it lands
in "not in any repository". Press `h` to show that section.

## A branch I can see on GitHub is not listed

The remote list needs the network and your git credentials:

```sh
git ls-remote --heads origin
```

If that fails or hangs, so will the picker's background lookup — it gives up quietly and
leaves the local list standing. A repository with no `origin` shows only local branches by
design.

## No pull requests are shown

Optional and never fatal. Check what the picker sees:

```sh
gh auth status
gh pr list --json number,title,headRefName,isDraft
```

If `gh` is missing, unauthenticated, or the repository is not on GitHub, the column is simply
absent.

## The worktree was created somewhere I did not expect

The location is herdr's, not this plugin's:

```sh
herdr --default-config | grep -A2 '\[worktrees\]'
```

Checkouts go to `<directory>/<repo>/<branch-slug>`. To change it, set `[worktrees] directory`
in your herdr config — see [Configuration](configuration.md).

## Manual verification checklist

The herdr side cannot be tested in CI: there is no server there. Before a release, run
through this against a real session.

- [ ] `herdr plugin link .` registers both actions and both pane entrypoints
      (`herdr plugin list --json`).
- [ ] The panes view lists every open repository, grouped by worktree, and does **not** list
      its own overlay.
- [ ] A worktree with no pane appears with `no pane`, and `Enter` opens it.
- [ ] `Enter` on a pane in another space lands there — and stays there after the overlay
      closes.
- [ ] `n` adds a pane to the checkout under the cursor.
- [ ] `Tab` reaches the branches view for the repository under the cursor.
- [ ] The branches view opens from **inside a worktree**, not just from the plugin directory.
      This is the case that catches a relative pane command.
- [ ] A never-fetched remote branch is fetched and the worktree is based on
      `origin/<branch>`, not on `HEAD` — check `git log --oneline -1` in the new checkout.
- [ ] A branch already open in a pane jumps rather than checking out a second copy.
- [ ] Each destination works: split here, an existing tab, an existing space, a new space.
- [ ] After a create-and-move, `herdr workspace list` shows no leftover workspace and the
      checkout still exists on disk.
- [ ] With the herdr server stopped, the binary exits with an explanation rather than a panic.
