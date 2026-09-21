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
RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links -A rustdoc::private_intra_doc_links" \
    cargo doc --no-deps --document-private-items
./scripts/check-invariants.sh
./scripts/check-docs-sync.sh origin/main
```

CI runs exactly these.

`cargo test` needs a locale your git has a translation for. `tests/git_locale.rs` proves the
`LC_ALL` pin by reading a git that would otherwise answer in another language, and where there
is no such locale it fails rather than skipping, because libtest keeps a passing test's output
to itself and a skip would read as a pass. The failure says which of the two things is missing
and names the fix — on Linux, `sudo locale-gen de_DE.UTF-8`.

If you changed the UI, review the snapshots rather than editing them:

```sh
cargo insta review
```

## What CI will hold you to

- **Everything you write here is in English.** Code, comments, commit messages, pull request
  titles and bodies, documentation. `docs/ja` is the one exception, and it is a translation.
- **Conventional Commits.** `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, `ci:`.
- **`src/domain` stays pure.** No processes, filesystem, network, environment, or clock.
  `Command::new` belongs in `src/adapter` alone, and exactly one place there starts a git:
  that is where the locale git's messages are matched in gets pinned.
- **Translations ship together.** Touch `docs/en/x.md` and you touch `docs/ja/x.md` in the
  same change. If you cannot write the Japanese, say so in the pull request and it can be
  written for you — but the pair must land together.
- **The manifest and crate versions agree.**
- **The toolchain is pinned.** `rust-toolchain.toml` says which Rust the gates run on, and
  every workflow has to install that one. If `RUSTUP_TOOLCHAIN` is set in your shell it wins
  over the file — unset it, or you are linting against something CI will not.

Two testing rules live in [the architecture page](docs/en/architecture.md#testing) because
they cost something to learn: a function whose failure is a value its success can also have
needs a test that watches it succeed, and a mutation is measured with
`cargo test --all-targets` and nothing narrower.

## What a comment is for

A comment carries what stays true when the code changes. Anything else is a second copy of a
fact the code already holds, and the second copy is the one nothing checks: it goes on
reading correctly for exactly as long as nobody touches the first.

Write:

- **What a type or a function promises**, and what a caller may assume. `WorkingTree` having
  three answers rather than two is the shape of the thing, not a detail of today's code.
- **Facts about the world outside this crate.** What git prints when it drops a ref, what
  herdr does with a tab that empties, what `gh` answers for a fork. A reader cannot get
  these from the code, and getting them again costs an experiment. Say which version you
  measured against.
- **The alternative that looks obvious and is wrong**, and why. Same test as an ADR: would a
  later reader be tempted to undo it? Where an ADR already carries the decision, link it
  rather than summarising it — a summary is one more copy to keep in step with the record.

Do not write:

- **A measured number.** A column width in prose is a value with no test holding it, and it
  is usually sitting directly above an assertion carrying the same value, which does. Name a
  constant or state the rule, and let the assertion keep the number.
- **What the code used to be.** `git log` has it, and an ADR has it where the decision earned
  a record. What is left when the narration goes is the constraint itself, which is still
  true and still worth saying: not "both were wrong once", but what makes both easy to get
  wrong.
- **The step the line below already takes.** A reader who wants the how has it in front of
  them; what they cannot see is why it is that way.

Tests are held to the same rule. A test name here is a sentence, so a comment above it
saying the same thing again is the copy that rots — what belongs there is the failure the
test would catch.

`check-invariants.sh` gates the first two of the three, and gates one more: a name in
backticks has to be the name of something. It reads comments as well as the pages that ship,
and it searches the code with the comment lines taken out, so a name surviving only in the
sentence that names it no longer satisfies it. A name belonging to git, herdr, std or one of
the crates goes on the `external` list in that script — the one place to say "this one is
not ours to keep current".

That check asks only whether a name exists somewhere, so a rustdoc link written on the wrong
type — `[`forget`](Self::forget)` on a type with no `forget` — satisfies it as soon as some
other type has one. `cargo doc` asks whether the link resolves from where it is written,
which is the question, so it runs with `-D rustdoc::broken_intra_doc_links`.

## Naming another item from a comment

A sentence that points at something else in the crate is the part of a comment a rename
breaks, and it breaks silently: the sentence goes on reading well while pointing at nothing.
So write the pointer in the form that something checks.

On a `///` or `//!` line, write it as an intra-doc link — `[`Removal::sweeping`]`, or
`[`domain::rows::marks`](crate::domain::rows::marks)` where you want the path in the prose —
and rustdoc will resolve it from where it is written. Two things it cannot resolve, and
neither is worth bending the code for: a private item in another module, which is not
reachable by path, and anything at all on a `//` line, which rustdoc does not read.

Those keep the path in plain backticks, and `check-invariants.sh` takes the question instead:
whatever defines the segment before the end has to define the end. `domain::sweep::judge`
sends it to src/domain/sweep.rs, `Refs::Unreadable` to whichever file declares `Refs`. It is
the last two segments only, which is enough to catch the rename and cheap enough to run on
every comment in the tree, the pages under docs/ included. A holder belonging to std,
crossterm or a test crate goes on the `external_holders` list in that script.

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
