//! What `gh` says has become of each repository's pull requests.
//!
//! Asked when a sweep is entered rather than when the picker opens, because it is the
//! heavier of the two `gh` calls — a window over everything that has landed rather than a
//! glance at what is in flight — and because most sessions never sweep. That is the sentence
//! in `docs/adr/0011-what-may-be-swept.md` this carries out.
//!
//! One thread per repository, not per checkout. Repositories are however many the user has
//! panes open in, which is a handful, so there is nothing here to cap — unlike
//! [`app::dirty`](crate::app::dirty), where every checkout costs a process of its own.
//!
//! A repository that has not answered yet is absent from the map, which is not the same as
//! present with `None`: [`domain::sweep::Facts`](crate::domain::sweep::Facts) reads the first
//! as "nobody has asked" and the second as "asked, and `gh` could not say", and only the
//! second is worth a word on a row.

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
/// testing for the one it wants — the shape
/// [`domain::sweep::Candidate::is_markable`](crate::domain::sweep::Candidate::is_markable)'s
/// doc argues for. A `gh` that refused is worth asking again and a repository with no GitHub
/// remote is not, so one state for both could not tell them apart.
#[derive(Debug, Clone)]
enum Answer {
    /// Asked, and the call has not come home.
    Asking,
    /// There is nothing to ask: git named no GitHub remote. That does not change while the
    /// picker is up, so it is not asked again on entering a sweep. The rows still say
    /// `PR unknown`, as ADR 0011 asks: nothing has looked.
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
    /// the whole point of `r` is that one may have landed since. Why a round and not arrival
    /// order: `docs/adr/0016-an-answer-belongs-to-a-round.md`.
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
    /// and re-entered — is a map lookup rather than another round of `gh`. A call still out
    /// keeps its slot whatever the tree says, so that its answer has somewhere to land.
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
                    Ok(None) => Answer::Unaskable("no GitHub remote to ask about".to_string()),
                    // One line, because this one reaches the prompt line: git says its
                    // piece over several — an unreadable `.git/config` is a `warning:` and a
                    // `fatal:` — and a newline inside a `Span` breaks the row it is drawn
                    // in. The `gh` half is single-lined in its own adapter; this half had
                    // nothing to single-line until git's refusals could reach here at all.
                    Err(error) => Answer::Refused(crate::app::one_line(&format!(
                        "git could not name the repository: {error:#}"
                    ))),
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
    /// What `r` means here: a pull request merged while the picker was up is what a reload
    /// is for.
    ///
    /// What becomes of the calls already out is ADR 0016.
    pub fn forget(&mut self) {
        self.generation += 1;
        self.answers.clear();
    }

    /// Throw away the refusals, so the next [`ask`](Self::ask) asks those repositories again
    /// and leaves the rest alone.
    ///
    /// What entering a sweep means here. A `gh` that could not answer once — the network was
    /// out, the token had just expired — is not one that can never answer, and the only
    /// other way to ask again is `r`, which a sweep does not take. A repository with no
    /// GitHub remote is kept, because asking again would be a `git remote get-url` per
    /// repository per `Shift-S` for an answer that cannot change; a call still out is kept,
    /// because dropping its slot would have the next `ask` start a second call for the same
    /// repository in the same round, with the older free to land last and win.
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
            // Recorded against what was asked, rather than inserted: a reply with no slot
            // cannot happen today, but inserting would put a repository back that nothing is
            // waiting on.
            if let Some(slot) = self.answers.get_mut(&repo_root) {
                *slot = answer;
            }
        }
        received
    }

    /// What `gh` has said so far, in the shape [`domain::sweep`](crate::domain::sweep) decides
    /// on.
    ///
    /// A repository still being asked about is left out entirely rather than entered as
    /// `None`: absent is "nobody has asked yet", and `None` is "asked, and `gh` could not
    /// answer", which is what puts `PR unknown` on a row. A repository with no GitHub remote
    /// is `None` for the same reason: nothing has looked, and the row says so.
    ///
    /// Keyed from the tree rather than from the string this stored, because
    /// [`RepoRoot::of`] is the only way to make one and a `RepoNode` is the only thing it
    /// takes. `RepoNode` carries `repo_key` and `repo_root` side by side, and a map keyed by
    /// the wrong one answers nothing for every checkout in the tree, silently.
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
    /// Read off the map rather than from a count of calls started, which would have to be
    /// kept level with the map by hand across [`forget`](Self::forget) and an `ask` that
    /// asks the same repository again. And read from the tree, like
    /// [`answers`](Self::answers) and [`trouble`](Self::trouble): a call still out for a
    /// repository that has left the list is not one the user can see a spinner for.
    ///
    /// A call that never comes home would keep this true for as long as the picker is up and
    /// the repository is listed; [`adapter::gh_cli`](crate::adapter::gh_cli) gives up on one
    /// after `GH_BUDGET` and answers with a refusal instead —
    /// `a_gh_on_the_path_is_given_the_budget_and_no_more` in `tests/gh_cli.rs`.
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
    /// `PR unknown`.
    ///
    /// A refusal is named ahead of a missing remote. The second is a fact about the
    /// repository that the user can do nothing about and that the rows already say; the
    /// first is the one that a login or a network fixes.
    ///
    /// Within a kind, the repository named is the first in the tree — the order the screen
    /// lists them in, which is the same on every frame; walking the map gives path order
    /// instead. Read from the tree for the reason [`answers`](Self::answers) is: a
    /// repository that has left the list leaves the prompt line with it.
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
mod tests;
