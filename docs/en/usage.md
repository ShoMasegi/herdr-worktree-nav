# Usage

[日本語](../ja/usage.md)

Two pickers, opened by two actions, toggled with `Tab`. Each opens as a popup: a centred box
over the live session, framed by herdr in your accent colour and titled `herdr-gh-nav`, at
the same proportions the session navigator uses.

Inside that frame they are drawn the way the navigator is — the same search line, breadcrumb
and key hint, the same connected tree glyphs, the same status glyphs. The keys the navigator
defines mean the same thing here.

While a picker is up, herdr routes every key into it, so its own keybindings are out of reach
until you close it. `Esc`, `q` and `Ctrl-C` all close it.

Where the picker opens *from* matters. It takes the repository and the pane you were on when
you pressed the key, which is how "split here" knows where "here" is and which repository the
branch list starts on.

## Panes

| Key | Does |
| --- | --- |
| `↑` `↓`, `k` `j`, `Ctrl-P` `Ctrl-N` | move |
| `Enter` on a pane | go to it |
| `Enter` on a checkout with nothing running | open it |
| `n` | add a pane to the checkout under the cursor |
| `Tab` | branches, starting on the repository under the cursor |
| `/` | search |
| `b` `w` `i` `d` | narrow to blocked, working, idle, or done |
| `a` | clear the state filter |
| `h` | show or hide panes that are not in a repository |
| `r` | reload |
| `q`, `Esc`, `Ctrl-C` | close |

The cursor stops only where there is somewhere to go: a pane, and a checkout with nothing
running in it. Repository headings and checkouts that already have panes are stepped over —
the panes listed directly under them are the answer, and stopping on the header first would
only make the walk longer. They stay on screen; the arrow keys just pass through them.

`b`/`w`/`i`/`d` replace the search box with a state chip. Pressing the same one again clears
it, so a filter is never a one-way door.

While searching, letters are text rather than commands. `Enter` keeps the filter and returns
to the keys above, `Esc` abandons it, and `Ctrl-U` empties it without leaving search.

### Reading a row

```
 ◆ ● ShoMasegi/herdr-gh-nav (2)
   └── ● main                       ~/Workspace/herdr-gh-nav
 ◆    ├── ● claude                  w7:p2
      └── · shell                   w7:p3
   └── · fix/crash  no pane         ~/.herdr/worktrees/herdr-gh-nav/fix-crash
                                    ↑ four columns past the longest label
```

Left to right: a gutter, the tree, a status glyph, the label, and a meta column.

- **The gutter** carries `◆` on the pane the session is currently focused on, and on the
  repository holding it.
- **The tree** connects what is under a repository with `├──`/`└──`, so a deep worktree
  still reads as belonging somewhere. The repository itself gets no glyph: it is a heading,
  with nothing to expand.
- **The label** is `owner/repo (n)` for a repository, where `n` counts its open panes; the
  branch for a worktree; and the agent's name, or `shell`, for a pane.
- **The meta column** says where the thing is: the checkout path for a worktree and the pane
  id for a pane. A repository shows nothing there — the main checkout directly beneath
  carries the same path. Paths under your home are shortened to `~`.
- The column starts four blanks past the longest label that has something to put there, not
  at the right edge, so a path stays beside its row on a wide pane rather than across the
  screen from it. It takes the rest of the line from there, so a path is usually shown in
  full; one that still will not fit loses its middle rather than either end, since the head
  says which tree the checkout is in and the tail says which checkout.
- A checkout with nothing running in it is marked `no pane` beside its name, because its
  meta column is taken by the path.

The breadcrumb under the list carries the whole path for the row under the cursor, which is
where an elided one can be read in full.

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

Three steps: which repository, which branch, and where its pane goes.

### Choosing a repository

Every repository herdr has open, which is the same set the panes view groups by. The one you
summoned the picker from is marked ◆ and starts under the cursor, so carrying on where you
were costs one more `Enter` — and any other repository is one ↓ away, without going through the
panes view first.

```
 / search repositories                                        4 repositories
 ◆ ShoMasegi/herdr-gh-nav     1 worktree, 2 panes    ~/Workspace/herdr-gh-nav
   ShoMasegi/harbour-backend  3 worktrees, 5 panes   ~/Workspace/harbour/harbour-backend
   nightowl/harken            1 worktree, 1 pane     ~/Workspace/nightowl/harken
   nightowl/harken_android    1 worktree, 3 panes    ~/Workspace/nightowl/harken_android
```

The counts say how much of each repository is already open. The path tells two checkouts of
the same fork apart, and is searchable along with the name.

