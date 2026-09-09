//! What the adapter reads when git is not speaking English.
//!
//! A binary of its own, and one test in it, for the reason `git_missing.rs` is: it works by
//! setting `LC_ALL` for the whole process, `std::env::set_var` is process-wide, and cargo
//! runs the tests of one binary on threads that share it. Every other test here reads git's
//! English, so mutating it underneath them is the flake this avoids. Both halves of the pin
//! are checked in the one test for the same reason.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use herdr_worktree_nav::adapter::GitCli;
use herdr_worktree_nav::port::GitPort;

/// The literal `adapter::git_cli::dropped_refs` matches, which is the whole point: a locale
/// this test accepts has to be one where git does not print *this*.
const DROPPED: &str = "warning: ignoring broken ref ";

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

/// A repository with two branches, one of whose refs is broken. The ref format is pinned for
/// the reason `git_adapter.rs` pins it: this writes into `.git/refs`, which the reftable
/// backend does not have.
fn repository_with_a_broken_ref() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path();
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
    git(path, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(path, &["branch", "feat/login"]);
    std::fs::write(path.join(".git/refs/heads/feat/login"), "not-a-sha\n").unwrap();
    dir
}

/// Why there is no locale to test with. Two situations, and only one of them is about
/// locales: a machine with no `locale` was never asked, and saying it had none would be a
/// measurement that did not happen.
enum NoLocale {
    CannotAsk(String),
    NoneTranslates(usize),
}

/// The language part of a locale name, for `LANGUAGE`.
fn language_of(locale: &str) -> &str {
    locale.split(['_', '.']).next().unwrap_or(locale)
}

/// A locale on this machine under which git does not print [`DROPPED`], or why there is none.
///
/// Asked of git, on the message this test asserts. Two things a shorter probe gets wrong.
/// A locale existing is not the same as git having a translation for it — `pt_BR.UTF-8` is
/// generated here and git answers in English under it — so picking by name would pass
/// without ever leaving English. And git's messages are separate translation units: probing
/// with `not a git repository`, which an earlier version of this did, accepts `el_GR`, where
/// that one is translated and this one is not, so the test would then assert English against
/// English and hold nothing.
fn a_locale_git_hides_the_warning_in(broken: &Path) -> Result<String, NoLocale> {
    let listed = Command::new("locale")
        .arg("-a")
        .output()
        .map_err(|error| NoLocale::CannotAsk(format!("`locale -a` could not be run: {error}")))?;
    let status = listed.status;
    let complained = String::from_utf8_lossy(&listed.stderr).trim().to_string();
    let listed = String::from_utf8_lossy(&listed.stdout);
    let names: Vec<&str> = listed
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    // A list is a list whatever the exit code: `locale -a` can name every locale here and
    // still complain about one of them, and refusing it would throw away a measurement that
    // would have worked. No list at all is the thing that cannot be worked with, and it is
    // "could not ask" rather than "nothing is translated" — the other arm would report a
    // measurement that never happened.
    if names.is_empty() {
        return Err(NoLocale::CannotAsk(format!(
            "`locale -a` listed nothing ({status}): {complained}"
        )));
    }
    let mut in_english = 0;
    for name in &names {
        let said = Command::new("git")
            .arg("-C")
            .arg(broken)
            .args(["for-each-ref", "refs/heads"])
            .env("LC_ALL", name)
            .env("LANGUAGE", language_of(name))
            .output();
        let Ok(said) = said else { continue };
        let words = String::from_utf8_lossy(&said.stderr);
        if words.contains(DROPPED) {
            in_english += 1;
        } else if !words.trim().is_empty() {
            return Ok((*name).to_string());
        }
    }
    Err(NoLocale::NoneTranslates(in_english))
}

/// The process's locale, put back the way it was when this goes out of scope — including
/// when an assertion between here and there panics, which is what a guard buys over two
/// `remove_var` calls at the end.
struct Locale {
    lc_all: Option<OsString>,
    language: Option<OsString>,
}

