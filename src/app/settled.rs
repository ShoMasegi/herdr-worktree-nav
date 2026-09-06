//! What `gh` says has become of each repository's pull requests.
//!
//! Asked when a sweep is entered rather than when the picker opens, because it is the
//! heavier of the two `gh` calls — a window over everything that has landed rather than a
//! glance at what is in flight — and because most sessions never sweep. That is the sentence
//! in `docs/adr/0011-what-may-be-swept.md` this carries out.
//!
//! One thread per repository, not per checkout. Repositories are however many the user has
//! panes open in, which is a handful, so there is nothing here to cap — unlike
//! `app::dirty`, where every checkout costs a process of its own.
//!
//! A repository that has not answered yet is absent from the map, which is not the same as
//! present with `None`: `domain::sweep::Facts` reads the first as "nobody has asked" and the
//! second as "asked, and `gh` could not say", and only the second is worth a word on a row.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use crate::domain::model::Tree;
use crate::domain::sweep::RepoRoot;
use crate::port::{GhPort, GitPort, SettledPullRequests};

/// One repository's reply: the round of asking it belongs to, the root it is about, and
/// what came of asking.
type Reply = (u64, String, Answer);

/// What is known about one repository's pull requests.
///
/// Four things, each named, so that every reader says what it does with each rather than
/// testing for the one it wants. This was an `Option<Result<_, String>>`, and
/// `forget_failures` was `!matches!(_, Some(Err(_)))` over it — which answers "keep" for
/// whatever it has not been taught, the shape `domain::sweep::Candidate::is_markable`'s
/// doc argues against. It also had one state for two things: a `gh` that refused, which
/// is worth asking again, and a repository with no GitHub remote, which is not.
#[derive(Debug, Clone)]
enum Answer {
    /// Asked, and the call has not come home.
    Asking,
    /// There is nothing to ask: git named no GitHub remote. That does not change while the
    /// picker is up, so it is not asked again on entering a sweep — and on the prompt line
    /// it is outranked by a refusal, which is the one the user can do something about. The
    /// rows still say `PR unknown`, as ADR 0011 asks: nothing has looked.
    Unaskable(String),
    /// Asked, and could not answer: `gh` refused, or git would not name the repository.
    /// Asked again the next time a sweep is entered.
    Refused(String),
    /// What `gh` said.
    Answered(SettledPullRequests),
}

/// What the sweep knows about finished pull requests, and what it is still waiting to hear.
///
/// Owned by the view switch for the reason `Dirty` is: an answer that cost a round of `gh`
/// calls should survive a `Tab` rather than be asked for again.
pub struct Settled {
    git: Arc<dyn GitPort>,
    gh: Arc<dyn GhPort>,
    sender: Sender<Reply>,
    receiver: Receiver<Reply>,
    /// Every repository asked about, by root.
    answers: BTreeMap<String, Answer>,
    /// Which round of asking is current. Bumped by [`forget`](Self::forget), because a `gh`
    /// call started before it was called is answering about pull requests as they were, and
    /// the whole point of `r` is that one may have landed since. The same counter
    /// `app::dirty` keeps, for the same reason: `mpsc` does not deliver in the order the
    /// threads were spawned, so without it the stale round can simply win.
    generation: u64,
}

