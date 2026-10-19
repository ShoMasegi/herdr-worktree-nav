//! `GitCli` against real repositories.
//!
//! These are the only tests that run git for real. Everything they cover is a place where a
//! plausible-looking implementation is quietly wrong: a bare `.git` from the repository root,
//! a branch whose name contains a slash, a linked worktree that must resolve to the same
//! repository as its parent, and a fetch that has to leave a usable base behind.

use std::num::NonZeroU32;
use std::path::Path;
use std::process::Command;

use herdr_worktree_nav::adapter::GitCli;
use herdr_worktree_nav::port::{GitPort, ListedHead, RefKind, Slug, Track};
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A repository with one commit on `main`, an extra `feat/login` branch, and a committer
/// configured so commits work on a machine with no global git identity.
fn repository() -> TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path();
    // The ref format is pinned rather than inherited: two tests here reach into `.git/refs`
    // and `.git/packed-refs` directly, and under a global `init.defaultRefFormat = reftable`
    // neither file exists. `-c` on the command rather than `--ref-format=`, which git 2.43 on
    // CI does not know: an unknown configuration key is ignored where an unknown flag is
    // fatal.
    git(
        path,
        &[
            "-c",
            "init.defaultRefFormat=files",
            "init",
            "--initial-branch=main",
        ],
    );
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "Test"]);
    // Pinned, not inherited. `%(push:track)` answers a different question under each
    // `push.default`, so a test that leaves it to the machine's global config is testing
    // the machine — see `a_branch_nobody_has_pushed_is_never_gone`.
    git(path, &["config", "push.default", "simple"]);
    std::fs::write(path.join("README.md"), "hello\n").unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-m", "first"]);
    git(path, &["branch", "feat/login"]);
    dir
}

fn path_str(path: &Path) -> String {
    // macOS reports /var while the temp dir is /private/var; canonicalise both sides so the
    // comparisons in these tests are about git, not about symlinks.
    path.canonicalize().unwrap().to_string_lossy().into_owned()
}

#[test]
fn identifies_the_repository_from_the_root_a_subdirectory_and_a_worktree() {
    let repo = repository();
    let root = path_str(repo.path());

    let from_root = GitCli
        .identify(&root)
        .unwrap()
        .expect("root is a repository");
    // The interesting part: from the root, `git rev-parse --git-common-dir` prints a bare
    // ".git" unless --path-format=absolute is passed, which would not be a usable identity.
    assert_eq!(from_root.repo_key, format!("{root}/.git"));
    assert_eq!(from_root.checkout_path, root);
    assert_eq!(from_root.branch.as_deref(), Some("main"));

    std::fs::create_dir(repo.path().join("src")).unwrap();
    let from_subdir = GitCli
        .identify(&path_str(&repo.path().join("src")))
        .unwrap()
        .expect("a subdirectory is still in the repository");
    assert_eq!(from_subdir.checkout_path, root, "resolves to the top level");

    // A linked worktree is a different checkout of the same repository, and has to share the
    // identity — that is what groups them under one heading in the picker.
    let worktree = repo.path().join("wt");
    git(
        repo.path(),
        &["worktree", "add", worktree.to_str().unwrap(), "feat/login"],
    );
    let from_worktree = GitCli
        .identify(&path_str(&worktree))
        .unwrap()
        .expect("a worktree is a repository");
    assert_eq!(from_worktree.repo_key, from_root.repo_key);
    assert_ne!(from_worktree.checkout_path, from_root.checkout_path);
    assert_eq!(from_worktree.branch.as_deref(), Some("feat/login"));
}

#[test]
fn a_path_outside_any_repository_is_an_answer_rather_than_an_error() {
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(GitCli.identify(&path_str(empty.path())).unwrap(), None);
}

#[test]
fn a_checkout_that_has_gone_is_an_answer_rather_than_a_git_that_could_not_be_run() {
    // A pane left sitting in a checkout another pane removed. `app::collect` reads `Err`
    // here as "git could not be run, or would not say where it is", which is the item
    // `docs/{en,ja}/troubleshooting` sends the reader to check `PATH` over, and this is not
    // that — so it has to be the ordinary answer.
    //
    // It is `Ok(None)` because `command` passes the directory as `git -C`: git starts,
    // cannot chdir, and says `fatal: cannot change to …: No such file or directory`, which
    // `CANNOT_CHANGE_TO` matches.
    // Run with `Command::current_dir` instead, the spawn itself would fail with
    // `No such file or directory (os error 2)` — the same words a git that is not on the
    // path gives — and the prompt line would name the wrong cause. That is what this pins.
    let gone = tempfile::tempdir().unwrap();
    let path = path_str(gone.path());
    drop(gone);
    assert_eq!(GitCli.identify(&path).unwrap(), None);
}

#[test]
fn a_directory_git_could_not_enter_is_an_error_rather_than_no_repository() {
    // The other side of the test above. git says `cannot change to` of a directory that is
    // there too, when it may not enter it; it never looked, so it has not said the pane is
    // in no repository, and read so the pane would sit among those in none with nothing to
    // say why.
    use std::os::unix::fs::PermissionsExt;
    let repo = repository();
    let locked = repo.path().join("locked");
    let inner = locked.join("inner");
    std::fs::create_dir_all(&inner).unwrap();
    // Spelled while it can still be resolved: `path_str` canonicalises.
    let cwd = path_str(&inner);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _put_back = ReadableAgain(&locked);
    // Root enters a directory with no permissions, and git then answers with the checkout.
    // That is not what this test is about, so it says so rather than failing as though the
    // adapter had changed.
    assert!(
        std::fs::read_dir(&inner).is_err(),
        "this test needs a user the mode 000 keeps out, which root is not"
    );

    let error = GitCli
        .identify(&cwd)
        .expect_err("git could not enter the directory");
    let words = format!("{error:#}");
    assert!(
        words.starts_with("fatal: cannot change to") && words.contains("Permission denied"),
        "git's own words: {words}"
    );
}

#[test]
fn a_checkout_that_has_gone_is_said_to_have_gone_in_gits_words() {
    // Issue #34. The other side of `identify`'s answer above: to a call handed a checkout
    // herdr lists, a directory that has gone is an error, and git's words for it say what
    // happened. A sentence of the plugin's own saying the path is not a repository would
    // send the reader looking for a `.git` in a directory that is not there.
    let gone = tempfile::tempdir().unwrap();
    let path = path_str(gone.path());
    drop(gone);

    for (call, error) in [
        ("is_dirty", GitCli.is_dirty(&path).map(|_| ()).unwrap_err()),
        (
            "local_refs",
            GitCli.local_refs(&path).map(|_| ()).unwrap_err(),
        ),
    ] {
        let words = format!("{error:#}");
        assert!(
            words.starts_with("fatal: cannot change to")
                && words.contains("No such file or directory"),
            "{call}: git's own words, first: {words}"
        );
        assert!(
            !words.contains("not a git repository"),
            "{call}: and nothing that says it was never one: {words}"
        );
    }
}

#[test]
fn a_repository_git_may_not_enter_is_not_one_that_is_not_there() {
    // The same refusal reaching a call that expects a repository. Read as "no repository",
    // it would say the repository is not one, and the reader would look for a repository
    // that is there rather than at its permissions.
    use std::os::unix::fs::PermissionsExt;
    let repo = repository();
    let root = path_str(repo.path());
    std::fs::set_permissions(repo.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
    let _put_back = ReadableAgain(repo.path());
    assert!(
        std::fs::read_dir(repo.path()).is_err(),
        "this test needs a user the mode 000 keeps out, which root is not"
    );

    let error = GitCli
        .local_refs(&root)
        .expect_err("git could not enter the repository");
    let words = format!("{error:#}");
    assert!(
        words.starts_with("fatal: cannot change to") && words.contains("Permission denied"),
        "git's own words: {words}"
    );
}

#[test]
fn a_pane_in_a_repository_but_in_no_work_tree_is_an_answer_rather_than_an_error() {
    // A shell in a bare repository — the layout that keeps one beside its worktrees — or in
    // a `.git` directory. git exits 128 with `this operation must be run in a work tree`:
    // it has answered, and the answer is that the directory is in no work tree, so git
    // places it in no checkout. Read as a refusal, the prompt line would say git could not
    // place the pane.
    let repo = repository();
    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "--bare", "-q"]);
    for (cwd, what) in [
        (bare.path().to_path_buf(), "a bare repository"),
        (repo.path().join(".git"), "a .git directory"),
        (
            repo.path().join(".git").join("refs"),
            "a directory under .git",
        ),
    ] {
        assert_eq!(
            GitCli.identify(&path_str(&cwd)).unwrap(),
            None,
            "{what} is no checkout"
        );
    }
}

#[test]
fn a_repository_git_would_not_read_is_an_error_rather_than_no_checkout() {
    // The tests above make `identify` answer `None` from the refusals that mean the pane is
    // in no checkout. Any other is git failing to say where the pane is, and has to reach
    // the prompt line in git's words: read as `None`, the pane would sit among the panes in
    // no repository and nothing would say why — the silence issue #33 is about, from a git
    // that started and refused rather than one that could not start.
    let repo = repository();
    let config = repo.path().join(".git").join("config");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str("[core\n");
    std::fs::write(&config, text).unwrap();

    let error = GitCli
        .identify(&path_str(repo.path()))
        .expect_err("git could not parse its own config");
    let words = format!("{error:#}");
    assert!(words.contains("bad config"), "git's own words: {words}");
}

