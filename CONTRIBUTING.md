# Contributing

Thanks for taking a look. Issues and pull requests are both welcome.

The development conventions — architecture invariants, testing approach, commit format,
translation rules — live in [CLAUDE.md](./CLAUDE.md). This file is only the practical steps.

## Getting set up

```sh
git clone https://github.com/ShoMasegi/herdr-gh-nav
cd herdr-gh-nav
cargo build --release && mkdir -p bin && ln -sf ../target/release/herdr-gh-nav bin/herdr-gh-nav
herdr plugin link .
```

`herdr plugin link` does not run the `[[build]]` step, which is why the binary is built by
hand. After that `cargo build --release` is enough; the symlink keeps `bin/` current.

Try it:

```sh
herdr plugin action invoke herdr-gh-nav.open-panes
herdr-gh-nav dump          # from a pane inside a herdr session
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

- **Conventional Commits.** `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, `ci:`.
- **`src/domain` stays pure.** No processes, filesystem, network, environment, or clock.
  `Command::new` belongs in `src/adapter` alone.
- **Translations ship together.** Touch `docs/en/x.md` and you touch `docs/ja/x.md` in the
  same change. If you cannot write the Japanese, say so in the pull request and it can be
  written for you — but the pair must land together.
- **The manifest and crate versions agree.**

## Things worth knowing before changing behaviour

herdr's own API shapes some decisions in ways that are not obvious from the code. Three of
them are written up in [docs/adr](./docs/adr/): why worktree creation is delegated to herdr,
why the socket is used instead of the CLI, and why `gh` is only decoration. If you are about
to undo one of those, read the record first — each one exists because the obvious alternative
was tried and had a specific problem.

Anything touching herdr cannot be tested in CI, because there is no server there. The manual
checklist is in [docs/en/troubleshooting.md](./docs/en/troubleshooting.md); please run it and
say in the pull request which parts you covered.

## Reporting a bug

Include:

- `herdr --version` and your OS,
- the output of `herdr plugin log list --plugin herdr-gh-nav --limit 5`,
- the output of `herdr-gh-nav dump` if the picker showed something surprising.

Those three answer most of the questions a maintainer would otherwise have to ask.