impl Settled {
    pub fn new(git: Arc<dyn GitPort>, gh: Arc<dyn GhPort>) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            git,
            gh,
            sender,
            receiver,
            answers: BTreeMap::new(),
            generation: 0,
        }
    }

    /// Ask about every repository in the tree that has not been asked about yet, and let go
    /// of what was learned about repositories no longer in it.
    ///
    /// Called on every frame a sweep is on. For a repository already in the map it asks
    /// nothing: a merged pull request does not become unmerged, so a frame — or a sweep left
    /// and re-entered — is a map lookup rather than another round of `gh`. What asks again
    /// is `r`, through [`forget`](Self::forget); entering a sweep, through
    /// [`forget_failures`](Self::forget_failures), where the answer was a refusal; and a
    /// repository that left the list and came back, because what was learned about it went
    /// when it left. A call still out keeps its slot whatever the tree says, so that its
    /// answer has somewhere to land.
    pub fn ask(&mut self, tree: &Tree) {
        let listed: BTreeSet<&str> = tree
            .repos
            .iter()
            .map(|repo| repo.repo_root.as_str())
            .collect();
        self.answers.retain(|root, answer| {
            listed.contains(root.as_str()) || matches!(answer, Answer::Asking)
        });
        for repo in &tree.repos {
            if self.answers.contains_key(&repo.repo_root) {
                continue;
            }
            self.answers.insert(repo.repo_root.clone(), Answer::Asking);
            let git = Arc::clone(&self.git);
            let gh = Arc::clone(&self.gh);
            let sender = self.sender.clone();
            let repo_root = repo.repo_root.clone();
            let generation = self.generation;
            // Not joined, for the reason `app::dirty`'s are not: the answer is wanted on
            // both sides of a `Tab`, and leaving the picker ends the thread with the process.
            std::thread::spawn(move || {
                let answer = match git.github_slug(&repo_root) {
                    // Nothing to ask, and nothing that will change. Still a row that says
                    // so — not a row that looks like it has nothing to find.
                    Ok(None) => Answer::Unaskable("no GitHub remote to ask about".to_string()),
                    Err(error) => {
                        Answer::Refused(format!("git could not name the repository: {error:#}"))
                    }
                    Ok(Some(slug)) => match gh.settled_pull_requests(&slug) {
                        Ok(settled) => Answer::Answered(settled),
                        Err(refusal) => Answer::Refused(refusal),
                    },
                };
                let _ = sender.send((generation, repo_root, answer));
            });
        }
    }

    /// Throw every answer away so the next [`ask`](Self::ask) asks again.
    ///
    /// What `r` means here. A pull request merged while the picker was up is exactly the
    /// kind of thing a reload is for.
    ///
    /// Threads already running are left alone — there is no way to call one back — but their
    /// answers belong to the round this ends, and [`drain`](Self::drain) drops them on that
    /// basis. Not on whether the repository is still listed: it usually is, and `mpsc` hands
    /// over in whatever order the calls finish rather than the order they started, so a slow
    /// call from before the reload can land after a fast one from after it and win.
    pub fn forget(&mut self) {
        self.generation += 1;
        self.answers.clear();
    }

    /// Throw away the refusals, so the next [`ask`](Self::ask) asks those repositories again
    /// and leaves the rest alone.
    ///
    /// What entering a sweep means here. A `gh` that could not answer once — the network was
    /// out, the token had just expired — is not one that can never answer, and the only
    /// other way to ask again is `r`, which a sweep does not take. Everything else is kept:
    /// an answer, for the reason `ask` asks once; a repository with no GitHub remote, because
    /// asking again would be a `git remote get-url` per repository per `Shift-S` for an
    /// answer that cannot change; and a call still out, because dropping its slot would
    /// have the next `ask` start a second call for the same repository in the same round,
    /// with the older free to land last and win.
    ///
    /// No round ends here, unlike in [`forget`](Self::forget): a refusal is a call that has
    /// already come home, so there is no thread still out whose answer this has to disown.
    pub fn forget_failures(&mut self) {
        self.answers.retain(|_, answer| match answer {
            Answer::Refused(_) => false,
            Answer::Asking | Answer::Unaskable(_) | Answer::Answered(_) => true,
        });
    }

    /// Take in whatever has arrived, and say how many replies that was — the ones dropped
    /// included, which is what lets a test know that a reply it expects to be dropped has
    /// been, rather than not arrived yet.
    pub fn drain(&mut self) -> usize {
        let mut received = 0;
        while let Ok((generation, repo_root, answer)) = self.receiver.try_recv() {
            received += 1;
            if generation != self.generation {
                continue;
            }
            // Recorded against what was asked, rather than inserted. A reply with no slot
            // cannot happen today: a call still out keeps its slot through `ask`'s pruning
            // and through `forget_failures`, and the one thing that does drop it, `forget`,
            // ends the round, so the guard above has already dropped the reply. Kept as a
            // guard rather than an `insert` all the same, because inserting would put a
            // repository back that nothing is waiting on — and a mutation of it survives.
            if let Some(slot) = self.answers.get_mut(&repo_root) {
                *slot = answer;
            }
        }
        received
    }

    /// What `gh` has said so far, in the shape `domain::sweep` decides on.
    ///
    /// A repository still being asked about is left out entirely rather than entered as
    /// `None`, because those two are different questions there: absent is "nobody has asked
    /// yet", which says nothing on a row, and `None` is "asked, and `gh` could not answer",
    /// which does. A repository with no GitHub remote is `None` for the same reason: nothing
    /// has looked, and the row says so.
    ///
    /// Keyed from the tree rather than from the string this stored, because
    /// [`RepoRoot::of`] is the only way to make one and a `RepoNode` is the only thing it
    /// takes. `RepoNode` carries `repo_key` and `repo_root` side by side and a map keyed by
    /// the wrong one answers nothing for every checkout in the tree, silently — so the type
    /// keeps that choice in one place, and this walks the tree to stay inside it.
    pub fn answers(&self, tree: &Tree) -> BTreeMap<RepoRoot, Option<SettledPullRequests>> {
        tree.repos
            .iter()
            .filter_map(|repo| {
                let answer = match self.answers.get(&repo.repo_root)? {
                    Answer::Asking => return None,
                    Answer::Unaskable(_) | Answer::Refused(_) => None,
                    Answer::Answered(settled) => Some(settled.clone()),
                };
                Some((RepoRoot::of(repo), answer))
            })
            .collect()
    }

    /// Whether any answer is still coming, for a repository on the list. The prompt line
    /// turns a spinner while this is true, so a sweep entered on a slow network does not
    /// read as one that found nothing.
    ///
    /// Read off the map rather than counted. A count of calls started had to be kept level
    /// with the map by hand, and [`forget`](Self::forget) did not keep it: a round the user
    /// had just ended went on turning the spinner until every call from it came home, and
    /// asking again put the same repository in the count twice. The map cannot disagree
    /// with itself. And read from the tree, like [`answers`](Self::answers) and
    /// [`trouble`](Self::trouble): a call still out for a repository that has left the list
    /// is not one the user can see a spinner for.
    ///
    /// A call that never comes home keeps this true for as long as the picker is up and the
    /// repository is listed. That is what a budget on the call is for, and
    /// `adapter::gh_cli` does not yet apply one.
    pub fn is_waiting(&self, tree: &Tree) -> bool {
        tree.repos
            .iter()
            .any(|repo| matches!(self.answers.get(&repo.repo_root), Some(Answer::Asking)))
    }

    /// What went wrong, for the prompt line, or `None` when nothing did.
    ///
    /// One sentence however many repositories are in trouble, because this ends up on one
    /// line — with a count of the rest, so that one repository's trouble does not read as
    /// the whole of it. It names its repository, because the rows cannot be relied on to: a
    /// repository whose checkouts are all primary, running or `gone` never reaches
    /// `PR unknown`, so with two repositories in trouble for two reasons the sentence and
    /// the rows were about different ones.
    ///
    /// A refusal is named ahead of a missing remote. The second is a fact about the
    /// repository that the user can do nothing about and that the rows already say; the
    /// first is the one that a login or a network fixes — and with the local scratch
    /// repository sorted first, it was what the prompt line said for ever while the
    /// expired token on the repository that mattered was said nowhere.
    ///
    /// Within a kind, the repository named is the first in the tree — the order the screen
    /// lists them in, which is the same on every frame. What this replaced walked the map,
    /// which is path order; and the test that asserted "the first thing that went wrong"
    /// was racing its own fake, which handed each sentence to whichever thread reached it
    /// first. It failed six full runs in forty.
    ///
    /// Read from the tree for the reason [`answers`](Self::answers) is: a repository that
    /// has left the list leaves the prompt line with it.
    pub fn trouble(&self, tree: &Tree) -> Option<String> {
        let mut refused = None;
        let mut unaskable = None;
        let mut in_trouble = 0;
        for repo in &tree.repos {
            let (slot, why) = match self.answers.get(&repo.repo_root) {
                Some(Answer::Refused(why)) => (&mut refused, why),
                Some(Answer::Unaskable(why)) => (&mut unaskable, why),
                Some(Answer::Asking) | Some(Answer::Answered(_)) | None => continue,
            };
            in_trouble += 1;
            slot.get_or_insert_with(|| format!("{}: {why}", repo.display_name));
        }
        let first = refused.or(unaskable)?;
        Some(match in_trouble {
            1 => first,
            more => format!("{first} (+{} more)", more - 1),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use anyhow::Result;

    use super::*;
    use crate::app::fakes::until;
    use crate::domain::model::RepoNode;
    use crate::port::{PullRequest, Slug};

    /// A git that names one repository, and a `gh` that answers about it — both counting
    /// what they were asked, because asking twice is the thing this is built not to do.
    #[derive(Default)]
    struct Remote {
        /// `None` is a repository GitHub has never heard of.
        slug: Option<&'static str>,
        /// A git that will not answer at all, which is a third thing again.
        refuses: bool,
        /// What `gh` says, or the sentence it refuses with.
        answer: Option<Result<SettledPullRequests, String>>,
        asked: Mutex<Vec<String>>,
        /// How often git was asked to name a repository — a process each time.
        named: Mutex<usize>,
    }

    impl Remote {
        fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }

        fn named(&self) -> usize {
            *self.named.lock().unwrap()
        }
    }

    impl GitPort for Remote {
        fn github_slug(&self, _repo_root: &str) -> Result<Option<Slug>> {
            *self.named.lock().unwrap() += 1;
            if self.refuses {
                return Err(anyhow::anyhow!("fatal: not a git repository"));
            }
            Ok(self
                .slug
                .and_then(|slug| slug.split_once('/'))
                .and_then(|(owner, repo)| Slug::owner_repo(owner, repo)))
        }
        fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            unreachable!("only github_slug is asked of this port")
        }
        fn local_refs(&self, _repo_root: &str) -> Result<Vec<crate::port::GitRef>> {
            unreachable!("only github_slug is asked of this port")
        }
        fn remote_heads(&self, _repo_root: &str) -> Result<Vec<String>> {
            unreachable!("only github_slug is asked of this port")
        }
        fn fetch_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!("only github_slug is asked of this port")
        }
        fn fetch_all(&self, _repo_root: &str) -> Result<()> {
            unreachable!("only github_slug is asked of this port")
        }
        fn remove_worktree(&self, _repo_root: &str, _checkout_path: &str) -> Result<()> {
            unreachable!("only github_slug is asked of this port")
        }
        fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
            unreachable!("only github_slug is asked of this port")
        }
        fn head_ref(&self, _repo_root: &str) -> Result<String> {
            unreachable!("only github_slug is asked of this port")
        }
    }

    impl GhPort for Remote {
        fn pull_requests(&self, _slug: &Slug) -> Vec<PullRequest> {
            unreachable!("the sweep does not decorate")
        }
        fn settled_pull_requests(&self, slug: &Slug) -> Result<SettledPullRequests, String> {
            self.asked.lock().unwrap().push(slug.as_str().to_string());
            self.answer
                .clone()
                .unwrap_or_else(|| Ok(SettledPullRequests::All(Vec::new())))
        }
    }

    /// A `gh` that answers only when the test says so, and counts how many calls it has
    /// taken. The two rounds of a reload are otherwise a race nothing can pin.
    #[derive(Default)]
    struct Held {
        /// The slug each call was for, in the order the calls reached this. Which repository
        /// reaches it first is a race, so a test that cares which call is which asks by
        /// slug — `call_for` — rather than by number.
        started: Mutex<Vec<String>>,
        /// The answer each call is waiting for, by the order it started in. Addressed rather
        /// than queued, so the test decides which call finishes and in which order — which
        /// is the whole of what this is here to pin.
        released: Mutex<BTreeMap<usize, Result<SettledPullRequests, String>>>,
        /// Repositories git names no GitHub remote for.
        no_remote: Vec<&'static str>,
    }

    impl Held {
        fn started(&self) -> usize {
            self.started.lock().unwrap().len()
        }

        /// Which call, counting from one, was for this slug.
        fn call_for(&self, slug: &str) -> usize {
            self.started
                .lock()
                .unwrap()
                .iter()
                .position(|started| started == slug)
                .map(|index| index + 1)
                .unwrap_or_else(|| panic!("no call for {slug} has started"))
        }

        /// Let the `nth` call to start finish, with this answer.
        fn release(&self, nth: usize, answer: SettledPullRequests) {
            self.released.lock().unwrap().insert(nth, Ok(answer));
        }

        /// Let the `nth` call to start finish by refusing.
        fn release_err(&self, nth: usize, said: &str) {
            self.released
                .lock()
                .unwrap()
                .insert(nth, Err(said.to_string()));
        }
    }

    impl GitPort for Held {
        fn github_slug(&self, repo_root: &str) -> Result<Option<Slug>> {
            if self.no_remote.contains(&repo_root) {
                return Ok(None);
            }
            // Named from the path, the way the test tree's repositories are.
            let name = repo_root.rsplit('/').next().unwrap_or(repo_root);
            Ok(Slug::owner_repo("me", name))
        }
        fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            unreachable!("only github_slug is asked of this port")
        }
        fn local_refs(&self, _repo_root: &str) -> Result<Vec<crate::port::GitRef>> {
            unreachable!("only github_slug is asked of this port")
        }
        fn remote_heads(&self, _repo_root: &str) -> Result<Vec<String>> {
            unreachable!("only github_slug is asked of this port")
        }
        fn fetch_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!("only github_slug is asked of this port")
        }
        fn fetch_all(&self, _repo_root: &str) -> Result<()> {
            unreachable!("only github_slug is asked of this port")
        }
        fn remove_worktree(&self, _repo_root: &str, _checkout_path: &str) -> Result<()> {
            unreachable!("only github_slug is asked of this port")
        }
        fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
            unreachable!("only github_slug is asked of this port")
        }
        fn head_ref(&self, _repo_root: &str) -> Result<String> {
            unreachable!("only github_slug is asked of this port")
        }
    }

    impl GhPort for Held {
        fn pull_requests(&self, _slug: &Slug) -> Vec<PullRequest> {
            unreachable!("the sweep does not decorate")
        }
        fn settled_pull_requests(&self, slug: &Slug) -> Result<SettledPullRequests, String> {
            let nth = {
                let mut started = self.started.lock().unwrap();
                started.push(slug.as_str().to_string());
                started.len()
            };
            loop {
                if let Some(answer) = self.released.lock().unwrap().remove(&nth) {
                    return answer;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }

    fn settled_pr(number: u64, head_ref: &str) -> crate::port::SettledPullRequest {
        crate::port::SettledPullRequest {
            number,
            head_ref: head_ref.to_string(),
            from_a_fork: false,
            outcome: crate::port::PullRequestOutcome::Merged,
        }
    }

    fn repo(repo_root: &str) -> RepoNode {
        // Named from the path, so a sentence can say which repository it is about.
        let name = repo_root.rsplit('/').next().unwrap_or(repo_root);
        RepoNode {
            repo_key: format!("{repo_root}/.git"),
            repo_root: repo_root.to_string(),
            display_name: format!("me/{name}"),
            worktrees: Vec::new(),
        }
    }

    fn tree(roots: &[&str]) -> Tree {
        Tree {
            repos: roots.iter().map(|root| repo(root)).collect(),
            ungrouped: Vec::new(),
        }
    }

    fn until_answered(settled: &mut Settled, tree: &Tree) {
        until("gh never answered", || {
            settled.drain();
            !settled.is_waiting(tree)
        });
    }

    fn asking(remote: Remote) -> (Arc<Remote>, Settled) {
        let remote = Arc::new(remote);
        let settled = Settled::new(remote.clone(), remote.clone());
        (remote, settled)
    }

    #[test]
    fn each_repository_is_asked_about_once_however_often_the_sweep_is_entered() {
        // The whole reason this is owned by the view switch. A merged pull request does not
        // become unmerged, so entering the sweep a second time is a frame — not another
        // round of `gh` over the network on a key the user is holding down.
        let (remote, mut settled) = asking(Remote {
            slug: Some("me/app"),
            ..Remote::default()
        });
        let tree = tree(&["/src/app"]);

        settled.ask(&tree);
        until_answered(&mut settled, &tree);
        settled.ask(&tree);
        settled.ask(&tree);
        settled.drain();

        assert_eq!(remote.asked(), ["me/app"]);
    }

    #[test]
    fn a_reload_asks_again_because_a_pull_request_can_land_while_the_picker_is_up() {
        let (remote, mut settled) = asking(Remote {
            slug: Some("me/app"),
            ..Remote::default()
        });
        let tree = tree(&["/src/app"]);

        settled.ask(&tree);
        until_answered(&mut settled, &tree);
        settled.forget();
        settled.ask(&tree);
        until_answered(&mut settled, &tree);

        assert_eq!(remote.asked(), ["me/app", "me/app"]);
    }

    #[test]
    fn an_answer_from_before_a_reload_does_not_overwrite_the_one_after_it() {
        // `mpsc` hands answers over in whatever order the calls finish, not the order they
        // started, so a slow `gh` from before `r` can land after a fast one from after it.
        // The whole point of `r` is that a pull request may have landed since, so the older
        // answer winning is a sweep showing the state of the world the user just asked it to
        // stop showing.
        let held = Arc::new(Held::default());
        let mut settled = Settled::new(held.clone(), held.clone());
        let tree = tree(&["/src/app"]);

        settled.ask(&tree);
        until("the first call never started", || held.started() == 1);

        // `r`. The call already running belongs to the round this ends.
        settled.forget();
        settled.ask(&tree);
        until("the second call never started", || held.started() == 2);

        // The reload's call comes back first, and the one from before it comes back after.
        held.release(
            2,
            SettledPullRequests::All(vec![settled_pr(2, "feat/after")]),
        );
        until("the reload's answer never landed", || {
            settled.drain();
            settled.answers(&tree).len() == 1
        });
        held.release(
            1,
            SettledPullRequests::All(vec![settled_pr(1, "feat/before")]),
        );
        // Waited for, not assumed: with the stale reply still in the channel the assertion
        // below holds whether or not `drain` drops it, and a `drain` that did not was green
        // one run in six.
        until("the stale answer never arrived", || settled.drain() > 0);

        let listed = settled
            .answers(&tree)
            .into_values()
            .next()
            .flatten()
            .expect("the repository answered");
        assert_eq!(
            listed.pull_requests()[0].head_ref,
            "feat/after",
            "the round the user asked for is the one on screen"
        );
    }

    #[test]
    fn a_repository_github_has_never_heard_of_is_asked_and_answered_for() {
        // Not absent, which would read as "still coming" and put a spinner on a row that
        // will never fill in. `gh` is never started for it — there is nothing to start it
        // with — but the question was put and this is the answer.
        let (remote, mut settled) = asking(Remote::default());
        let tree = tree(&["/src/app"]);

        settled.ask(&tree);
        until_answered(&mut settled, &tree);

        assert!(
            remote.asked().is_empty(),
            "gh has nothing to be asked about"
        );
        let answers = settled.answers(&tree);
        assert_eq!(answers.len(), 1, "the repository is in the answer");
        assert!(
            answers.values().all(Option::is_none),
            "and its answer is that there is not one"
        );
        assert_eq!(
            settled.trouble(&tree),
            Some("me/app: no GitHub remote to ask about".to_string())
        );
    }

    #[test]
    fn a_git_that_would_not_name_the_repository_says_so_rather_than_nothing() {
        // The other half of "asked and unanswerable". Reporting it as an empty answer would
        // be the conflation ADR 0011 exists to prevent — a sweep saying nothing is finished
        // with, on a repository it never managed to ask about.
        let (remote, mut settled) = asking(Remote {
            refuses: true,
            ..Remote::default()
        });
        let tree = tree(&["/src/app"]);

        settled.ask(&tree);
        until_answered(&mut settled, &tree);

        assert!(
            remote.asked().is_empty(),
            "there was nothing to ask gh about"
        );
        assert_eq!(settled.answers(&tree).len(), 1, "but it was asked");
        assert!(settled.answers(&tree).values().all(Option::is_none));
        let said = settled
            .trouble(&tree)
            .expect("a sentence for the prompt line");
        assert!(
            said.contains("git could not name the repository"),
            "and it says which half went wrong: {said}"
        );
    }

    #[test]
    fn the_first_repository_on_screen_that_failed_is_the_one_named() {
        // One line, however many repositories failed — but it names its repository, because
        // the rows cannot be relied on to: a repository whose checkouts are all primary,
        // running or `gone` never reaches `PR unknown`, so with two failing for two reasons
        // the sentence and the rows were about different ones. And "first" is the order the
        // screen lists them in, which is the same on every frame. The test this replaces
        // asserted "the first thing that went wrong" against a fake that handed each
        // sentence to whichever thread reached it first, and it failed six full runs in
        // forty.
        let (_, mut settled) = asking(Remote {
            slug: Some("me/app"),
            answer: Some(Err("gh is out".to_string())),
            ..Remote::default()
        });
        // Listed the other way round from how their paths sort, so that path order — which
        // is what walking the map gives — is a different answer from screen order.
        let tree = tree(&["/src/site", "/src/app"]);

        settled.ask(&tree);
        until_answered(&mut settled, &tree);

        assert_eq!(
            settled.trouble(&tree),
            Some("me/site: gh is out (+1 more)".to_string())
        );
    }

    #[test]
    fn a_repository_that_left_the_tree_leaves_the_prompt_line_with_it() {
        // `answers` is read off the tree, and so is this. Kept in the map and read from
        // there, an error from a repository no longer listed put a sentence on the prompt
        // line with every row on screen answered and none of them saying `PR unknown`.
        let (_, mut settled) = asking(Remote {
            slug: Some("me/app"),
            answer: Some(Err("gh is out".to_string())),
            ..Remote::default()
        });
        let listed = tree(&["/src/app"]);
        let gone = tree(&[]);

        settled.ask(&listed);
        until_answered(&mut settled, &listed);
        assert!(settled.trouble(&listed).is_some());

        assert_eq!(settled.trouble(&gone), None);
    }

    #[test]
    fn entering_a_sweep_again_asks_again_where_gh_refused_and_nowhere_else() {
        // A `gh` that could not answer once — the network was out, the token had just
        // expired — is not one that can never answer, and `r`, the only other way to ask
        // again, is not taken during a sweep. Only the refusals are asked again: an answer
        // that came is kept, for the reason `ask` asks once.
        let held = Arc::new(Held::default());
        let mut settled = Settled::new(held.clone(), held.clone());
        let tree = tree(&["/src/app", "/src/site"]);

        settled.ask(&tree);
        until("both calls never started", || held.started() == 2);
        held.release_err(1, "gh is out");
        held.release(2, SettledPullRequests::All(Vec::new()));
        until_answered(&mut settled, &tree);
        assert!(settled.trouble(&tree).is_some());

        settled.forget_failures();
        settled.ask(&tree);
        until("the refused one was not asked again", || {
            held.started() == 3
        });
        held.release(3, SettledPullRequests::All(Vec::new()));
        until_answered(&mut settled, &tree);

        assert_eq!(settled.trouble(&tree), None);
        assert_eq!(
            held.started(),
            3,
            "the one that answered was not asked twice"
        );
    }

    #[test]
    fn a_reload_stops_waiting_for_the_round_it_ended() {
        // Whether anything is still coming is read off the map, not counted. A count of
        // calls started had to be kept level with the map by hand, and `forget` did not
        // keep it: the round the user had just ended went on turning the spinner until
        // every call from it came home, and asking again put the same repository in the
        // count twice — so the new round's answer landing was not enough to stop it.
        let held = Arc::new(Held::default());
        let mut settled = Settled::new(held.clone(), held.clone());
        let tree = tree(&["/src/app"]);

        settled.ask(&tree);
        until("the call never started", || held.started() == 1);
        assert!(settled.is_waiting(&tree));

        settled.forget();
        assert!(
            !settled.is_waiting(&tree),
            "a round the user ended is not one the spinner turns for"
        );

        settled.ask(&tree);
        until("the second call never started", || held.started() == 2);
        assert!(settled.is_waiting(&tree), "the new round is");
        // Only the new round's call comes home. Which round wins when both do is
        // `an_answer_from_before_a_reload_does_not_overwrite_the_one_after_it`'s to say,
        // and it says so by waiting for the stale reply; released together here, the two
        // raced, and this was green without the guard one run in six.
        held.release(2, SettledPullRequests::All(vec![settled_pr(2, "fresh")]));
        until_answered(&mut settled, &tree);

        let listed = settled
            .answers(&tree)
            .into_values()
            .next()
            .flatten()
            .expect("the repository answered");
        assert_eq!(listed.pull_requests()[0].head_ref, "fresh");
        held.release(1, SettledPullRequests::All(Vec::new()));
    }

    #[test]
    fn a_sweep_is_still_waiting_while_any_listed_repository_is() {
        // Two repositories, one slow. Read as "all still out" instead of "any", the spinner
        // stopped when the fast one landed, the prompt read as a finished sweep, and the
        // slow one's answer then widened it under the cursor — the very thing the spinner
        // is there to say is coming.
        let held = Arc::new(Held::default());
        let mut settled = Settled::new(held.clone(), held.clone());
        let tree = tree(&["/src/app", "/src/site"]);

        settled.ask(&tree);
        until("both calls never started", || held.started() == 2);
        held.release(1, SettledPullRequests::All(Vec::new()));
        until("the first answer never landed", || {
            settled.drain();
            settled.answers(&tree).len() == 1
        });

        assert!(settled.is_waiting(&tree), "one answered, one is still out");
        held.release(2, SettledPullRequests::All(Vec::new()));
        until_answered(&mut settled, &tree);
    }

    #[test]
    fn nothing_is_wrong_while_an_answer_is_still_coming() {
        let held = Arc::new(Held::default());
        let mut settled = Settled::new(held.clone(), held.clone());
        let tree = tree(&["/src/app"]);

        settled.ask(&tree);
        until("the call never started", || held.started() == 1);

        assert!(settled.is_waiting(&tree));
        assert_eq!(
            settled.trouble(&tree),
            None,
            "still asking is not a failure"
        );
        held.release(1, SettledPullRequests::All(Vec::new()));
        until_answered(&mut settled, &tree);
    }

    #[test]
    fn a_repository_that_answered_does_not_hide_the_one_after_it_that_could_not() {
        // The test above this one has both repositories fail, so it pins the order and not
        // the scan: "the first repository that failed" and "the first repository, if it
        // failed" gave the same answer there. Here the first answered.
        let held = Arc::new(Held::default());
        let mut settled = Settled::new(held.clone(), held.clone());
        let tree = tree(&["/src/app", "/src/site"]);

        settled.ask(&tree);
        until("both calls never started", || held.started() == 2);
        held.release(
            held.call_for("me/app"),
            SettledPullRequests::All(Vec::new()),
        );
        held.release_err(held.call_for("me/site"), "gh is out");
        until_answered(&mut settled, &tree);

        assert_eq!(
            settled.trouble(&tree),
            Some("me/site: gh is out".to_string())
        );
    }

    #[test]
    fn a_refusal_is_named_ahead_of_a_repository_with_nothing_to_ask() {
        // A scratch repository with no remote sorted first, and the prompt line said so for
        // ever — while the expired token on the repository that mattered, listed second,
        // was said nowhere. Named ahead of it now, with the rest counted.
        let held = Arc::new(Held {
            no_remote: vec!["/src/local"],
            ..Held::default()
        });
        let mut settled = Settled::new(held.clone(), held.clone());
        let tree = tree(&["/src/local", "/src/site"]);

        settled.ask(&tree);
        until("the site's call never started", || held.started() == 1);
        until("the local repository never answered", || {
            settled.drain();
            settled.answers(&tree).len() == 1
        });
        assert_eq!(
            settled.trouble(&tree),
            Some("me/local: no GitHub remote to ask about".to_string()),
            "the only trouble so far, while the other is still asking"
        );

        held.release_err(1, "gh refused the question this asked: no auth");
        until_answered(&mut settled, &tree);

        assert_eq!(
            settled.trouble(&tree),
            Some("me/site: gh refused the question this asked: no auth (+1 more)".to_string())
        );
    }

    #[test]
    fn a_repository_with_no_github_remote_is_not_asked_again_on_the_way_back_in() {
        // Nothing about it can change while the picker is up, and asking is a
        // `git remote get-url` — a process per repository per `Shift-S`, for ever.
        let (remote, mut settled) = asking(Remote::default());
        let tree = tree(&["/src/app"]);

        settled.ask(&tree);
        until_answered(&mut settled, &tree);
        assert_eq!(remote.named(), 1);

        settled.forget_failures();
        settled.ask(&tree);
        settled.drain();

        assert_eq!(remote.named(), 1, "git was not asked to name it again");
        assert_eq!(
            settled.trouble(&tree),
            Some("me/app: no GitHub remote to ask about".to_string()),
            "and the answer is still the answer"
        );
    }

    #[test]
    fn entering_a_sweep_again_leaves_a_question_still_out_alone() {
        // `forget_failures` throws away refusals and nothing else. Throw away the slot of a
        // call still out and the next `ask` starts a second call for the same repository in
        // the same round, with the older free to land last and win; end the round instead
        // and the call still out is disowned when it lands, so the spinner never stops.
        let held = Arc::new(Held::default());
        let mut settled = Settled::new(held.clone(), held.clone());
        let tree = tree(&["/src/app", "/src/site"]);

        settled.ask(&tree);
        until("both calls never started", || held.started() == 2);
        held.release_err(held.call_for("me/app"), "gh is out");
        until("the refusal never landed", || {
            settled.drain();
            settled.trouble(&tree).is_some()
        });

        settled.forget_failures();
        settled.ask(&tree);
        until("the refused one was not asked again", || {
            held.started() == 3
        });
        held.release(3, SettledPullRequests::All(Vec::new()));
        held.release(
            held.call_for("me/site"),
            SettledPullRequests::All(Vec::new()),
        );
        until_answered(&mut settled, &tree);

        assert_eq!(
            held.started(),
            3,
            "the call still out was not started twice"
        );
        assert_eq!(
            settled.answers(&tree).len(),
            2,
            "and its answer landed in the round it was asked in"
        );
    }

    #[test]
    fn a_repository_that_left_the_list_and_came_back_is_asked_again() {
        // What was learned about it went when it left. Kept, a refusal from before it left
        // was the sweep's answer about it for the life of the picker, with no `r` inside a
        // sweep to clear it and `forget_failures` firing only on the way in.
        let (remote, mut settled) = asking(Remote {
            slug: Some("me/app"),
            ..Remote::default()
        });
        let listed = tree(&["/src/app"]);
        let gone = tree(&[]);

        settled.ask(&listed);
        until_answered(&mut settled, &listed);
        settled.ask(&gone);
        settled.ask(&listed);
        until_answered(&mut settled, &listed);

        assert_eq!(remote.asked(), ["me/app", "me/app"]);
    }

    #[test]
    fn a_call_still_out_keeps_its_slot_when_its_repository_leaves_the_list() {
        // `ask` lets go of what was learned about a repository that has left — but not of a
        // call that has not come home, or its answer would have nowhere to land and the
        // repository coming back would start a second call in the same round.
        let held = Arc::new(Held::default());
        let mut settled = Settled::new(held.clone(), held.clone());
        let listed = tree(&["/src/app"]);
        let gone = tree(&[]);

        settled.ask(&listed);
        until("the call never started", || held.started() == 1);
        settled.ask(&gone);
        held.release(1, SettledPullRequests::All(Vec::new()));
        until("the answer never arrived", || settled.drain() > 0);

        assert_eq!(
            settled.answers(&listed).len(),
            1,
            "its answer had somewhere to land"
        );
    }

    #[test]
    fn the_spinner_turns_only_for_a_repository_on_the_list() {
        // `answers` and `trouble` read from the tree; this did not, and a call still out for
        // a repository the user could no longer see kept `asking gh…` turning with every
        // visible row answered.
        let held = Arc::new(Held::default());
        let mut settled = Settled::new(held.clone(), held.clone());
        let listed = tree(&["/src/app"]);
        let gone = tree(&[]);

        settled.ask(&listed);
        until("the call never started", || held.started() == 1);

        assert!(settled.is_waiting(&listed));
        assert!(!settled.is_waiting(&gone));
        held.release(1, SettledPullRequests::All(Vec::new()));
        until_answered(&mut settled, &listed);
    }

    #[test]
    fn a_repository_still_being_asked_about_is_not_in_the_answer_at_all() {
        // Absent and `None` are different questions to `domain::sweep`: absent is "nobody
        // has asked yet" and says nothing on a row, `None` is "asked and gh could not say"
        // and puts `PR unknown` on one. Reporting the first as the second would tell the
        // user a sweep failed while it was still running.
        let (_, mut settled) = asking(Remote {
            slug: Some("me/app"),
            ..Remote::default()
        });
        let tree = tree(&["/src/app"]);

        settled.ask(&tree);
        assert!(
            settled.answers(&tree).is_empty(),
            "nothing has come back yet"
        );
        assert!(settled.is_waiting(&tree), "and the prompt line says so");

        until_answered(&mut settled, &tree);
        assert_eq!(settled.answers(&tree).len(), 1);
        assert!(!settled.is_waiting(&tree));
    }

    #[test]
    fn a_gh_that_refused_says_so_once_and_leaves_the_rows_to_say_the_rest() {
        let (_, mut settled) = asking(Remote {
            slug: Some("me/app"),
            answer: Some(Err(
                "gh refused the question this asked: no auth".to_string()
            )),
            ..Remote::default()
        });
        let tree = tree(&["/src/app", "/src/lib"]);

        settled.ask(&tree);
        until_answered(&mut settled, &tree);

        assert_eq!(
            settled.trouble(&tree),
            Some("me/app: gh refused the question this asked: no auth (+1 more)".to_string()),
            "one sentence however many repositories failed — it goes on one line, and counts"
        );
        let answers = settled.answers(&tree);
        assert_eq!(answers.len(), 2, "both were asked");
        assert!(
            answers.values().all(Option::is_none),
            "and neither could answer, which is what the rows say"
        );
    }

    #[test]
    fn the_answer_is_keyed_by_the_root_and_not_the_directory_beside_it() {
        // `RepoNode` carries `/src/app/.git` and `/src/app` side by side. `domain::sweep`
        // looks its facts up by the second, so keying on the first answers nothing for
        // every checkout in the tree — no marks, no `PR unknown`, no error, nothing.
        let (_, mut settled) = asking(Remote {
            slug: Some("me/app"),
            ..Remote::default()
        });
        let tree = tree(&["/src/app"]);

        settled.ask(&tree);
        until_answered(&mut settled, &tree);

        assert_eq!(
            settled.answers(&tree).into_keys().collect::<Vec<_>>(),
            vec![RepoRoot::of(&repo("/src/app"))]
        );
    }

    #[test]
    fn nothing_went_wrong_is_not_a_sentence() {
        let (_, mut settled) = asking(Remote {
            slug: Some("me/app"),
            ..Remote::default()
        });
        let tree = tree(&["/src/app"]);

        settled.ask(&tree);
        until_answered(&mut settled, &tree);

        assert_eq!(settled.trouble(&tree), None);
        assert_eq!(
            settled.answers(&tree).into_values().next(),
            Some(Some(SettledPullRequests::All(Vec::new()))),
            "an empty answer is an answer: nothing here is finished with"
        );
    }
}
