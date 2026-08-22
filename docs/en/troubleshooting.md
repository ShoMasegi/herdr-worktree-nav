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
in "not in any repository", the section at the bottom of the list.

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
- [ ] The picker opens as a centred popup framed by herdr, titled `herdr-gh-nav`, with the
      session still visible around it — and it does **not** list itself.
- [ ] A worktree with no pane appears with `no pane`, and `Enter` opens it.
- [ ] `↑`/`↓` stop only on panes and on checkouts with nothing running: repository headings
      and checkouts that already have panes are stepped over, and still drawn.
- [ ] `←`/`→` move one repository per press, landing on its first pane or first idle
      checkout, wrapping at both ends and including the panes in no repository.
- [ ] `Enter` on a pane in another space lands there — and stays there after the popup
      closes.
- [ ] `n` adds a pane to the checkout under the cursor.
- [ ] `Tab` reaches the branches view, on the repository under the cursor.
- [ ] The branches view lists every repository herdr has open, marks the one it was summoned
      from, and starts with the cursor on it.
- [ ] Choosing another repository lists its branches; `Esc` goes back to the list, and going
      back into a repository already read does not run git again.
- [ ] `i` walks state, updated, and name and `Shift-I` reverses it, each landing the cursor
      on the new first row; `Ctrl-O`/`Ctrl-R` do the same while typing, and the order holds
      across a change of repository.
- [ ] The branches view opens from **inside a worktree**, not just from the plugin directory.
      This is the case that catches a relative pane command.
- [ ] A never-fetched remote branch is fetched and the worktree is based on
      `origin/<branch>`, not on `HEAD` — check `git log --oneline -1` in the new checkout.
- [ ] The branches view starts in command mode: `j`/`k` move, `f`/`o`/`r`/`q` do what the
      key hint says, and `/` is what puts letters into the search field.
- [ ] `Ctrl-F` fetches: `fetching origin…` appears, a branch that only `ls-remote` knew
      gains a date and a subject, and one deleted on the remote leaves the list.
- [ ] While that runs, the picker stays up, names the step it is on, and animates. `Ctrl-C`
      stops it during the fetch and does nothing once `working…` is shown.
- [ ] A step that fails holds the screen with git's or herdr's own words on it, closes on
      `Enter` or `Esc`, and the same message is in `herdr plugin log list`. A remote that
      cannot be reached must not be reported as "not a git repository".
- [ ] A branch already open in a pane jumps rather than checking out a second copy.
- [ ] Each destination works: split here, an existing tab, an existing space, a new space.
- [ ] After a create-and-move, `herdr workspace list` shows no leftover workspace and the
      checkout still exists on disk.
- [ ] With the herdr server stopped, the binary exits with an explanation rather than a panic.
- [ ] `Tab` leaves the border title alone (it is the plugin's name, not the view's) while the
      search line and key hint follow the view.

A popup is not addressable, so `herdr pane read` and `herdr pane send-keys` cannot drive the
picker. Run `./bin/herdr-gh-nav pane panes` in an ordinary pane to exercise the same code with
keys you can send; only the framing has to be looked at.