impl Locale {
    fn set(name: &str) -> Self {
        let kept = Locale {
            lc_all: std::env::var_os("LC_ALL"),
            language: std::env::var_os("LANGUAGE"),
        };
        // `LANGUAGE` as well, because it outranks `LC_ALL` for a reader who has both set —
        // and is ignored under the `C` the adapter pins, which is the half being tested.
        std::env::set_var("LC_ALL", name);
        std::env::set_var("LANGUAGE", language_of(name));
        kept
    }
}

impl Drop for Locale {
    fn drop(&mut self) {
        for (name, kept) in [
            ("LC_ALL", self.lc_all.take()),
            ("LANGUAGE", self.language.take()),
        ] {
            match kept {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

#[test]
fn git_is_read_in_the_language_its_messages_are_matched_in() {
    // The adapter decides two things by reading git's English: whether a ref was left out of
    // a walk, and whether a path is a repository at all. git translates both. Read in the
    // locale herdr launched the plugin in, a German git says `Warnung: Ignoriere fehlerhafte
    // Referenz …`, the walk comes back claiming to be whole, and the checkout on that ref
    // carries no marker with nothing anywhere to say why — the silence issue #21 is about.
    // `GIT_LOCALE` is what keeps git's messages in the language they are matched in.
    let broken = repository_with_a_broken_ref();
    let elsewhere = tempfile::tempdir().expect("a temp dir");
    let locale = match a_locale_git_hides_the_warning_in(broken.path()) {
        Ok(locale) => locale,
        // Neither is a pass this test could earn, and a skip here reads as one: libtest keeps
        // a passing test's output to itself, so nobody would see it.
        Err(NoLocale::CannotAsk(why)) => panic!(
            "this test needs to know which locales exist and could not ask: {why}. \
             Install a working `locale`, or run the suite where there is one."
        ),
        Err(NoLocale::NoneTranslates(asked)) => panic!(
            "no locale on this machine makes this git say anything but the English this test \
             matches: {asked} of them printed it and none printed anything else, so nothing \
             would be measured. Generate one git has a translation for: \
             sudo locale-gen de_DE.UTF-8"
        ),
    };

    let _locale = Locale::set(&locale);

    // The guard has to be alive from here to the end of the test, and one character decides
    // it: `let _ = Locale::set(…)` drops it on that line instead, the two halves below then
    // run in whatever locale the suite was started in, git answers in English, both hold, and
    // the test measures nothing. That is the shape this file was rewritten to remove, so it
    // is checked rather than trusted.
    assert_eq!(
        (std::env::var("LC_ALL").ok(), std::env::var("LANGUAGE").ok()),
        (Some(locale.clone()), Some(language_of(&locale).to_string())),
        "the process has to still be in {locale} for the rest of this test to mean anything"
    );

    // git's half: the ref really is dropped, and the adapter still reads the warning as one.
    let walk = GitCli
        .local_refs(&broken.path().to_string_lossy())
        .expect("git listed what it could and exited 0");
    let words = walk
        .dropped
        .unwrap_or_else(|| panic!("a dropped ref went unnoticed under {locale}"));
    assert!(
        words.starts_with("warning: ignoring broken ref refs/heads/feat/login"),
        "git's English, under {locale}: {words}"
    );
    assert!(
        !walk.refs.iter().any(|git_ref| git_ref.name == "feat/login"),
        "and the ref really was dropped: {:?}",
        walk.refs
    );

    // The other half, which is the same pin: `NOT_A_REPOSITORY` is English too, and a path
    // that is not a repository is an ordinary answer rather than git refusing. Without the
    // pin the German words miss the match, `run` bails, and this is an `Err`.
    assert!(
        matches!(
            GitCli.identify(&elsewhere.path().to_string_lossy()),
            Ok(None)
        ),
        "a path that is not a repository is not a refusal, under {locale}"
    );
}