#[test]
fn a_repository_git_does_not_take_for_one_is_not_answered_for_by_the_one_around_it() {
    // Where a repository's `.git/HEAD` cannot be read, is empty or holds garbage, git does
    // not take its `.git` for a repository and walks up to the one around it, exit 0. Every
    // call handed a repository root or a checkout's own path, as it is or through a link,
    // has to fail there instead, or the outer repository's refs and checkouts are read as
    // the inner one's, and a removal or a branch deletion runs in the outer one — issue #93.
    // The same where the outer repository's directory holds a `:` — issue #96.
    use std::os::unix::fs::PermissionsExt;

    let commit = |dir: &Path, message: &str| {
        git(
            dir,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-m",
                message,
            ],
        );
    };
    let around = tempfile::tempdir().unwrap();
    for name in ["outer", "out:er"] {
        let outer = around.path().join(name);
        std::fs::create_dir(&outer).unwrap();
        git(&outer, &["init", "--initial-branch=main"]);
        commit(&outer, "outer");
        git(&outer, &["branch", "merged"]);
        let outer_checkout = around.path().join(format!("{name}-checkout"));
        git(
            &outer,
            &[
                "worktree",
                "add",
                "--detach",
                outer_checkout.to_str().unwrap(),
            ],
        );
        let inner = outer.join("inner");
        std::fs::create_dir(&inner).unwrap();
        git(&inner, &["init", "--initial-branch=main"]);
        commit(&inner, "inner");
        let head = inner.join(".git").join("HEAD");
        let link = around.path().join(format!("{name}-inner-link"));
        std::os::unix::fs::symlink(&inner, &link).unwrap();
        assert_eq!(
            GitCli.head_ref(&path_str(&inner)).unwrap(),
            "main",
            "{name}: a repository git reads answers for itself"
        );
        let sub = inner.join("sub");
        std::fs::create_dir(&sub).unwrap();
        assert_eq!(
            GitCli
                .identify(&path_str(&sub))
                .unwrap()
                .map(|found| found.checkout_path),
            Some(path_str(&inner)),
            "{name}: a pane below the top of a repository git reads is placed in it"
        );
        let linked = around
            .path()
            .join(format!("{name}-linked"))
            .join("checkout");
        git(
            &outer,
            &["worktree", "add", "--detach", linked.to_str().unwrap()],
        );
        let dotfiles = around.path().join(format!("{name}-home")).join(".dotfiles");
        std::fs::create_dir_all(&dotfiles).unwrap();
        git(&dotfiles, &["init", "--initial-branch=main"]);
        git(&dotfiles, &["config", "core.worktree", "../.."]);
        commit(&dotfiles, "dotfiles");
        let dotfiles_link = around.path().join(format!("{name}-dotfiles-link"));
        std::os::unix::fs::symlink(&dotfiles, &dotfiles_link).unwrap();
        let named = around
            .path()
            .join(format!("{name}-home2"))
            .join(".dotfiles");
        std::fs::create_dir_all(&named).unwrap();
        let named_git_dir = around.path().join(format!("{name}-dotfiles.git "));
        git(
            &named,
            &[
                "init",
                "--initial-branch=main",
                "--separate-git-dir",
                named_git_dir.to_str().unwrap(),
            ],
        );
        git(
            &named,
            &[
                "config",
                "core.worktree",
                named.parent().unwrap().to_str().unwrap(),
            ],
        );
        commit(&named, "named");
        let bare = around.path().join(format!("{name}-bare")).join("repo.git");
        git(
            around.path(),
            &[
                "clone",
                "--quiet",
                "--bare",
                outer.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        let spaced = around.path().join(format!("{name}-bare")).join("repo.git ");
        git(
            around.path(),
            &[
                "clone",
                "--quiet",
                "--bare",
                outer.to_str().unwrap(),
                spaced.to_str().unwrap(),
            ],
        );
        let carriage = around
            .path()
            .join(format!("{name}-bare"))
            .join("repo.git\r");
        git(
            around.path(),
            &[
                "clone",
                "--quiet",
                "--bare",
                outer.to_str().unwrap(),
                carriage.to_str().unwrap(),
            ],
        );
        let bare_link = around.path().join(format!("{name}-bare-link"));
        std::os::unix::fs::symlink(&bare, &bare_link).unwrap();
        let crlf = around.path().join(format!("{name}-crlf")).join("checkout");
        std::fs::create_dir_all(&crlf).unwrap();
        git(
            &crlf,
            &[
                "init",
                "--initial-branch=main",
                "--separate-git-dir",
                around
                    .path()
                    .join(format!("{name}-crlf.git"))
                    .to_str()
                    .unwrap(),
            ],
        );
        commit(&crlf, "crlf");
        let gitfile = crlf.join(".git");
        let written = std::fs::read_to_string(&gitfile).unwrap();
        assert!(
            written.ends_with('\n'),
            "{name}: git ends a `.git` file with a line end"
        );
        std::fs::write(&gitfile, written.replace('\n', "\r\n")).unwrap();
        let store = around.path().join(format!("{name}-store"));
        std::fs::create_dir(&store).unwrap();
        git(&store, &["init", "--initial-branch=main"]);
        commit(&store, "store");
        let symlinked = around.path().join(format!("{name}-home3")).join("checkout");
        std::fs::create_dir_all(&symlinked).unwrap();
        std::os::unix::fs::symlink(store.join(".git"), symlinked.join(".git")).unwrap();
        git(
            &store,
            &["config", "core.worktree", symlinked.to_str().unwrap()],
        );
        // A volume that ignores case keeps a linked checkout's `.git` file naming its main
        // checkout's directory as it was spelled when the checkout was added.
        #[cfg(target_os = "macos")]
        let respelled = {
            let main = around.path().join(format!("{name}-Respelled"));
            std::fs::create_dir(&main).unwrap();
            git(&main, &["init", "--initial-branch=main"]);
            commit(&main, "respelled");
            let checkout = around
                .path()
                .join(format!("{name}-respelled-linked"))
                .join("checkout");
            git(
                &main,
                &["worktree", "add", "--detach", checkout.to_str().unwrap()],
            );
            let aside = around.path().join(format!("{name}-respelled-aside"));
            std::fs::rename(&main, &aside).unwrap();
            std::fs::rename(&aside, around.path().join(format!("{name}-respelled"))).unwrap();
            assert!(
                main.exists(),
                "this volume tells case apart, so the checkout's `.git` file names a directory \
                 that has gone and there is nothing here to measure. Run this on one that does not."
            );
            Some((
                "a linked checkout whose main checkout's directory changed case",
                path_str(&checkout),
            ))
        };
        #[cfg(not(target_os = "macos"))]
        let respelled = None;
        for (shape, root) in [
            ("a bare repository", path_str(&bare)),
            (
                "the same through a link",
                bare_link.to_str().unwrap().to_string(),
            ),
            (
                "a bare repository whose name ends in a space",
                path_str(&spaced),
            ),
            (
                "a bare repository whose name ends in a carriage return",
                path_str(&carriage),
            ),
            ("a checkout whose `.git` is a link", path_str(&symlinked)),
            ("a checkout whose `.git` file ends in CRLF", path_str(&crlf)),
            ("a linked checkout", path_str(&linked)),
            (
                "a repository whose work tree is above it",
                path_str(&dotfiles),
            ),
            (
                "the same through a link",
                dotfiles_link.to_str().unwrap().to_string(),
            ),
            (
                "the same with a `.git` file naming a git directory whose name ends in a space",
                path_str(&named),
            ),
        ]
        .into_iter()
        .chain(respelled)
        {
            let answer = GitCli.head_ref(&root);
            assert!(
                answer.is_ok(),
                "{name}: {shape} answers for itself: {answer:?}"
            );
        }
        let unreadable_config = around.path().join(format!("{name}-config")).join("repo");
        std::fs::create_dir_all(&unreadable_config).unwrap();
        git(&unreadable_config, &["init", "--initial-branch=main"]);
        std::fs::write(unreadable_config.join(".git").join("config"), "[[[\n").unwrap();
        let words = format!(
            "{:#}",
            GitCli
                .local_refs(&path_str(&unreadable_config))
                .unwrap_err()
        );
        assert!(
            words.contains("bad config"),
            "{name}: a repository git will not read says so in git's words: {words}"
        );
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            inner,
            "{name}: the link"
        );

        for how in ["empty", "garbage", "unreadable"] {
            std::fs::write(&head, "ref: refs/heads/main\n").unwrap();
            std::fs::set_permissions(&head, std::fs::Permissions::from_mode(0o644)).unwrap();
            match how {
                "empty" => std::fs::write(&head, "").unwrap(),
                "garbage" => std::fs::write(&head, "garbage\n").unwrap(),
                _ => {
                    std::fs::set_permissions(&head, std::fs::Permissions::from_mode(0o000)).unwrap()
                }
            }
            let _put_back = ReadableAgain(&head);
            if how == "unreadable" {
                assert!(
                    std::fs::read(&head).is_err(),
                    "this user can read a file with mode 000, so git will not be refused and \
                     there is nothing here to measure. Run this as a user permissions apply to."
                );
            }
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&inner)
                    .args(["worktree", "list", "--porcelain"])
                    .output()
                    .unwrap()
                    .status
                    .success(),
                "{how}: a git left to look for itself answers for the repository around it"
            );

            for root in [path_str(&inner), link.to_str().unwrap().to_string()] {
                let how = format!("{how} at {root}");
                let words = format!("{:#}", GitCli.local_refs(&root).unwrap_err());
                let refused = if name == "outer" {
                    "fatal: not a git repository"
                } else {
                    "is not a git repository: git answered for the repository around it"
                };
                assert!(words.contains(refused), "{how}: local_refs: {words}");
                assert!(
                    GitCli.worktree_heads(&root).is_err(),
                    "{how}: worktree_heads"
                );
                assert!(GitCli.is_dirty(&root).is_err(), "{how}: is_dirty");
                assert!(
                    GitCli.has_no_commit_yet(&root).is_err(),
                    "{how}: has_no_commit_yet"
                );
                assert!(GitCli.head_ref(&root).is_err(), "{how}: head_ref");
                assert!(GitCli.github_slug(&root).is_err(), "{how}: github_slug");
                assert!(
                    GitCli.delete_branch(&root, "merged").is_err(),
                    "{how}: delete_branch"
                );
                assert!(
                    GitCli
                        .remove_worktree(&root, &path_str(&outer_checkout))
                        .is_err(),
                    "{how}: remove_worktree"
                );
            }
        }
        assert!(
            outer_checkout.exists(),
            "{name}: the outer repository's checkout is still there"
        );
        git(&outer, &["rev-parse", "--verify", "refs/heads/merged"]);

        // An outer repository whose git directory sits inside the nested one: inside the
        // directory, and still not the nested repository's own.
        let outer = around.path().join(format!("{name}2"));
        let inner = outer.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        git(
            &outer,
            &[
                "init",
                "--initial-branch=main",
                "--separate-git-dir",
                inner.join("outer.git").to_str().unwrap(),
            ],
        );
        commit(&outer, "outer");
        git(&outer, &["branch", "merged"]);
        git(&inner, &["init", "--initial-branch=main"]);
        commit(&inner, "inner");
        std::fs::write(inner.join(".git").join("HEAD"), "").unwrap();
        assert!(
            GitCli.delete_branch(&path_str(&inner), "merged").is_err(),
            "{name}: an outer git directory inside the nested repository is not its own"
        );
        git(&outer, &["rev-parse", "--verify", "refs/heads/merged"]);

        // An outer git directory whose name ends in a carriage return or a line feed, around a
        // nested repository whose `.git` links to one named the same without it: git's answer
        // is read up to its one line end and no further.
        let unread = around.path().join(format!("{name}3")).join("store");
        git(around.path(), &["init", "--bare", unread.to_str().unwrap()]);
        std::fs::write(unread.join("HEAD"), "").unwrap();
        for (end, ends) in [("\r", "a carriage return"), ("\n", "a line feed")] {
            let store = around
                .path()
                .join(format!("{name}3"))
                .join(format!("store{end}"));
            git(
                around.path(),
                &[
                    "clone",
                    "--quiet",
                    "--bare",
                    outer.to_str().unwrap(),
                    store.to_str().unwrap(),
                ],
            );
            let store_checkout = around
                .path()
                .join(format!("{name}3-checkout"))
                .join(ends.replace(' ', "-"));
            git(
                &store,
                &[
                    "worktree",
                    "add",
                    "--detach",
                    store_checkout.to_str().unwrap(),
                ],
            );
            let inner = store.join("inner");
            std::fs::create_dir(&inner).unwrap();
            std::os::unix::fs::symlink(&unread, inner.join(".git")).unwrap();
            assert!(
                GitCli.delete_branch(&path_str(&inner), "merged").is_err(),
                "{name}: delete_branch under a git directory whose name ends in {ends}"
            );
            assert!(
                GitCli
                    .remove_worktree(&path_str(&inner), &path_str(&store_checkout))
                    .is_err(),
                "{name}: remove_worktree under a git directory whose name ends in {ends}"
            );
            git(&store, &["rev-parse", "--verify", "refs/heads/merged"]);
            assert!(
                store_checkout.exists(),
                "{name}: the outer repository's checkout is still there"
            );
        }
    }
}

