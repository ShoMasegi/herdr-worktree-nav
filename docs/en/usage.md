# Usage

[日本語](../ja/usage.md)

Two pickers, opened by two actions, toggled with `Tab`. Both are overlays: they cover the
session while they are up and get out of the way the moment they close.

Where the picker opens *from* matters. It takes the repository and the pane you were on when
you pressed the key, which is how "split here" knows where "here" is and how the branch list
knows which repository you meant.

## Panes

| Key | Does |
| --- | --- |
| `↑` `↓`, `k` `j`, `Ctrl-P` `Ctrl-N` | move |
| `Enter` on a pane | go to it |
| `Enter` on a worktree with panes | go to the first pane in it |
| `Enter` on a worktree with none | open that checkout |
| `Enter` on a repository | fold or unfold it |
| `n` | add a pane to the checkout under the cursor |
| `Tab` | branches, for the repository under the cursor |
| `/` | filter |
| `h` | show or hide panes that are not in a repository |
| `r` | reload |
| `q`, `Esc`, `Ctrl-C` | close |

While filtering, letters are text rather than commands. `Enter` keeps the filter and returns
to the keys above; `Esc` clears it.

### Reading a row

```
▾ ShoMasegi/herdr-gh-nav                          ~/Workspace/herdr-gh-nav
  ● main                                          ~/Workspace/herdr-gh-nav
    ● claude                                                         w7:p2
  ○ fix/crash  no pane           ~/.herdr/worktrees/herdr-gh-nav/fix-crash
```

- The repository is named `owner/repo` when its `origin` is on GitHub, and by its directory
  name otherwise. Its path is shown only while it is folded — expanded, the main checkout
  directly beneath already carries it.
- `●` marks the main checkout, `○` a linked worktree.
- `no pane` means nothing is running in that checkout. `Enter` opens it.
- Panes show the agent's name, or `shell` when herdr is not tracking an agent there, and the
  pane id on the right.

Agent state: `●` working, `○` idle, `◆` blocked, `✓` done, `·` no agent.

### Filtering

The filter is fuzzy and cascades down the tree: matching a repository shows everything in it,
matching a worktree shows the panes running on it, and a pane that matches on its own brings
its headers along so you can see where it is.

Fuzzy matching is permissive — `harken` is a subsequence of a surprising amount of text — so
results are ordered by how well they matched rather than by tree order. The repository you
meant is at the top.

## Branches

The branch list is a search box. There is no mode to enter: typing filters, because typing a
branch name is the common case.

| Key | Does |
| --- | --- |
| any letter | filter |
| `↑` `↓`, `Ctrl-P` `Ctrl-N` | move |
| `Enter` | choose this branch |
| `Backspace` | delete a character |
| `Tab` | back to panes |
| `Esc`, `Ctrl-C` | close |

The remote is read in the background. The local answer is on screen immediately and
`reading the remote…` sits beside the prompt until `git ls-remote` returns; branches that
have never been fetched appear when it does. Offline, that line simply goes away and the
local list stands.

### What the states mean

| Shown as | The branch is | `Enter` does |
| --- | --- | --- |
| `● running` | open in a pane right now | goes to that pane |
| `○ checked out` | a worktree with nothing running | opens that checkout where you choose |
| `· local` | a local branch, no worktree | cuts a worktree from it |
| `↓ remote` | on the remote, never fetched | fetches it, then cuts from `origin/<branch>` |
| `+ create` | nothing yet — you typed it | creates it from `HEAD`, then cuts |

`running` skips the destination step. You already have that work open; being asked where to
put a second copy of it would be the wrong question.

`remote` is fetched into `refs/remotes/origin/<branch>` and the worktree is based on that
ref. Basing on `HEAD` instead would hand you an empty branch that merely shares a name with
the one on GitHub.

The offer to create sits last and appears whenever what you typed is a valid branch name that
does not already exist — including when something else fuzzy-matched. Typing `feat/login-v2`
while `feat/login` exists must still offer to create it.

### Choosing a destination

| Key | Does |
| --- | --- |
| `↑` `↓`, `k` `j` | move |
| `Enter` | open the pane there |
| `Esc`, `Backspace` | back to the branch list |

```
here            split right
                split down
existing tab    w1  app / logs
                w5  harken / android
existing space  w1  app → new tab
new space       on its own
```

- **here** splits the pane you summoned the picker from.
- **existing tab** splits that tab at whichever pane herdr thinks best. The tab you came from
  is not listed — "here" already covers it.
- **existing space** adds a tab to that space.
- **new space** leaves it in the workspace herdr made for it, which is what herdr's own
  `new worktree` binding does.

`split right` starts selected, so `Enter` `Enter` puts the branch beside what you were doing.

## Diagnostics

```sh
herdr-gh-nav dump
```

Prints the tree the picker would draw, as plain text. Useful for separating "herdr or git
told us something odd" from "the picker drew it wrong". Run it from a pane inside a herdr
session — it needs `HERDR_SOCKET_PATH`.
