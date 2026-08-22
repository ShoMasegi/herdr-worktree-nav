# Installation

[日本語](../ja/installation.md)

## Requirements

- **herdr 0.7.4 or later.** Older versions are missing `WorktreeInfo.open_workspace_id` and
  the `worktree` field on the plugin invocation context, both of which this plugin relies on.
- **git.** Any version that supports `git rev-parse --path-format=absolute` (2.31+).
- macOS or Linux. See [Windows](#windows) below.

Optional:

- **[`gh`](https://cli.github.com/)**, authenticated. Adds pull request numbers and titles to
  the branch list. Nothing else uses it, and every failure path degrades to "no pull
  requests" rather than an error.

## Install

```sh
herdr plugin install ShoMasegi/herdr-worktree-nav
```

herdr shows you the manifest and the commands it will run before anything happens. Read them
— a plugin is ordinary code that runs on your machine with your environment.

Then bind the actions to keys; see [Configuration](configuration.md).

## What the build step does

`herdr plugin install` runs `scripts/fetch-or-build.sh` once, after you confirm.

1. It works out the version from `herdr-plugin.toml` and the target from `uname`.
2. It downloads `herdr-worktree-nav-<version>-<target>.tar.gz` from the matching GitHub release
   along with `SHA256SUMS`, and verifies the archive against it.
3. On **any** miss — no release, no network, an unsupported platform, a checksum mismatch —
   it falls back to `cargo build --release`.

So a Rust toolchain is not needed when a prebuilt binary exists, and installing still works
when one does not. If neither path is available the script says so and stops rather than
leaving a half-installed plugin.

## Updating

```sh
herdr plugin uninstall herdr-worktree-nav
herdr plugin install ShoMasegi/herdr-worktree-nav
```

## Uninstall

```sh
herdr plugin uninstall herdr-worktree-nav
```

Remove the `[[keys.command]]` entries you added, then `herdr server reload-config`.

Nothing outside herdr's own plugin directories is touched. Worktrees this plugin created are
ordinary git worktrees and are left alone; remove them with `git worktree remove` or herdr's
own "Delete worktree checkout" if you want them gone.

## Developing against a checkout

```sh
git clone https://github.com/ShoMasegi/herdr-worktree-nav
cd herdr-worktree-nav
cargo build --release && mkdir -p bin && ln -sf ../target/release/herdr-worktree-nav bin/herdr-worktree-nav
herdr plugin link .
```

`herdr plugin link` does **not** run the `[[build]]` step, which is why the binary is built
by hand first. After that, `cargo build --release` is enough — the symlink keeps `bin/`
pointing at the fresh binary, and the next time a picker opens it runs the new code.

To go back to the released version:

```sh
herdr plugin unlink herdr-worktree-nav
```

## Windows

Not supported in v1, for two reasons that both need real work rather than a flag:

- herdr resolves a plugin pane's relative command against its own directory on Windows, so
  the entrypoints need absolute-path launchers written for PowerShell.
- The herdr API is reached over a Unix domain socket
  ([why](../adr/0002-socket-transport.md)), which needs a different transport there.