#[test]
fn a_call_that_needs_a_work_tree_gets_gits_words_in_a_bare_repository() {
    // The other side of the test above: only `identify` reads `must be run in a work tree`
    // as "no checkout here". `is_dirty` asked of a bare repository is asked of something
    // with no working tree to be dirty, and git's own words are the error.
    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "--bare", "-q"]);
    let error = format!("{:#}", GitCli.is_dirty(&path_str(bare.path())).unwrap_err());
    assert!(
        error.contains("this operation must be run in a work tree"),
        "git's words: {error}"
    );
}

#[test]
fn a_detached_checkout_reports_no_branch() {
    let repo = repository();
    let head = GitCli.head_ref(&path_str(repo.path())).unwrap();
    git(repo.path(), &["checkout", "--detach", &head]);
    let identity = GitCli.identify(&path_str(repo.path())).unwrap().unwrap();
    assert_eq!(identity.branch, None, "a detached HEAD names no branch");
}

#[test]
fn a_repository_with_no_commit_yet_is_still_a_repository() {
    // `rev-parse --abbrev-ref HEAD` exits 128 on a HEAD with nothing behind it, and taking
    // that for a refusal put a new repository's panes that git was asked about under
    // `not placed`.
    let dir = tempfile::tempdir().expect("a temp dir");
    git(dir.path(), &["init", "--initial-branch=main"]);
    let root = path_str(dir.path());
    let identity = GitCli
        .identify(&root)
        .expect("git named the repository")
        .expect("and it is one");
    assert_eq!(
        identity.checkout_path,
        path_str(&dir.path().canonicalize().unwrap())
    );
    assert_eq!(identity.branch.as_deref(), Some("main"));
}

#[test]
fn reads_local_and_remote_refs_without_confusing_the_two() {
    let repo = repository();
    let root = path_str(repo.path());
    // A fake remote, so this needs no network.
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "--initial-branch=main"]);
    git(
        repo.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(repo.path(), &["push", "-q", "origin", "main"]);
    git(repo.path(), &["fetch", "-q", "origin"]);

    let refs = GitCli.local_refs(&root).unwrap().refs;
    let local: Vec<_> = refs
        .iter()
        .filter(|r| r.kind == RefKind::Local)
        .map(|r| r.name.as_str())
        .collect();
    // `feat/login` contains a slash, which is exactly what makes `refname:short` ambiguous
    // with a remote ref printed as `origin/main`.
    assert!(local.contains(&"main"), "got {local:?}");
    assert!(local.contains(&"feat/login"), "got {local:?}");

    let remote_refs: Vec<_> = refs
        .iter()
        .filter(|r| r.kind == RefKind::Remote)
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(
        remote_refs,
        ["main"],
        "the remote prefix is stripped, leaving the branch as it exists upstream"
    );

    assert!(
        refs.iter().any(|r| r.subject.as_deref() == Some("first")),
        "the tip commit's subject comes through"
    );
    assert!(refs.iter().all(|r| r.committed_at.is_some()));
}

#[test]
fn lists_remote_heads_and_fetches_one_into_a_usable_base() {
    let repo = repository();
    let root = path_str(repo.path());
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "--initial-branch=main"]);
    git(
        repo.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(repo.path(), &["push", "-q", "origin", "main", "feat/login"]);

    let mut heads = GitCli.remote_heads(&root).unwrap();
    heads.sort();
    assert_eq!(heads, ["feat/login", "main"]);

    // `git push` writes the remote-tracking refs as a side effect, so drop them to get the
    // state this test is actually about: a branch that exists on the remote and has never
    // been fetched into this clone.
    git(
        repo.path(),
        &["update-ref", "-d", "refs/remotes/origin/main"],
    );
    git(
        repo.path(),
        &["update-ref", "-d", "refs/remotes/origin/feat/login"],
    );
    assert!(
        !GitCli
            .local_refs(&root)
            .unwrap()
            .refs
            .iter()
            .any(|r| r.kind == RefKind::Remote),
        "there should be no local ref to base a worktree on yet"
    );

    GitCli.fetch_branch(&root, "feat/login").unwrap();

    // The point of the fetch: `origin/feat/login` now exists, so the worktree can be cut
    // from the branch that is actually on the remote rather than from HEAD.
    let output = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["rev-parse", "--verify", "refs/remotes/origin/feat/login"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fetch should have written the remote-tracking ref"
    );
}

#[test]
fn fetching_the_repository_updates_every_branch_and_drops_the_ones_that_are_gone() {
    let repo = repository();
    let root = path_str(repo.path());
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "--initial-branch=main"]);
    git(
        repo.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(repo.path(), &["push", "-q", "origin", "main", "feat/login"]);
    git(
        repo.path(),
        &["update-ref", "-d", "refs/remotes/origin/feat/login"],
    );

    // A branch the remote has and this clone has never fetched.
    GitCli.fetch_all(&root).unwrap();
    let refs = GitCli.local_refs(&root).unwrap().refs;
    let fetched = refs
        .iter()
        .find(|r| r.name == "feat/login" && r.kind == RefKind::Remote);
    assert!(
        fetched.is_some_and(|r| r.committed_at.is_some() && r.subject.is_some()),
        "the fetch is what gives a remote branch a date and a subject: {refs:?}"
    );

    // And one it no longer has.
    let remote_repo = remote.path().to_str().unwrap().to_string();
    Command::new("git")
        .arg("--git-dir")
        .arg(&remote_repo)
        .args(["branch", "-D", "feat/login"])
        .output()
        .unwrap();

    GitCli.fetch_all(&root).unwrap();
    assert!(
        !GitCli
            .local_refs(&root)
            .unwrap()
            .refs
            .iter()
            .any(|r| r.name == "feat/login" && r.kind == RefKind::Remote),
        "--prune is what keeps a deleted branch from haunting the list"
    );
}

#[test]
fn a_fetch_that_cannot_reach_the_remote_says_so_rather_than_blaming_the_repository() {
    // git exits 128 for every fatal error, so treating that code alone as "not a git
    // repository" turns an unreachable remote into a diagnosis about the wrong thing —
    // and the picker puts that message in front of the user.
    let repo = repository();
    let root = path_str(repo.path());
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "--initial-branch=main"]);
    git(
        repo.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(repo.path(), &["push", "-q", "origin", "feat/login"]);
    drop(remote);

    let error = GitCli
        .fetch_branch(&root, "feat/login")
        .expect_err("the remote is gone");
    let error = format!("{error:#}");
    assert!(
        error.contains("Could not read from remote repository")
            || error.contains("does not appear to be a git repository"),
        "the failure should be git's own account of it, got: {error}"
    );
    assert!(
        !error.contains("not a git repository"),
        "the repository is fine; the remote is not: {error}"
    );
}

#[test]
fn removing_a_worktree_takes_the_checkout_and_leaves_the_branch() {
    let repo = repository();
    let root = path_str(repo.path());
    // In a temp dir of its own, so a failed run leaves nothing behind in the shared one.
    let elsewhere = tempfile::tempdir().unwrap();
    let checkout = elsewhere.path().join("gone-worktree");
    git(
        repo.path(),
        &["worktree", "add", checkout.to_str().unwrap(), "feat/login"],
    );
    assert!(checkout.join("README.md").exists());

    GitCli
        .remove_worktree(&root, checkout.to_str().unwrap())
        .unwrap();

    assert!(!checkout.exists(), "the checkout is gone from disk");
    let listed = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["worktree", "list"])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&listed.stdout).contains("gone-worktree"),
        "and gone from git's record of them"
    );
    assert!(
        GitCli
            .local_refs(&root)
            .unwrap()
            .refs
            .iter()
            .any(|r| r.name == "feat/login"),
        "the branch it was on is not the picker's to delete"
    );
}

#[test]
fn deleting_a_branch_git_calls_merged_takes_it() {
    // `feat/login` sits at `main`'s commit, and its worktree comes and goes first because
    // that is the order a sweep runs the two in.
    let repo = repository();
    let root = path_str(repo.path());
    let elsewhere = tempfile::tempdir().unwrap();
    let checkout = elsewhere.path().join("merged-worktree");
    git(
        repo.path(),
        &["worktree", "add", checkout.to_str().unwrap(), "feat/login"],
    );
    GitCli
        .remove_worktree(&root, checkout.to_str().unwrap())
        .unwrap();

    GitCli.delete_branch(&root, "feat/login").unwrap();

    assert!(
        !GitCli
            .local_refs(&root)
            .unwrap()
            .refs
            .iter()
            .any(|r| r.name == "feat/login"),
        "the branch is gone"
    );
}

#[test]
fn a_branch_git_does_not_call_merged_is_kept_and_git_says_why() {
    // The case ADR 0011 names: the branch's change is on `main` by a squash merge, no commit
    // of the branch is, and `-d` declines — which is the sweep's `branch kept`.
    let repo = repository();
    let root = path_str(repo.path());
    let elsewhere = tempfile::tempdir().unwrap();
    let checkout = elsewhere.path().join("squashed-worktree");
    git(
        repo.path(),
        &["worktree", "add", checkout.to_str().unwrap(), "feat/login"],
    );
    std::fs::write(checkout.join("login.rs"), "fn login() {}\n").unwrap();
    git(&checkout, &["add", "."]);
    git(&checkout, &["commit", "-q", "-m", "login"]);
    git(repo.path(), &["merge", "--squash", "feat/login"]);
    git(repo.path(), &["commit", "-q", "-m", "squash"]);
    assert!(
        repo.path().join("login.rs").exists(),
        "the squash put the change on main"
    );
    assert!(!GitCli.is_dirty(&root).unwrap(), "and committed it there");
    GitCli
        .remove_worktree(&root, checkout.to_str().unwrap())
        .unwrap();

    let error = GitCli
        .delete_branch(&root, "feat/login")
        .expect_err("git does not call a squash merge merged");
    let error = format!("{error:#}");
    assert!(
        error.contains("not fully merged"),
        "git's own reason should reach the user: {error}"
    );
    assert!(
        GitCli
            .local_refs(&root)
            .unwrap()
            .refs
            .iter()
            .any(|r| r.name == "feat/login"),
        "and the branch is still there"
    );
}

