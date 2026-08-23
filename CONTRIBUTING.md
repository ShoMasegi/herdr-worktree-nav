# Contributing

Thanks for taking a look. Issues and pull requests are both welcome.

How the repository is worked on — the gates, the commit format, the translation rule, when a
decision earns a record — is below. What shapes the code itself, the layers and how herdr is
talked to, is in [docs/en/architecture.md](./docs/en/architecture.md).

## Getting set up

```sh
git clone https://github.com/ShoMasegi/herdr-worktree-nav
cd herdr-worktree-nav
cargo build --release && mkdir -p bin && ln -sf ../target/release/herdr-worktree-nav bin/herdr-worktree-nav
herdr plugin link .
```

`herdr plugin link` does not run the `[[build]]` step, which is why the binary is built by
hand. After that `cargo build --release` is enough; the symlink keeps `bin/` current.

Try it:

```sh
herdr plugin action invoke herdr-worktree-nav.open-panes
herdr-worktree-nav dump          # from a pane inside a herdr session
```

## Before you open a pull request

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
./scripts/check-invariants.sh
./scripts/check-docs-sync.sh origin/main
```

CI runs exactly these.

If you changed the UI, review the snapshots rather than editing them:

```sh
cargo insta review
```

## What CI will hold you to

- **Everything you write here is in English.** Code, comments, commit messages, pull request
  titles and bodies, documentation. `docs/ja` is the one exception, and it is a translation.
- **Conventional Commits.** `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, `ci:`.
- **`src/domain` stays pure.** No processes, filesystem, network, environment, or clock.
  `Command::new` belongs in `src/adapter` alone.
- **Translations ship together.** Touch `docs/en/x.md` and you touch `docs/ja/x.md` in the
  same change. If you cannot write the Japanese, say so in the pull request and it can be
  written for you — but the pair must land together.
- **The manifest and crate versions agree.**
- **The toolchain is pinned.** `rust-toolchain.toml` says which Rust the gates run on, and
  every workflow has to install that one. If `RUSTUP_TOOLCHAIN` is set in your shell it wins
  over the file — unset it, or you are linting against something CI will not.

## Things worth knowing before changing behaviour

herdr's own API shapes some decisions in ways that are not obvious from the code. Three of
them are written up in [docs/adr](./docs/adr/): why worktree creation is delegated to herdr,
why the socket is used instead of the CLI, and why `gh` is only decoration. If you are about
to undo one of those, read the record first — each one exists because the obvious alternative
was tried and had a specific problem.

Add one yourself when a decision is non-obvious, and "non-obvious" has a test: would a later
reader be tempted to undo it? If yes, the reason has to outlive you. Documentation ships in
the same commit as the code it describes, records included.

Anything touching herdr cannot be tested in CI, because there is no server there. The manual
checklist is in [docs/en/troubleshooting.md](./docs/en/troubleshooting.md); please run it and
say in the pull request which parts you covered.

## Releasing

Maintainers only, and deliberately manual:

1. Bump `version` in both `Cargo.toml` and `herdr-plugin.toml`. `check-invariants.sh` fails if
   they disagree, because `scripts/fetch-or-build.sh` looks for a release tag named after the
   manifest's.
2. Add the `CHANGELOG.md` entry.
3. Tag `vX.Y.Z`. The tag is what triggers the cross-compiled release build.

## Reporting a bug

Include:

- `herdr --version` and your OS,
- the output of `herdr plugin log list --plugin herdr-worktree-nav --limit 5`,
- the output of `herdr-worktree-nav dump` if the picker showed something surprising.

Those three answer most of the questions a maintainer would otherwise have to ask.
