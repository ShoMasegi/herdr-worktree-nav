# CLAUDE.md

Development conventions for herdr-worktree-nav. This file is the single source of truth for how
this repository is worked on. `CONTRIBUTING.md` covers only the human-facing "how do I send
a PR" steps and deliberately does not repeat anything here.

## What this is

A [herdr](https://herdr.dev) plugin: two overlay pickers backed by one Rust binary.

- **Panes view** — every open herdr pane grouped as `repo -> worktree(branch) -> pane`,
  select one to jump to it.
- **Branches view** — every branch of the current repo (including remote branches that have
  never been fetched), select one to create a worktree and open it as a pane at a
  destination you choose.

`<Tab>` toggles between the two views. They are one UI, not two features.

## Language policy

- Code, comments, commit messages, PR titles/descriptions, and all documentation: **English**.
- Conversation with the maintainer in an agent session: **Japanese**.
- Every document under `docs/en/` has a translated twin at `docs/ja/` with the same filename.

## Architecture invariants

These are enforced in CI. Breaking one fails the build.

1. **`src/domain/` is pure.** No I/O, no `std::process`, no filesystem, no clock, no network.
   It takes plain data in and returns plain data out. This is where the interesting logic
   lives (tree building, branch-state resolution, destination-to-command planning) and it is
   where the tests live.
2. **`std::process::Command` appears only in `src/adapter/`.** Everything else reaches the
   outside world through the `HerdrPort` / `GitPort` / `GhPort` traits in `src/port/`.
3. **`herdr-plugin.toml`'s `version` equals `Cargo.toml`'s `version`.**

## Talking to herdr

The entire herdr CLI is the plugin API. Call it via `$HERDR_BIN_PATH` (falling back to
`herdr` on PATH) — never hardcode a path.

- One `herdr api snapshot` call returns workspaces, tabs, panes, agents, and layouts. Prefer
  it over several narrower calls.
- Prefer data herdr already computed over recomputing it:
  `WorkspaceInfo.worktree` gives `repo_key` / `repo_root` / `checkout_path` for
  worktree-backed workspaces, and `WorktreeInfo.open_workspace_id` tells you whether a
  worktree is already open. Only shell out to `git` for panes herdr has no worktree record for.
- `herdr worktree create` **always** creates a new workspace, tab, and root pane. There is no
  option to place it into an existing tab. To land a worktree pane somewhere else, create it
  and then `herdr pane move` the root pane, then close the emptied workspace. See
  `docs/adr/0001-delegate-worktree-creation.md` for why this is preferred over calling
  `git worktree add` directly.
- Resolve git information per-cwd and cache it. Several panes usually share one cwd, and a
  picker that spawns one `git` process per pane is noticeably slow.

## Testing

Test-first for everything in `src/domain/`. Write the failing test, then the code.

| Layer | How |
| --- | --- |
| `src/domain/` | Unit tests with fake ports. Cover all branch states and all destinations. |
| `src/ui/` | `ratatui::backend::TestBackend` + `insta` snapshots of the rendered buffer. |
| `src/adapter/` git | Real repositories built in a `tempfile::TempDir`. |
| `src/adapter/` herdr | Not testable in CI (no herdr server). Use the manual checklist in `docs/en/troubleshooting.md`. |

Run `cargo insta review` after intentional UI changes; never hand-edit a `.snap` file.

## Commits and releases

- [Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `docs:`,
  `refactor:`, `test:`, `chore:`, `ci:`. Enforced in CI.
- Documentation changes ship in the same commit as the code they describe. If you touch
  `docs/en/x.md`, update `docs/ja/x.md` in the same commit — CI fails otherwise.
- Non-obvious design decisions get an ADR in `docs/adr/`. "Non-obvious" means a future
  reader would otherwise be tempted to undo it.
- Releasing: bump `version` in `Cargo.toml` and `herdr-plugin.toml`, add a `CHANGELOG.md`
  entry, tag `vX.Y.Z`. The tag triggers the cross-compiled release build.

## Commands

```sh
cargo build --release                       # build
cargo test                                  # tests
cargo fmt --all -- --check                  # formatting gate
cargo clippy --all-targets -- -D warnings   # lint gate
./scripts/check-invariants.sh               # architecture invariants gate
./scripts/check-docs-sync.sh                # en/ja documentation parity gate

# Try it against the running herdr session
cargo build --release && mkdir -p bin && ln -sf ../target/release/herdr-worktree-nav bin/herdr-worktree-nav
herdr plugin link .
herdr plugin action invoke herdr-worktree-nav.open-panes
```

`herdr plugin link` does **not** run the `[[build]]` step, so build the binary yourself
before linking a checkout.