#[test]
fn removing_a_worktree_with_uncommitted_work_refuses_and_says_why() {
    let repo = repository();
    let root = path_str(repo.path());
    let elsewhere = tempfile::tempdir().unwrap();
    let checkout = elsewhere.path().join("dirty-worktree");
    git(
        repo.path(),
        &["worktree", "add", checkout.to_str().unwrap(), "feat/login"],
    );
    std::fs::write(checkout.join("README.md"), "edited but not committed\n").unwrap();

    let error = GitCli
        .remove_worktree(&root, checkout.to_str().unwrap())
        .expect_err("git should refuse to throw work away");
    let error = format!("{error:#}");
    assert!(
        error.contains("contains modified or untracked files"),
        "git's own reason should reach the user: {error}"
    );
    assert!(
        checkout.join("README.md").exists(),
        "and nothing should have been deleted"
    );
}

#[test]
fn the_main_checkout_is_not_a_worktree_that_can_be_removed() {
    // The picker refuses before asking, but the adapter must not pretend otherwise either.
    let repo = repository();
    let root = path_str(repo.path());
    assert!(GitCli.remove_worktree(&root, &root).is_err());
}

#[test]
fn recognises_a_github_origin_and_ignores_anything_else() {
    let repo = repository();
    let root = path_str(repo.path());

    assert_eq!(GitCli.github_slug(&root).unwrap(), None, "no origin yet");

    git(
        repo.path(),
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:ShoMasegi/herdr-worktree-nav.git",
        ],
    );
    assert_eq!(
        GitCli
            .github_slug(&root)
            .unwrap()
            .as_ref()
            .map(Slug::as_str),
        Some("ShoMasegi/herdr-worktree-nav")
    );

    git(
        repo.path(),
        &[
            "remote",
            "set-url",
            "origin",
            "git@gitlab.com:owner/repo.git",
        ],
    );
    assert_eq!(
        GitCli.github_slug(&root).unwrap(),
        None,
        "a non-GitHub remote falls back to the directory name elsewhere"
    );
}

#[test]
fn a_config_git_cannot_parse_is_not_a_missing_remote() {
    // Every way git failed to name the repository used to reach the sweep as "no GitHub
    // remote to ask about" — a reader whose `.git/config` git cannot parse was told to look
    // at their remotes. Issue #27.
    let repo = repository();
    let root = path_str(repo.path());
    let config = repo.path().join(".git").join("config");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str("[remote \"origin\"\n");
    std::fs::write(&config, text).unwrap();

    let error = GitCli
        .github_slug(&root)
        .expect_err("git could not parse its own config");
    let words = format!("{error:#}");
    assert!(
        words.starts_with("fatal: bad config line"),
        "git's own words, first: {words}"
    );
    // And the call after them, whole. `refusal` is also what turns a non-zero exit with
    // nothing on stderr into `git said nothing (…)`, so this is what keeps the sentence
    // from ending at the colon.
    assert!(
        words.ends_with("(`git remote get-url origin`)"),
        "the call after them: {words}"
    );
}

#[test]
fn a_repository_root_that_has_gone_is_not_a_missing_remote() {
    // A mount dropped, or another session removed the checkout: git cannot change to it,
    // and that is not "no remote" either.
    let gone = tempfile::tempdir().unwrap();
    let root = path_str(gone.path());
    drop(gone);

    let error = GitCli
        .github_slug(&root)
        .expect_err("there is no repository there to name");
    let words = format!("{error:#}");
    assert!(
        words.starts_with("fatal: cannot change to"),
        "git's own words, first: {words}"
    );
    // And the call after them, whole. `refusal` is also what turns a non-zero exit with
    // nothing on stderr into `git said nothing (…)`, so this is what keeps the sentence
    // from ending at the colon.
    assert!(
        words.ends_with("(`git remote get-url origin`)"),
        "the call after them: {words}"
    );
}

#[test]
fn head_ref_names_the_branch_and_falls_back_to_a_commit_when_detached() {
    let repo = repository();
    let root = path_str(repo.path());
    assert_eq!(GitCli.head_ref(&root).unwrap(), "main");

    let commit = GitCli.head_ref(&root).unwrap();
    git(repo.path(), &["checkout", "--detach", &commit]);
    let detached = GitCli.head_ref(&root).unwrap();
    assert_ne!(
        detached, "HEAD",
        "\"HEAD\" is not something a worktree can be based on"
    );
    assert_eq!(detached.len(), 40, "a full commit id: {detached}");
}

#[test]
fn worktree_heads_reads_each_commit_off_the_registration_rather_than_the_checkout() {
    // All that tells a checkout naming no branch from the next one at its path — issue #29.
    // Read where git keeps the registration, a directory that has gone and one that lost its
    // `.git` answer with the commit they were made at, and not with nothing or with the
    // commit of the repository around them; an unborn branch and a ref git could not read
    // answer with no commit, and a registration whose `HEAD` is cut short with none git
    // resolved.
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;

    let commit_of = |rev: &str, repo: &Path| {
        let said = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", rev])
            .output()
            .expect("git should run");
        String::from_utf8(said.stdout).unwrap().trim().to_string()
    };
    let repo = repository();
    let around = tempfile::tempdir().unwrap();
    let add = |args: &[&str]| git(repo.path(), &[&["worktree", "add"][..], args].concat());
    let first = commit_of("main", repo.path());

    // A space in a path is part of it, at its end too: the line is `worktree <path>`, and
    // nothing more.
    let detached = around.path().join("a detached one ");
    let gone = around.path().join("gone");
    let nested = repo.path().join(".wt").join("x");
    let cut = around.path().join("cut");
    for checkout in [&detached, &gone, &nested, &cut] {
        add(&["--detach", checkout.to_str().unwrap(), "main"]);
    }
    let orphan = around.path().join("orphan");
    add(&["--orphan", "-b", "orphan", orphan.to_str().unwrap()]);
    git(repo.path(), &["branch", "feat2/x"]);
    let unread = around.path().join("feat2-x");
    add(&[unread.to_str().unwrap(), "feat2/x"]);
    let at = |checkout: &Path| path_str(checkout);
    let listed = [
        (at(&detached), ListedHead::At(first.clone())),
        (at(&gone), ListedHead::At(first.clone())),
        (at(&nested), ListedHead::At(first.clone())),
        (at(&orphan), ListedHead::NoCommit),
        (at(&unread), ListedHead::NoCommit),
        (at(&cut), ListedHead::Unresolved),
    ];

    git(repo.path(), &["commit", "--allow-empty", "-m", "second"]);
    let second = commit_of("main", repo.path());
    assert_ne!(first, second);
    std::fs::rename(&gone, around.path().join("moved-away")).expect("the directory moves");
    std::fs::rename(nested.join(".git"), around.path().join("x.git")).expect("the file moves");
    std::fs::write(
        repo.path().join(".git/worktrees/cut/HEAD"),
        format!("{}\n", &first[..12]),
    )
    .expect("the registration's HEAD is written");
    let locked = repo.path().join(".git/refs/heads/feat2");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _put_back = ReadableAgain(&locked);
    assert!(
        std::fs::read_dir(&locked).is_err(),
        "this user can read a directory with mode 000, so git will not be refused and there \
         is nothing here to measure. Run this as a user permissions apply to."
    );

    let root = path_str(repo.path());
    assert_eq!(
        GitCli
            .worktree_heads(&root)
            .expect("git listed the checkouts"),
        BTreeMap::from_iter(
            [(root.clone(), ListedHead::At(second))]
                .into_iter()
                .chain(listed)
        )
    );

    let elsewhere = tempfile::tempdir().unwrap();
    assert!(
        GitCli.worktree_heads(&path_str(elsewhere.path())).is_err(),
        "a directory that is no repository is refused rather than listed as empty"
    );
}

#[test]
fn worktree_heads_reads_a_sha256_null_id_as_no_commit() {
    // The null object id is as long as the repository's hash: 64 zeros under sha256.
    let repo = tempfile::tempdir().unwrap();
    git(
        repo.path(),
        &["init", "--object-format=sha256", "--initial-branch=main"],
    );
    git(
        repo.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "first",
        ],
    );
    let around = tempfile::tempdir().unwrap();
    let orphan = around.path().join("orphan");
    git(
        repo.path(),
        &[
            "worktree",
            "add",
            "--orphan",
            "-b",
            "orphan",
            orphan.to_str().unwrap(),
        ],
    );

    let heads = GitCli.worktree_heads(&path_str(repo.path())).unwrap();
    assert_eq!(
        heads.get(&path_str(&orphan)),
        Some(&ListedHead::NoCommit),
        "{heads:?}"
    );
    assert!(
        matches!(&heads[&path_str(repo.path())], ListedHead::At(id) if id.len() == 64),
        "a sha256 id: {heads:?}"
    );
}

