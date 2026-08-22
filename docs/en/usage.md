# Usage

[日本語](../ja/usage.md)

Two pickers, opened by two actions, toggled with `Tab`. Both are overlays: they cover the
session while they are up and get out of the way the moment they close.

They are drawn the way herdr's own session navigator is — the same panel, search line,
breadcrumb and key hint, the same connected tree glyphs, the same status glyphs, and the
accent colour from your herdr theme. The keys the navigator defines mean the same thing here.

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
| `/` | search |
| `b` `w` `i` `d` | narrow to blocked, working, idle, or done |
| `a` | clear the state filter |
| `h` | show or hide panes that are not in a repository |
| `r` | reload |
| `q`, `Esc`, `Ctrl-C` | close |

`b`/`w`/`i`/`d` replace the search box with a state chip. Pressing the same one again clears
it, so a filter is never a one-way door.

While searching, letters are text rather than commands. `Enter` keeps the filter and returns
to the keys above, `Esc` abandons it, and `Ctrl-U` empties it without leaving search.
to the keys above; `Esc` clears it.

### Reading a row

```
 ◆ ▾ ● ShoMasegi/herdr-gh-nav (2)               1 working
   └── ● main                                   2 panes · 1 working
 ◆    ├── ● claude                              claude · working
      └── · shell                               shell
   └── · fix/crash                              no pane
```

Left to right: a gutter, the tree, a status glyph, the label, and a meta column.

- **The gutter** carries `◆` on the pane the session is currently focused on, and on the
  repository holding it.
- **The tree** uses `▾`/`▸` for a repository — `Enter` folds it — and connected `├──`/`└──`
  glyphs for what is under it, so a deep worktree still reads as belonging somewhere.
- **The label** is `owner/repo (n)` for a repository, where `n` counts its open panes; the
  branch for a worktree; and the agent's name, or `shell`, for a pane.
- **The meta column** says what is happening, not where the files are: the activity summary
  for a repository, `n panes · activity` for a worktree, and `agent · state` for a pane. A
  checkout with nothing running in it says `no pane`.

The checkout path lives in the breadcrumb under the list, which follows the cursor. Keeping
paths out of the rows is what lets the list be scanned for activity at a glance.

A blank line separates each repository, and a scrollbar appears on the right when the list is
longer than the pane.

Agent state: `●` working, `○` idle, `◆` blocked, `·` no agent — or `◐`, `○`, `×`, `·` if your
herdr is set to `status_indicators = "symbols"`.

### Filtering

The filter is fuzzy and cascades down the tree: matching a repository shows everything in it,
matching a worktree shows the panes running on it, and a pane that matches on its own brings
its headers along so you can see where it is.

Rows that did not match themselves but are kept for context — the repository above a result,
the panes under a matching branch — stay in the list and are drawn dimmed, so a result is
never shown without the structure that explains where it is.

Fuzzy matching is permissive — `harken` is a subsequence of a surprising amount of text — so
results are ordered by how well they matched rather than by tree order. The repository you
meant is at the top. (herdr's navigator keeps session order instead; its rows are ordered by
something you already know, and these are not.)

## Branches

The branch list is a search box. There is no mode to enter: typing filters, because typing a
branch name is the common case.

| Key | Does |
| --- | --- |
| any letter | filter |
| `↑` `↓`, `Ctrl-P` `Ctrl-N` | move |
| `Enter` | choose this branch |
| `Backspace` | delete a character |
| `Ctrl-U` | empty the search |
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
