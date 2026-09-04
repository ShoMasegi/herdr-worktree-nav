# Security

## Reporting a vulnerability

Please report privately through GitHub's
[security advisories](https://github.com/ShoMasegi/herdr-worktree-nav/security/advisories/new)
rather than in a public issue. Include what you did, what happened, and what you expected.

You will get an acknowledgement within a week. This is a small project maintained in spare
time, so please allow reasonable time for a fix before disclosing.

## What this plugin can do

Worth knowing before you install it — and before you install any herdr plugin.

A herdr plugin is ordinary code that runs on your machine, with your environment and your
permissions. herdr validates the manifest and keeps each plugin's state in its own directory,
but it does not sandbox plugin code.

Specifically, herdr-worktree-nav:

- **Talks to the herdr API** over the socket at `HERDR_SOCKET_PATH`. It can see your session
  — every workspace, tab, pane, and working directory — and it can create worktrees, move
  panes, and change focus.
- **Runs `git`** in your repositories: `rev-parse`, `for-each-ref`, `remote get-url`,
  `ls-remote`, `fetch`, and `worktree remove`. `fetch` writes only under
  `refs/remotes/origin/`, and with `--prune` — which is what `Ctrl-F` runs — it also deletes
  the refs under there whose branch is gone from the remote. Those are a cache of the remote
  and the next fetch rebuilds them.
- **Deletes a checkout, and only when you say so.** `Shift-D` in the panes view asks before
  it runs `git worktree remove` on the checkout under the cursor — or, on a pane, the
  checkout that pane is in. There is no `--force`: git
  refuses a checkout holding uncommitted changes or untracked files, and this plugin does not
  override that. The branch is left alone.
- **Closes panes, and says so first.** A checkout with panes in it has them closed before it
  is removed, which stops whatever was running in them — an agent mid-task included. The
  question lists every pane that will stop, or, in a pane too short for the list, says how
  many; it is never asked without saying that panes will close. A checkout that is holding
  uncommitted work, or whose working tree git has not yet been able to read, is refused
  before the question rather than after its panes have gone.
- **That removal outlives the picker.** It runs as a second copy of this binary, in a session
  of its own, so closing the picker does not leave a half-deleted checkout behind. It removes
  the one checkout it was given and then exits; it takes no further instructions and does
  nothing else. It reports what happened as a herdr notification.
- **Runs `gh pr list`** if `gh` is on your `PATH`, to annotate branches. Read-only.

Nothing here rewrites history, deletes a branch, changes a file in a working tree, or pushes.

It stores nothing of its own on disk — not even a preference. It does not read your
credentials, send anything anywhere, or run any command a repository supplies. Network access is
`git ls-remote` / `git fetch` against your remote, and `gh` against GitHub — both using
credentials you have already configured.

## Verifying what you install

`herdr plugin install` shows you the manifest and the commands it will run before anything
executes. Read that screen.

The install step downloads a binary from this repository's GitHub releases and verifies it
against the `SHA256SUMS` published alongside it; a mismatch falls back to building from
source rather than running the download. Release binaries are built by
[the release workflow](.github/workflows/release.yml) from a tagged commit, and the workflow
refuses to build if the tag and the manifest version disagree.