#[test]
fn worktree_heads_leaves_a_bare_repository_out() {
    // A bare repository's own entry is `bare`, with no `HEAD` line: nothing is checked out
    // there, so it is not a checkout at no commit either.
    let repo = repository();
    let around = tempfile::tempdir().unwrap();
    let bare = around.path().join("bare.git");
    git(
        around.path(),
        &[
            "clone",
            "--bare",
            repo.path().to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let detached = around.path().join("detached");
    git(
        &bare,
        &[
            "worktree",
            "add",
            "--detach",
            detached.to_str().unwrap(),
            "main",
        ],
    );

    let heads = GitCli.worktree_heads(&path_str(&bare)).unwrap();
    let listed: Vec<(&str, bool)> = heads
        .iter()
        .map(|(path, head)| (path.as_str(), matches!(head, ListedHead::At(_))))
        .collect();
    assert_eq!(listed, [(path_str(&detached).as_str(), true)]);
}

/// A repository with an `origin` that is a real bare repository on disk, so upstream
/// tracking is exercised without a network.
fn with_origin() -> (TempDir, TempDir) {
    let repo = repository();
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "--initial-branch=main"]);
    git(
        repo.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(repo.path(), &["push", "-q", "-u", "origin", "main"]);
    (repo, remote)
}

fn track_of(refs: &[herdr_worktree_nav::port::GitRef], name: &str) -> Option<Track> {
    refs.iter()
        .find(|r| r.kind == RefKind::Local && r.name == name)
        .unwrap_or_else(|| panic!("no local ref {name}"))
        .track
}

fn upstream_of(refs: &[herdr_worktree_nav::port::GitRef], name: &str) -> Option<String> {
    refs.iter()
        .find(|r| r.kind == RefKind::Local && r.name == name)
        .unwrap_or_else(|| panic!("no local ref {name}"))
        .upstream
        .clone()
}

#[test]
fn a_branch_level_with_its_upstream_has_nothing_to_report() {
    let (repo, _remote) = with_origin();
    let refs = GitCli.local_refs(&path_str(repo.path())).unwrap().refs;
    assert_eq!(track_of(&refs, "main"), None);
    // Never pushed, so it has no upstream at all — also nothing to say, and in particular
    // not "gone".
    assert_eq!(track_of(&refs, "feat/login"), None);

    // The same `None` twice, and the upstream's name is what tells them apart: `dump`
    // reads it, since a row with no marker cannot say which of the two it is.
    assert_eq!(upstream_of(&refs, "main").as_deref(), Some("origin/main"));
    assert_eq!(upstream_of(&refs, "feat/login"), None);
}

#[test]
fn ahead_and_behind_come_out_of_the_ref_walk() {
    let (repo, remote) = with_origin();
    // One commit here that origin does not have.
    std::fs::write(repo.path().join("local.txt"), "local\n").unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-q", "-m", "local only"]);
    assert_eq!(
        track_of(
            &GitCli.local_refs(&path_str(repo.path())).unwrap().refs,
            "main"
        ),
        Some(Track::Ahead(NonZeroU32::new(1).unwrap()))
    );

    // And one on origin that this repository does not have, made through a second clone so
    // the first one is genuinely behind.
    let other = tempfile::tempdir().unwrap();
    let other = other.path().join("clone");
    git(
        Path::new("/"),
        &[
            "clone",
            "-q",
            remote.path().to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    git(&other, &["config", "user.email", "test@example.com"]);
    git(&other, &["config", "user.name", "Test"]);
    std::fs::write(other.join("theirs.txt"), "theirs\n").unwrap();
    git(&other, &["add", "."]);
    git(&other, &["commit", "-q", "-m", "theirs"]);
    git(&other, &["push", "-q", "origin", "main"]);

    git(repo.path(), &["fetch", "-q", "origin"]);
    assert_eq!(
        track_of(
            &GitCli.local_refs(&path_str(repo.path())).unwrap().refs,
            "main"
        ),
        Some(Track::Diverged {
            ahead: NonZeroU32::new(1).unwrap(),
            behind: NonZeroU32::new(1).unwrap()
        })
    );
}

#[test]
fn a_branch_nobody_has_pushed_is_never_gone() {
    // Under `push.default = current` the push destination of a never-pushed branch is a ref
    // that has never existed, and git reports that as `[gone]` in `%(push:track)`. Believing
    // it would put the finished-with marker on the newest branch in the repository — and on
    // exactly the branches a sweep could not undo deleting, since they exist nowhere else.
    let (repo, _remote) = with_origin();
    git(repo.path(), &["config", "push.default", "current"]);

    let refs = GitCli.local_refs(&path_str(repo.path())).unwrap().refs;
    assert_eq!(track_of(&refs, "feat/login"), None);
}

#[test]
fn a_branch_level_with_its_upstream_is_not_marked_with_the_push_destinations_delta() {
    // `%(upstream:track)` is empty for a branch level with what it tracks, and that emptiness
    // is git's answer rather than an absence of one. Reading the push field there measures
    // against a ref the user never asked about — `remote.pushDefault` names a second remote
    // here — and draws the difference beside the upstream's name, where nothing says which
    // of the two comparisons is on screen.
    let (repo, _origin) = with_origin();
    let fork = tempfile::tempdir().unwrap();
    git(fork.path(), &["init", "--bare", "--initial-branch=main"]);
    git(
        repo.path(),
        &["remote", "add", "fork", fork.path().to_str().unwrap()],
    );
    git(repo.path(), &["push", "-q", "-u", "origin", "feat/login"]);
    git(repo.path(), &["push", "-q", "fork", "feat/login"]);

    // Commits that reach the upstream and not the push destination, so the two refs answer
    // differently about the same branch.
    git(repo.path(), &["checkout", "-q", "feat/login"]);
    git(
        repo.path(),
        &["commit", "-q", "--allow-empty", "-m", "work"],
    );
    git(repo.path(), &["push", "-q", "origin", "feat/login"]);

    git(repo.path(), &["config", "push.default", "current"]);
    git(repo.path(), &["config", "remote.pushDefault", "fork"]);

    let refs = GitCli.local_refs(&path_str(repo.path())).unwrap().refs;
    assert_eq!(
        upstream_of(&refs, "feat/login").as_deref(),
        Some("origin/feat/login"),
        "the branch tracks origin, which is the comparison a marker would be about"
    );
    assert_eq!(
        track_of(&refs, "feat/login"),
        None,
        "level with origin/feat/login, whatever fork/feat/login is behind by"
    );
}

#[test]
fn an_upstream_deleted_on_the_remote_reads_as_gone() {
    // The ordinary end of a branch whose pull request was merged: GitHub deletes the head,
    // a pruning fetch drops the remote-tracking ref, and git starts calling it gone. It is
    // the marker the sweep will be built on.
    let (repo, remote) = with_origin();
    git(repo.path(), &["push", "-q", "-u", "origin", "feat/login"]);
    git(remote.path(), &["branch", "-D", "feat/login"]);
    git(repo.path(), &["fetch", "-q", "--prune", "origin"]);

    let refs = GitCli.local_refs(&path_str(repo.path())).unwrap().refs;
    assert_eq!(track_of(&refs, "feat/login"), Some(Track::Gone));
    assert_eq!(track_of(&refs, "main"), None, "only the deleted one");
}

#[test]
fn a_branch_says_which_checkout_has_it() {
    // This is what ties a branch to a row in the panes view without assuming that two
    // things named `feat/login` are the same one.
    let repo = repository();
    let root = path_str(repo.path());
    let worktree = repo.path().join("wt");
    git(
        repo.path(),
        &["worktree", "add", worktree.to_str().unwrap(), "feat/login"],
    );

    let refs = GitCli.local_refs(&root).unwrap().refs;
    let checked_out = |name: &str| {
        refs.iter()
            .find(|r| r.kind == RefKind::Local && r.name == name)
            .unwrap()
            .worktree_path
            .clone()
    };
    assert_eq!(checked_out("main").as_deref(), Some(root.as_str()));
    assert_eq!(
        checked_out("feat/login").map(|p| path_str(Path::new(&p))),
        Some(path_str(&worktree))
    );
}

#[test]
fn a_repository_that_hides_untracked_files_is_still_asked_about_them() {
    // One line of `status.showUntrackedFiles = no`, anywhere in the chain, and git exits 0
    // with nothing to say about an ordinary untracked file. Read as a clean working tree,
    // that is a sweep closing every pane in the checkout and deleting the file. Issue #68.
    let repo = repository();
    let path = path_str(repo.path());
    std::fs::write(repo.path().join("keep.txt"), "an afternoon\n").unwrap();
    assert!(
        GitCli.is_dirty(&path).expect("git answered"),
        "an untracked file is work, before any configuration is in the way"
    );

    git(repo.path(), &["config", "status.showUntrackedFiles", "no"]);
    assert!(
        GitCli.is_dirty(&path).expect("git answered"),
        "and the plugin's own question is what decides, not the machine's configuration"
    );
}

#[test]
fn what_git_was_told_not_to_look_at_is_not_work_it_can_report() {
    // The limit `docs/en/usage.md` states, as a test: git does not walk an ignored path, and
    // does not stat a file under `assume-unchanged`, so neither reaches this call. What
    // makes it a limit rather than a defect is that `git worktree remove` is blind to both
    // as well — a sweep takes exactly what deleting by hand would have taken. A later
    // `--ignored`, or a second call that reads contents, would change that and should change
    // the page with it.
    let repo = repository();
    let path = path_str(repo.path());
    std::fs::write(repo.path().join(".gitignore"), "node_modules/\n").unwrap();
    git(repo.path(), &["add", ".gitignore"]);
    git(repo.path(), &["commit", "-m", "ignore node_modules"]);

    std::fs::create_dir(repo.path().join("node_modules")).unwrap();
    std::fs::write(repo.path().join("node_modules/.env"), "SECRET=1\n").unwrap();
    assert!(
        !GitCli.is_dirty(&path).expect("git answered"),
        "an ignored path is not reported, and `git worktree remove` does not refuse over one"
    );

    git(
        repo.path(),
        &["update-index", "--assume-unchanged", "README.md"],
    );
    std::fs::write(repo.path().join("README.md"), "edited\n").unwrap();
    assert!(
        !GitCli.is_dirty(&path).expect("git answered"),
        "nor is an edit to a file git was told to stop watching"
    );
}

#[test]
fn a_checkout_git_will_not_look_at_is_an_error_rather_than_a_clean_one() {
    // The whole `Unreadable` state upstream of this rests on the refusal arriving as an
    // `Err`. If it were ever softened into empty output, every checkout would be recorded
    // as clean and nothing above would notice.
    let empty = tempfile::tempdir().unwrap();
    assert!(GitCli.is_dirty(&path_str(empty.path())).is_err());
}

/// Makes a path readable again (mode 755) when the test is over, so `TempDir` can remove it.
struct ReadableAgain<'a>(&'a Path);

impl Drop for ReadableAgain<'_> {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
    }
}

#[test]
fn a_directory_git_could_not_read_is_not_a_clean_working_tree() {
    // `status --porcelain` exits 0 with a directory it cannot open reported on stderr and
    // everything under it absent from stdout. Read from stdout alone, a checkout holding a
    // day of work under such a directory was clean: no marker on the row, and offered by a
    // sweep for deletion on the strength of a silence. Issue #42.
    use std::os::unix::fs::PermissionsExt;
    let repo = repository();
    let root = path_str(repo.path());
    let notes = repo.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(notes.join("work.txt"), "a day of work\n").unwrap();
    assert!(
        GitCli.is_dirty(&root).unwrap(),
        "readable, the untracked file counts"
    );

    std::fs::set_permissions(&notes, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _put_back = ReadableAgain(&notes);
    // Root reads a directory with no permissions, and so does a filesystem that ignores
    // them; git then has nothing to warn about and answers `Ok(true)` for the untracked
    // file. The `expect_err` below would fire, and read as an adapter that had stopped
    // reporting. This says which it was instead.
    assert!(
        std::fs::read_dir(&notes).is_err(),
        "this user can read a directory with mode 000 — a root container, or a filesystem \
         that ignores them — so git will not be refused and there is nothing here to \
         measure. Run this as a user permissions apply to."
    );

    let error = GitCli
        .is_dirty(&root)
        .expect_err("git could not look, and said so");
    let words = format!("{error:#}");
    assert!(
        words.starts_with("warning: could not open directory 'notes/'"),
        "git's own words, first: {words}"
    );
    // And nothing after them. This call is always the same one, so `refusal`'s argv would
    // tell the reader nothing — `unread_paths`' doc says why, and this is what holds it.
    assert!(
        !words.contains("(`git "),
        "the call is not named after them: {words}"
    );
}

#[test]
fn a_directory_git_could_not_read_is_not_answered_for_by_the_work_git_could_see() {
    // Work git can see, beside a directory it could not open. `true` would happen to be the
    // right answer, but it would be one arrived at from half a reading; `is_dirty` says the
    // reading was partial instead, whatever stdout holds.
    use std::os::unix::fs::PermissionsExt;
    let repo = repository();
    let root = path_str(repo.path());
    std::fs::write(repo.path().join("visible.txt"), "seen\n").unwrap();
    let notes = repo.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(notes.join("work.txt"), "a day of work\n").unwrap();
    std::fs::set_permissions(&notes, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _put_back = ReadableAgain(&notes);
    assert!(
        std::fs::read_dir(&notes).is_err(),
        "this user can read a directory with mode 000 — a root container, or a filesystem \
         that ignores them — so there is nothing here to measure. Run this as a user \
         permissions apply to."
    );

    let words = format!(
        "{:#}",
        GitCli
            .is_dirty(&root)
            .expect_err("half a reading is not an answer")
    );
    assert!(
        words.starts_with("warning: could not open directory 'notes/'"),
        "git's own words: {words}"
    );
}

#[test]
fn a_tracked_file_in_a_directory_git_may_list_but_not_search_is_not_a_clean_one() {
    // Mode `r--`: git can read the directory's names but not `lstat` what is in it. With
    // only a tracked file in it, git says `t/f: Permission denied` with no prefix and no
    // warning beside it, exits 0, and lists nothing — so a change to `t/f` read as a clean
    // working tree.
    use std::os::unix::fs::PermissionsExt;
    let repo = repository();
    let root = path_str(repo.path());
    let dir = repo.path().join("t");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("f"), "committed\n").unwrap();
    git(repo.path(), &["add", "t/f"]);
    git(repo.path(), &["commit", "-m", "t"]);
    std::fs::write(dir.join("f"), "a day of work\n").unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o444)).unwrap();
    let _put_back = ReadableAgain(&dir);
    assert!(
        std::fs::metadata(dir.join("f")).is_err(),
        "this user can look inside a directory with mode 444 — a root container, or a \
         filesystem that ignores permissions — so there is nothing here to measure. Run this \
         as a user permissions apply to."
    );

    let words = format!(
        "{:#}",
        GitCli
            .is_dirty(&root)
            .expect_err("git could not look, and said so")
    );
    assert_eq!(words, "t/f: Permission denied", "git's own words, alone");
}