| Key | Does |
| --- | --- |
| any letter | filter |
| `↑` `↓`, `Ctrl-P` `Ctrl-N` | move |
| `Enter` | list this repository's branches |
| `Ctrl-U` | empty the search |
| `Tab` | back to panes |
| `Esc`, `Ctrl-C` | close |

With only one repository open this step is skipped: a picker that asks you to choose between
one thing is asking nothing. `Esc` then closes the branch list rather than going back to it.

### Choosing a branch

The branch list is a search box. There is no mode to enter: typing filters, because typing a
branch name is the common case.

| Key | Does |
| --- | --- |
| any letter | filter |
| `↑` `↓`, `Ctrl-P` `Ctrl-N` | move |
| `Enter` | choose this branch |
| `Backspace` | delete a character |
| `Ctrl-U` | empty the search |
| `Ctrl-O` | next order |
| `Ctrl-R` | reverse it |
| `Tab` | back to panes |
| `Esc` | back to the repositories |
| `Ctrl-C` | close |

Each repository is read once. Going back, picking another, and returning does not re-run git.

The remote is read in the background. The local answer is on screen immediately and
`reading the remote…` sits beside the prompt until `git ls-remote` returns; branches that
have never been fetched appear when it does. Offline, that line simply goes away and the
local list stands.

### Ordering

`Ctrl-O` walks the three orders and `Ctrl-R` turns the current one around. Which one is in
force sits beside the count, and takes the accent colour once it is no longer the default,
because a list in an unusual order should say so.

| Order | Reads |
| --- | --- |
| `state ↓` | running, then checked out, then local, then remote-only — newest first within each. The default |
| `updated ↓` | most recently committed first |
| `name ↑` | a to z |

The arrow describes the values rather than the rows: ↓ is descending, so it means newest,
busiest, or z first. `Ctrl-O` puts the arrow back to the new order's own direction, since
"oldest first" is not what asking for a date order meant; `Ctrl-R` is how you say you meant
it.

Two things stay put whichever order is chosen. A branch with no date — one seen only through
`ls-remote` and never fetched — sinks to the bottom in both directions, because reversing an
order should not fill the top of the screen with the rows that have the least to say. And the
offer to create a branch stays last: it is an offer, not one of the repository's branches.

The order also survives typing. The fuzzy search decides what is in the list; the order
decides where it sits. Sorting the filtered list by match score instead would quietly
override an order you had just chosen, the moment you typed anything.

It is kept while the picker is open, across repositories, and forgotten when it closes: this
plugin writes nothing to disk.

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

Beside the list is what the tab will look like once the pane lands in it, for whichever row
the cursor is on: the tab's real layout, with the arriving pane drawn in where it will
actually go. The prediction is exact rather than a sketch — a destination that names no pane
splits whichever pane that tab has focused, and the split is even, which is what herdr does.

A tab that is **zoomed** shows a warning instead of a diagram. herdr answers a request to
move a pane into a zoomed tab with success and then does not move it, so the picker stops
before asking rather than appearing to work. Unzoom the tab and it becomes available.

`split right` starts selected, so `Enter` `Enter` puts the branch beside what you were doing.

### While it works

Opening a branch can mean a fetch across the network and a checkout of a whole working tree.
The picker stays where it is and says which step it is on:

```
 ⠸ fetching origin/feat/login…
 ─────────────────────────────────────────────────────────────────────
 here            split right       w1  app / agents
                 split down        ┌──────────┬──────────┐
 existing tab    w1  app / logs    │ ● claude │ + feat/l…│
 …
 ctrl+c stop
```

The list and the preview stay put — the highlighted row is the destination being acted on,
and the diagram beside it is the tab being built. The steps are `fetching origin/<branch>`,
`creating the worktree for <branch>` or `opening the checkout for <branch>`, and then moving
the pane where you asked.

`Ctrl-C` stops it, but only while the key hint says `ctrl+c stop` — that is, up to and
including the fetch. A fetch writes nothing but `refs/remotes` and can be walked away from.
Once herdr has been asked for a worktree, leaving would strand the workspace it made, so
there is no key for it and the hint changes to `working…`.

If a step fails, the picker holds the screen and shows what git or herdr said, on the step
that said it:

```
 × fetching origin/feat/login: `git fetch …` failed: fatal: Could not read from remote…
 …
 ↵ close  esc close
```

Nothing is left half-done: the failing step is the one that stopped, and the steps before it
either did not touch anything (the fetch) or completed. The same message also goes to
`herdr plugin log list`, in full.

## Diagnostics

```sh
herdr-gh-nav dump
```

Prints the tree the picker would draw, as plain text. Useful for separating "herdr or git
told us something odd" from "the picker drew it wrong". Run it from a pane inside a herdr
session — it needs `HERDR_SOCKET_PATH`.
