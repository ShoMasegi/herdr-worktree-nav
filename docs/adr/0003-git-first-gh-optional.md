# 3. Branches come from git; `gh` only decorates them

Status: accepted; the scope this gives `gh` is widened by
[ADR 0011](./0011-what-may-be-swept.md)

## Context

The branches view exists to turn "a branch that is on GitHub" into a local worktree. The
obvious reading is that it should ask GitHub: `gh api repos/{owner}/{repo}/branches` returns
exactly the branches GitHub has, with no fetching required.

## Decision

git is the source of truth. `gh` adds pull request numbers and titles and nothing else.

Branches are read with `git for-each-ref refs/heads refs/remotes`, which is instant and works
offline, and `git ls-remote --heads origin`, which finds branches that have never been
fetched using the git credentials the user already has.

Making GitHub the primary source would have meant:

- a hard dependency on `gh` being installed and authenticated,
- nothing at all to show offline, and
- no support for a repository whose origin is not GitHub, which the picker would otherwise
  handle perfectly well.

None of those are worth the one thing GitHub-first buys — seeing a branch someone pushed
seconds ago — when `ls-remote` gets that too.

## Consequences

- `GhPort::pull_requests` returns a `Vec`, not a `Result`. Every failure — `gh` missing, not
  authenticated, offline, not a GitHub repository — degrades to an empty list. This layer
  must never fail the picker.
- The plugin works against GitLab, Bitbucket, self-hosted, and purely local repositories.
  Only the pull request column goes missing.
- A never-fetched branch is fetched into `refs/remotes/origin/<branch>` and the worktree is
  cut from that ref rather than from `HEAD`. Basing on `HEAD` would silently produce an empty
  branch that merely shares a name with the one on GitHub.