#[test]
fn a_gitignore_git_could_not_read_is_not_a_clean_working_tree() {
    // git reads the tree as though an ignore file it cannot read were empty, and warns
    // `unable to access`. Usually that lists more; but here the file's one rule re-includes
    // what the root's excludes, so unread, the checkout's only work is left out, stdout is
    // empty, and — measured — `git worktree remove` deletes it with an exit of 0.
    use std::os::unix::fs::PermissionsExt;
    let repo = repository();
    let root = path_str(repo.path());
    std::fs::write(repo.path().join(".gitignore"), "*.log\nt/.gitignore\n").unwrap();
    git(repo.path(), &["add", ".gitignore"]);
    git(repo.path(), &["commit", "-m", "ignore"]);
    let dir = repo.path().join("t");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("keep.log"), "a day of work\n").unwrap();
    let ignore = dir.join(".gitignore");
    std::fs::write(&ignore, "!keep.log\n").unwrap();
    assert!(
        GitCli.is_dirty(&root).unwrap(),
        "readable, the re-included file counts"
    );

    std::fs::set_permissions(&ignore, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _put_back = ReadableAgain(&ignore);
    assert!(
        std::fs::read(&ignore).is_err(),
        "this user can read a file with mode 000 — a root container, or a filesystem that \
         ignores permissions — so there is nothing here to measure. Run this as a user \
         permissions apply to."
    );

    let words = format!(
        "{:#}",
        GitCli
            .is_dirty(&root)
            .expect_err("git read the tree by a rule it could not see")
    );
    assert_eq!(
        words, "warning: unable to access 't/.gitignore': Permission denied",
        "git's own words, alone"
    );
}

#[test]
fn a_tracked_file_with_a_colon_in_its_name_is_read_the_same_way() {
    // The same `r--` directory, with `: ` in the tracked file's own name: git says
    // `t/a: b: Permission denied`, and the reason is after the last `: `, not the first.
    use std::os::unix::fs::PermissionsExt;
    let repo = repository();
    let root = path_str(repo.path());
    let dir = repo.path().join("t");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("a: b"), "committed\n").unwrap();
    git(repo.path(), &["add", "t/a: b"]);
    git(repo.path(), &["commit", "-m", "t"]);
    std::fs::write(dir.join("a: b"), "a day of work\n").unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o444)).unwrap();
    let _put_back = ReadableAgain(&dir);
    assert!(
        std::fs::metadata(dir.join("a: b")).is_err(),
        "this user can look inside a directory with mode 444 — a root container, or a \
         filesystem that ignores permissions — so there is nothing here to measure. Run this \
         as a user permissions apply to."
    );

    let words = format!(
        "{:#}",
        GitCli
            .is_dirty(&root)
            .expect_err("git could not look, and said so")
    );
    assert_eq!(words, "t/a: b: Permission denied");
}

#[test]
fn a_tracked_file_under_a_symlink_that_loops_is_not_a_clean_one() {
    // `t` was a directory holding a tracked file, and is now a symlink to itself, which the
    // root's `.gitignore` keeps out of the untracked list. git cannot `lstat` `t/f`, says
    // `t/f: Too many levels of symbolic links` and nothing else, and exits 0 — read from
    // stdout alone, a clean checkout, though its tracked file has gone from under it.
    let repo = repository();
    let root = path_str(repo.path());
    let dir = repo.path().join("t");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("f"), "committed\n").unwrap();
    git(repo.path(), &["add", "t/f"]);
    git(repo.path(), &["commit", "-m", "t"]);
    std::fs::write(repo.path().join(".gitignore"), "t\n").unwrap();
    git(repo.path(), &["add", ".gitignore"]);
    git(repo.path(), &["commit", "-m", "ignore"]);
    std::fs::remove_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink("t", &dir).unwrap();

    let words = format!(
        "{:#}",
        GitCli
            .is_dirty(&root)
            .expect_err("git could not look, and said so")
    );
    assert_eq!(words, "t/f: Too many levels of symbolic links");
}

#[test]
fn a_fsmonitor_hook_that_cannot_run_leaves_the_answer_whole() {
    // git says `fatal: cannot exec '<hook>': Permission denied` and then looks at the whole
    // tree itself, exiting 0 with a complete answer. The words end the way an `lstat` git
    // could not do does, and are not one: read as one, every row of such a repository would
    // say `?`.
    use std::os::unix::fs::PermissionsExt;
    let repo = repository();
    let hook = repo.path().join(".git").join("not-executable");
    std::fs::write(&hook, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o644)).unwrap();
    git(
        repo.path(),
        &["config", "core.fsmonitor", &hook.to_string_lossy()],
    );
    assert!(
        !GitCli
            .is_dirty(&path_str(repo.path()))
            .expect("git could not run the hook and answered anyway"),
        "a clean checkout is clean"
    );
}

#[test]
fn a_gitignore_that_is_a_symlink_is_gits_rule_rather_than_a_file_it_could_not_read() {
    // git does not follow a `.gitignore` in the working tree that is a symlink, and says
    // `warning: unable to access '.gitignore': Too many levels of symbolic links` on every
    // `status` — here with the link's target perfectly readable. Read as a partial answer,
    // every row of such a repository would say `?` for good.
    let repo = repository();
    let shared = repo.path().join("shared");
    std::fs::create_dir(&shared).unwrap();
    std::fs::write(shared.join("ignore"), "*.log\n").unwrap();
    std::os::unix::fs::symlink("shared/ignore", repo.path().join(".gitignore")).unwrap();
    git(repo.path(), &["add", ".gitignore", "shared/ignore"]);
    git(repo.path(), &["commit", "-m", "ignore"]);
    assert!(
        !GitCli
            .is_dirty(&path_str(repo.path()))
            .expect("git answered by its own rules"),
        "a clean checkout is clean"
    );
}

#[test]
fn a_fsmonitor_command_that_is_not_on_the_path_leaves_the_answer_whole() {
    // `core.fsmonitor` naming a command rather than a path, with that command nowhere on
    // the path: git says `error: cannot run <command>: No such file or directory` and looks
    // at the whole tree itself. Read as a partial answer, every row of every repository
    // under such a setting would say `?`.
    let repo = repository();
    let root = path_str(repo.path());
    git(
        repo.path(),
        &["config", "core.fsmonitor", "no-such-fsmonitor-command"],
    );
    assert!(
        !GitCli
            .is_dirty(&root)
            .expect("git could not run it and answered anyway"),
        "a clean checkout is clean"
    );
    std::fs::write(repo.path().join("untracked.txt"), "work\n").unwrap();
    assert!(
        GitCli
            .is_dirty(&root)
            .expect("git could not run it and answered anyway"),
        "and work is still work"
    );
}

#[test]
fn a_warning_that_leaves_nothing_out_is_not_a_tree_git_could_not_read() {
    // A clean exit with something on stderr is not, on its own, a partial answer: git warns
    // about a deprecated setting here, and still looks at everything.
    // Reading any stderr as "could not look" would put `?` on every row of such a repository.
    let repo = repository();
    git(repo.path(), &["config", "core.fsyncObjectFiles", "true"]);
    assert!(
        !GitCli
            .is_dirty(&path_str(repo.path()))
            .expect("git warned and answered"),
        "a clean checkout is clean"
    );
}

#[test]
fn a_directory_git_was_told_to_ignore_is_not_one_it_could_not_read() {
    // The other side of the line above, and the one that decides whether this is usable at
    // all: git does not walk a directory `.gitignore` already excludes, so it has nothing to
    // warn about and the answer is the same as it would have been. A `node_modules` whose
    // permissions got mangled is the common case, and it must not turn every row into `?`.
    use std::os::unix::fs::PermissionsExt;
    let repo = repository();
    let root = path_str(repo.path());
    std::fs::write(repo.path().join(".gitignore"), "node_modules/\n").unwrap();
    git(repo.path(), &["add", ".gitignore"]);
    git(repo.path(), &["commit", "-m", "ignore"]);
    let ignored = repo.path().join("node_modules");
    std::fs::create_dir(&ignored).unwrap();
    std::fs::write(ignored.join("work.txt"), "cached\n").unwrap();

    std::fs::set_permissions(&ignored, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _put_back = ReadableAgain(&ignored);
    assert!(
        std::fs::read_dir(&ignored).is_err(),
        "this user can read a directory with mode 000 — a root container, or a filesystem \
         that ignores them — so there is nothing here to measure. Run this as a user \
         permissions apply to."
    );

    assert!(
        !GitCli
            .is_dirty(&root)
            .expect("git had nothing to say about it"),
        "an ignored directory git never looked into is not one it could not read"
    );
}

#[test]
fn a_ref_git_cannot_read_is_named_and_every_other_ref_is_still_listed() {
    // git drops a broken loose ref with a warning and exits 0. Read as a clean exit that is
    // one checkout with no marker and a repository saying nothing, which is the silence
    // issue #21 was about. The walk carries git's words, and the refs git could read: the
    // branches view reads an empty list as "every branch here is only on the remote".
    let repo = repository();
    std::fs::write(
        repo.path().join(".git/refs/heads/feat/login"),
        "not-a-sha\n",
    )
    .unwrap();

    let walk = GitCli
        .local_refs(&path_str(repo.path()))
        .expect("git listed what it could and exited 0");
    let words = walk.dropped.expect("git said which ref it would not read");
    assert!(
        words.starts_with("warning: ignoring broken ref refs/heads/feat/login"),
        "git's words, and first: {words}"
    );
    assert!(
        words.ends_with("refs/heads/feat/login"),
        "git's words, and only those: the call they came from is left off this one sentence, \
         because it is always this call and its format string is most of the line: {words}"
    );
    let names: Vec<&str> = walk.refs.iter().map(|r| r.name.as_str()).collect();
    assert!(
        names.contains(&"main"),
        "the refs git could read: {names:?}"
    );
    assert!(
        !names.contains(&"feat/login"),
        "and the one it could not is the one that is missing: {names:?}"
    );
}

#[test]
fn every_ref_git_dropped_is_named_and_not_just_the_first() {
    // git says it once per ref, so a repository with two broken refs gets two lines, and the
    // words carry both: a reader told about one, who fixes it and is then told about the
    // next, has been given half of what git already knew.
    let repo = repository();
    git(repo.path(), &["branch", "chore/deps"]);
    for branch in ["feat/login", "chore/deps"] {
        std::fs::write(
            repo.path().join(format!(".git/refs/heads/{branch}")),
            "not-a-sha\n",
        )
        .unwrap();
    }

    let walk = GitCli
        .local_refs(&path_str(repo.path()))
        .expect("git listed what it could and exited 0");
    let words = walk.dropped.expect("git said which refs it would not read");
    for branch in ["feat/login", "chore/deps"] {
        assert!(
            words.contains(&format!("ignoring broken ref refs/heads/{branch}")),
            "{branch} is named: {words}"
        );
    }
}

#[test]
fn a_ref_git_drops_without_a_word_leaves_its_checkout_with_no_branch_named() {
    // The other half of what the test above holds. git skips a directory under
    // `refs/heads` it may not open, exits 0, and says nothing, so `dropped` has nothing to
    // carry. `git worktree list` cannot name the branch either, and herdr, which reads it,
    // lists the checkout as not detached with no branch — measured against herdr 0.9.3.
    // `domain::tree::build` records that as `Branch::Unnamed` rather than as a detached
    // checkout, and `log` in it is what says something is wrong. Issue #32.
    use herdr_worktree_nav::domain::model::{Branch, Position, Refs};
    use herdr_worktree_nav::domain::tree::{build, RepoInput};
    use herdr_worktree_nav::port::{Snapshot, Worktree};
    use std::os::unix::fs::PermissionsExt;

    let repo = repository();
    git(repo.path(), &["branch", "feat2/x"]);
    let checkout = repo.path().join("feat2-x");
    git(
        repo.path(),
        &["worktree", "add", checkout.to_str().unwrap(), "feat2/x"],
    );
    let root = path_str(repo.path());
    let checkout = path_str(&checkout);

    let locked = repo.path().join(".git/refs/heads/feat2");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _put_back = ReadableAgain(&locked);
    // As in `a_directory_git_could_not_read_is_not_a_clean_working_tree`: a user the mode
    // does not apply to reads the directory, and git then drops nothing.
    assert!(
        std::fs::read_dir(&locked).is_err(),
        "this user can read a directory with mode 000, so git will not be refused and there \
         is nothing here to measure. Run this as a user permissions apply to."
    );

    let walk = GitCli
        .local_refs(&root)
        .expect("git listed what it could and exited 0");
    assert_eq!(
        walk.dropped, None,
        "and said nothing about what it left out"
    );
    let names: Vec<&str> = walk.refs.iter().map(|r| r.name.as_str()).collect();
    assert!(
        names.contains(&"main") && !names.contains(&"feat2/x"),
        "the premise: git left `feat2/x` out: {names:?}"
    );

    // What herdr reads for the same repository: `git worktree list`, whose stanza for the
    // checkout has neither a `branch` line nor `detached`.
    let porcelain = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .expect("git should run");
    let porcelain = String::from_utf8_lossy(&porcelain.stdout).to_string();
    let stanza = porcelain
        .split("\n\n")
        .find(|stanza| {
            stanza
                .lines()
                .next()
                .is_some_and(|l| l.ends_with("/feat2-x"))
        })
        .unwrap_or_else(|| panic!("git lists the checkout: {porcelain}"));
    assert!(
        !stanza.contains("\nbranch ") && !stanza.contains("\ndetached"),
        "the premise: git names no branch there and does not call it detached: {stanza}"
    );
    let listed = |branch: Option<&str>, path: &str, linked: bool| Worktree {
        branch: branch.map(str::to_string),
        path: path.to_string(),
        label: String::new(),
        is_bare: false,
        is_detached: Some(false),
        is_linked_worktree: linked,
        is_prunable: false,
        open_workspace_id: None,
    };
    let input = RepoInput {
        repo_key: format!("{root}/.git"),
        repo_root: root.clone(),
        display_name: "app".into(),
        worktrees: vec![
            listed(Some("main"), &root, false),
            listed(None, &checkout, true),
        ],
        refs: Ok(walk.refs),
    };
    let tree = build(&Snapshot::default(), &[input], &Default::default());
    let repo_node = &tree.repos[0];
    assert_eq!(
        repo_node.refs,
        Refs::Read,
        "git answered, for the rest of them"
    );
    let branch_at = |path: &str| {
        repo_node
            .worktrees
            .iter()
            .find(|worktree| worktree.checkout_path == path)
            .map(|worktree| worktree.branch.clone())
    };
    assert_eq!(
        branch_at(&checkout),
        Some(Branch::Unnamed {
            ref_names_path: false
        }),
        "a branch is out and nobody can say which, which is not what a detached checkout reads"
    );
    assert_eq!(
        branch_at(&root),
        Some(Branch::Out {
            name: "main".into(),
            position: Position::Said(None),
        })
    );

    // And git will not vouch for it as a branch with no commit yet, and says why in words of
    // its own, which `app::collect` keeps for the prompt line.
    let words = format!(
        "{:#}",
        GitCli
            .has_no_commit_yet(&checkout)
            .expect_err("git could not read the branch, and said so")
    );
    assert!(
        words.starts_with("fatal: your current branch appears to be broken"),
        "git's own words, first: {words}"
    );
    assert!(
        words.ends_with(" (`git log -1 --format=%H`)"),
        "and the call they answer, as every refusal here carries: {words}"
    );
}

#[test]
fn a_ref_git_cannot_read_but_once_packed_is_listed_at_its_packed_value_with_no_checkout() {
    // The same unreadable directory as above, in a repository whose refs were packed — by
    // `git gc`, among others — before the branch moved on. git does not drop the ref then:
    // it lists what `packed-refs` still holds, says nothing, and names no checkout for it.
    // The checkout reads the same as above, and is asked about the same way. Issue #32.
    use herdr_worktree_nav::domain::model::Branch;
    use herdr_worktree_nav::domain::tree::{build, RepoInput};
    use herdr_worktree_nav::port::{Snapshot, Worktree};
    use std::os::unix::fs::PermissionsExt;

    let repo = repository();
    git(repo.path(), &["branch", "feat2/x"]);
    let checkout = repo.path().join("feat2-x");
    git(
        repo.path(),
        &["worktree", "add", checkout.to_str().unwrap(), "feat2/x"],
    );
    git(repo.path(), &["pack-refs", "--all"]);
    git(&checkout, &["commit", "--allow-empty", "-m", "second"]);
    let root = path_str(repo.path());
    let checkout = path_str(&checkout);

    let locked = repo.path().join(".git/refs/heads/feat2");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _put_back = ReadableAgain(&locked);
    assert!(
        std::fs::read_dir(&locked).is_err(),
        "this user can read a directory with mode 000, so git will not be refused and there \
         is nothing here to measure. Run this as a user permissions apply to."
    );

    let walk = GitCli
        .local_refs(&root)
        .expect("git listed what it could and exited 0");
    assert_eq!(walk.dropped, None, "and said nothing");
    let packed = walk
        .refs
        .iter()
        .find(|git_ref| git_ref.name == "feat2/x")
        .expect("git lists the ref packed-refs holds");
    assert_eq!(
        (packed.subject.as_deref(), packed.worktree_path.as_deref()),
        (Some("first"), None),
        "at the commit it was packed at, out nowhere"
    );

    let porcelain = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .expect("git should run");
    let porcelain = String::from_utf8_lossy(&porcelain.stdout).to_string();
    let stanza = porcelain
        .split("\n\n")
        .find(|stanza| {
            stanza
                .lines()
                .next()
                .is_some_and(|l| l.ends_with("/feat2-x"))
        })
        .unwrap_or_else(|| panic!("git lists the checkout: {porcelain}"));
    assert!(
        !stanza.contains("\nbranch ") && !stanza.contains("\ndetached"),
        "git names no branch there and does not call it detached: {stanza}"
    );

    let listed = |branch: Option<&str>, path: &str, linked: bool| Worktree {
        branch: branch.map(str::to_string),
        path: path.to_string(),
        label: String::new(),
        is_bare: false,
        is_detached: Some(false),
        is_linked_worktree: linked,
        is_prunable: false,
        open_workspace_id: None,
    };
    let input = RepoInput {
        repo_key: format!("{root}/.git"),
        repo_root: root.clone(),
        display_name: "app".into(),
        worktrees: vec![
            listed(Some("main"), &root, false),
            listed(None, &checkout, true),
        ],
        refs: Ok(walk.refs),
    };
    let tree = build(&Snapshot::default(), &[input], &Default::default());
    let row = tree.repos[0]
        .worktrees
        .iter()
        .find(|worktree| worktree.checkout_path == checkout)
        .expect("the checkout has a row");
    assert_eq!(
        row.branch,
        Branch::Unnamed {
            ref_names_path: false
        },
        "the packed ref names no checkout, so it does not name this one"
    );

    let words = format!(
        "{:#}",
        GitCli
            .has_no_commit_yet(&checkout)
            .expect_err("git could not read the branch, and said so")
    );
    assert!(
        words.starts_with("fatal: your current branch appears to be broken"),
        "git's own words, first: {words}"
    );
    assert!(
        words.ends_with(" (`git log -1 --format=%H`)"),
        "and the call they answer, as every refusal here carries: {words}"
    );
}

#[test]
fn a_branch_with_no_commit_yet_is_told_apart_from_one_with_a_commit() {
    // The ordinary way to have a branch out that no ref names: a fresh `git init`, whose
    // `main` `for-each-ref` lists nothing for. Quiet, where the dropped ref above is not.
    let fresh = tempfile::tempdir().unwrap();
    git(
        fresh.path(),
        &[
            "-c",
            "init.defaultRefFormat=files",
            "init",
            "--initial-branch=main",
        ],
    );
    let fresh_root = path_str(fresh.path());
    assert!(
        GitCli
            .local_refs(&fresh_root)
            .expect("an empty walk")
            .refs
            .is_empty(),
        "the premise: no ref names it"
    );
    assert!(GitCli.has_no_commit_yet(&fresh_root).unwrap());

    let repo = repository();
    assert!(!GitCli.has_no_commit_yet(&path_str(repo.path())).unwrap());
}

#[test]
fn a_refusal_puts_gits_words_before_the_call() {
    // The other way round, the `--format=` string puts git's words past the right edge of
    // every prompt line the picker draws.
    let repo = repository();
    std::fs::write(
        repo.path().join(".git/packed-refs"),
        "this is not a packed-refs file\n",
    )
    .unwrap();

    let error = GitCli
        .local_refs(&path_str(repo.path()))
        .expect_err("git cannot read the refs");
    let words = format!("{error:#}");
    assert!(words.starts_with("fatal: "), "git's words first: {words}");
    let call = words
        .find("(`git for-each-ref")
        .expect("the call is named after them");
    assert!(
        words[..call].contains("packed-refs"),
        "and what git said is all before it: {words}"
    );
}

#[test]
fn a_branch_with_no_upstream_is_still_measured_against_where_it_would_push() {
    // Which is what `%(push:track)` is in the format string for. Pushed without `-u`, so
    // there is no upstream to compare against and `%(upstream:track)` says nothing.
    let (repo, _remote) = with_origin();
    git(repo.path(), &["config", "push.default", "current"]);
    git(repo.path(), &["push", "-q", "origin", "feat/login"]);
    std::fs::write(repo.path().join("more.txt"), "more\n").unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-q", "-m", "one more"]);
    git(repo.path(), &["branch", "-f", "feat/login", "HEAD"]);

    assert_eq!(
        track_of(
            &GitCli.local_refs(&path_str(repo.path())).unwrap().refs,
            "feat/login"
        ),
        Some(Track::Ahead(NonZeroU32::new(1).unwrap()))
    );
}

#[test]
fn dirty_means_what_worktree_remove_means_by_it() {
    let repo = repository();
    let root = path_str(repo.path());
    assert!(
        !GitCli.is_dirty(&root).unwrap(),
        "a fresh checkout is clean"
    );

    // An untracked file counts, because it counts to `git worktree remove` — the marker has
    // to mean the same thing there as it does on `Shift-D`.
    std::fs::write(repo.path().join("scratch.txt"), "notes\n").unwrap();
    assert!(GitCli.is_dirty(&root).unwrap(), "untracked files count");

    std::fs::remove_file(repo.path().join("scratch.txt")).unwrap();
    assert!(!GitCli.is_dirty(&root).unwrap());

    std::fs::write(repo.path().join("README.md"), "changed\n").unwrap();
    assert!(GitCli.is_dirty(&root).unwrap(), "and so do modified ones");
}

/// The `%(worktreepath)` of a local ref, for the premises below.
fn worktree_path_of<'a>(
    refs: &'a [herdr_worktree_nav::port::GitRef],
    name: &str,
) -> Option<&'a str> {
    refs.iter()
        .find(|r| r.kind == RefKind::Local && r.name == name)
        .unwrap_or_else(|| panic!("no local ref {name}"))
        .worktree_path
        .as_deref()
}

/// Two worktrees, one of which now sits at the path the other's entry still names. Returns
/// that path.
///
/// The way a person reaches it: one directory moves out of the way and the other moves into
/// its place, leaving `chore/deps`'s own entry naming the directory it left.
fn a_checkout_at_another_entrys_path(repo: &TempDir) -> String {
    git(repo.path(), &["branch", "chore/deps"]);
    let one = repo.path().join("one");
    let two = repo.path().join("two");
    git(
        repo.path(),
        &["worktree", "add", one.to_str().unwrap(), "feat/login"],
    );
    git(
        repo.path(),
        &["worktree", "add", two.to_str().unwrap(), "chore/deps"],
    );
    let shared = path_str(&one);
    std::fs::rename(&one, repo.path().join("aside")).expect("one moves out of the way");
    std::fs::rename(&two, &one).expect("two moves into its place");
    shared
}

#[test]
fn two_repositories_can_name_one_path_and_prune_will_not_part_them() {
    // Half of what `domain::tree::tracks` is keyed by repository for, and all of why
    // `git worktree prune` is not the answer instead. git goes on naming the path a moved
    // worktree had, and a prune in one repository leaves the other's entry alone, so both
    // name the path with nothing to clear it. Issue #31.
    //
    // The prune below is a negative control: deleting it leaves this test green.
    let old = repository();
    let app = repository();
    let shared = old.path().join("shared");
    git(
        old.path(),
        &["worktree", "add", shared.to_str().unwrap(), "feat/login"],
    );
    // Taken before the move: `path_str` canonicalises, and a path that is gone has none.
    let had = path_str(&shared);
    std::fs::rename(&shared, old.path().join("moved-away")).expect("the directory moves");
    git(app.path(), &["worktree", "add", &had, "feat/login"]);

    git(old.path(), &["worktree", "prune"]);

    for repo in [&old, &app] {
        let refs = GitCli
            .local_refs(&path_str(repo.path()))
            .expect("git listed what it could")
            .refs;
        assert_eq!(
            worktree_path_of(&refs, "feat/login"),
            Some(had.as_str()),
            "both repositories name it, after a prune: {:?}",
            refs
        );
    }
}

#[test]
fn a_prune_that_does_clear_a_stale_entry() {
    // The positive control for
    // `two_repositories_can_name_one_path_and_prune_will_not_part_them`: with nothing at the
    // path, prune clears the entry, which is what makes "will not part them" say anything.
    let repo = repository();
    let gone = repo.path().join("gone");
    git(
        repo.path(),
        &["worktree", "add", gone.to_str().unwrap(), "feat/login"],
    );
    let had = path_str(&gone);
    std::fs::rename(&gone, repo.path().join("moved-away")).expect("the directory moves");
    let before = GitCli
        .local_refs(&path_str(repo.path()))
        .expect("git listed what it could")
        .refs;
    assert_eq!(
        worktree_path_of(&before, "feat/login"),
        Some(had.as_str()),
        "git still names it after the move: {before:?}"
    );

    git(repo.path(), &["worktree", "prune"]);

    let refs = GitCli
        .local_refs(&path_str(repo.path()))
        .expect("git listed what it could")
        .refs;
    assert_eq!(
        worktree_path_of(&refs, "feat/login"),
        None,
        "prune cleared it: {refs:?}"
    );
}

#[test]
fn one_repository_can_name_one_path_from_two_refs() {
    // The other half: the second ref `tracks` answers with nothing rather than with
    // either of them is a state git makes, not one only a broken port could hand it.
    let repo = repository();
    let shared = a_checkout_at_another_entrys_path(&repo);
    // Repair rewrites the entry that lost its directory rather than adding one, and what it
    // rewrites it to is the path the other entry already names.
    git(repo.path(), &["worktree", "repair", &shared]);

    let refs = GitCli
        .local_refs(&path_str(repo.path()))
        .expect("git listed what it could")
        .refs;
    let mut naming: Vec<&str> = refs
        .iter()
        .filter(|r| r.kind == RefKind::Local && r.worktree_path.as_deref() == Some(shared.as_str()))
        .map(|r| r.name.as_str())
        .collect();
    naming.sort_unstable();
    assert_eq!(naming, ["chore/deps", "feat/login"], "refs: {:?}", refs);
}

#[test]
fn a_worktree_whose_directory_moved_is_still_listed_at_the_path_it_had() {
    // The first premise `domain::tree::tracks` is keyed by repository for: git does not stop
    // reporting a linked worktree when its directory goes away, it goes on naming the path
    // the worktree had.
    let repo = repository();
    let worktree = repo.path().join("wt");
    git(
        repo.path(),
        &["worktree", "add", worktree.to_str().unwrap(), "feat/login"],
    );
    // Taken before the move: `path_str` canonicalises, and a path that is gone has none.
    let had = path_str(&worktree);
    std::fs::rename(&worktree, repo.path().join("moved-away")).expect("the directory moves");
    // Without this the assertion below is one `path_str` compared with itself: `had` was
    // taken before the move, so it holds whether or not anything moved.
    assert!(
        !worktree.exists(),
        "the directory really is gone from {had}"
    );

    let refs = GitCli.local_refs(&path_str(repo.path())).unwrap().refs;
    let listed = refs
        .iter()
        .find(|r| r.kind == RefKind::Local && r.name == "feat/login")
        .expect("git still lists the branch");
    assert_eq!(
        listed.worktree_path.as_deref(),
        Some(had.as_str()),
        "the path it had, not the one it has: {:?}",
        listed.worktree_path
    );
}

#[test]
fn a_ref_carrying_gone_can_name_a_path_whose_checkout_has_no_branch_out() {
    // Why `domain::model::Branch` keeps a position only on a row that names a branch. Two
    // entries name one path and the checkout sitting there is then detached, so one entry
    // goes on naming a path with nothing out — while the ref that entry is for carries
    // `[gone]`, because its upstream was deleted on the remote. A branchless row with that
    // `[gone]` on it is one `domain::sweep::judge` would offer for deletion by default, and
    // the type is what leaves it nowhere to be carried.
    //
    // This holds git's half. The other half — that herdr reports that path as a checkout
    // with no branch — is an assumption about herdr's API, written down on
    // `port::Worktree::branch`, and there is no herdr in CI to hold it.
    let (repo, _remote) = with_origin();
    let shared = a_checkout_at_another_entrys_path(&repo);
    // Both, because which entry repair binds to the directory is git's own business and not
    // the same everywhere: whichever one is left naming the path has to carry the marker. On
    // a machine where `feat/login` is the one left, the `chore/deps` half is inert.
    git(
        repo.path(),
        &["push", "-q", "-u", "origin", "feat/login", "chore/deps"],
    );
    // Deleting this leaves the test green, and it still has to be here: repair is what makes
    // git list a second, branchless entry at `shared`, which is the half `build` reads and
    // `GitCli` has no way to see.
    git(repo.path(), &["worktree", "repair", &shared]);
    git(
        repo.path(),
        &[
            "push",
            "-q",
            "origin",
            "--delete",
            "feat/login",
            "chore/deps",
        ],
    );
    git(Path::new(&shared), &["checkout", "--detach"]);

    let identity = GitCli
        .identify(&shared)
        .expect("git answered")
        .expect("a repository");
    assert_eq!(
        identity.branch, None,
        "nothing is checked out at {shared} any more"
    );

    let refs = GitCli
        .local_refs(&path_str(repo.path()))
        .expect("git listed what it could")
        .refs;
    let naming: Vec<(&str, Option<Track>)> = refs
        .iter()
        .filter(|r| r.kind == RefKind::Local && r.worktree_path.as_deref() == Some(shared.as_str()))
        .map(|r| (r.name.as_str(), r.track))
        .collect();
    assert_eq!(naming.len(), 1, "one entry goes on naming it: {refs:?}");
    assert_eq!(
        naming[0].1,
        Some(Track::Gone),
        "and the ref it is for carries a marker: {refs:?}"
    );
}
