//! What the picker remembers about a repository between visits to the branches view.
//!
//! `Tab` leaves the branches view and comes back to it, and the two answers worth keeping
//! across that are the ones that cost a network round trip. Local refs are deliberately not
//! here: `git for-each-ref` is a few milliseconds, and it is the half that changes while the
//! picker is up, so it is read again on every visit.

use std::collections::HashMap;

use crate::port::PullRequest;

/// What the picker remembers about one repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Remote {
    pub heads: Vec<String>,
    pub pull_requests: Vec<PullRequest>,
    /// The remote listing is still in flight, so `heads` may still grow.
    pub loading: bool,
}

/// What a background thread came back with about one repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// `git ls-remote` succeeded.
    Heads(Vec<String>),
    /// It did not — offline, no `origin`, or no credentials. Either way the wait is over and
    /// the picker carries on with what git already had.
    Unavailable,
    PullRequests(Vec<PullRequest>),
    /// A `git fetch` rewrote the repository's refs.
    Refetched,
}

/// The cache the picker carries across a view switch, keyed by repository root.
pub type Cache = HashMap<String, Remote>;

/// Note that a repository is being read, so that a frame drawn before the answer lands can
/// say so.
pub fn starting(cache: &mut Cache, repo_root: &str) -> Remote {
    let entry = cache.entry(repo_root.to_string()).or_default();
    entry.loading = true;
    entry.clone()
}

/// Fold one answer into the cache and hand back what the repository now looks like.
///
/// `None` means there is nothing left to show from cache: a fetch rewrote the refs, so what
/// was remembered is behind and the repository has to be read again.
pub fn apply(cache: &mut Cache, repo_root: &str, answer: Answer) -> Option<Remote> {
    // Not a field on the entry: `--prune` deleted refs, and anything kept here would put
    // them straight back. The whole entry goes, and the repository is read again.
    if answer == Answer::Refetched {
        cache.remove(repo_root);
        return None;
    }

    let entry = cache.entry(repo_root.to_string()).or_default();
    match answer {
        Answer::Heads(heads) => {
            entry.heads = heads;
            entry.loading = false;
        }
        // Not `heads.clear()`: an unreachable remote is not an empty remote, and a previous
        // visit may well have reached it.
        Answer::Unavailable => entry.loading = false,
        Answer::PullRequests(pull_requests) => entry.pull_requests = pull_requests,
        Answer::Refetched => unreachable!("returned above"),
    }
    Some(entry.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull_request(number: u64, head_ref: &str) -> PullRequest {
        PullRequest {
            number,
            title: format!("pr {number}"),
            head_ref: head_ref.into(),
            is_draft: false,
        }
    }

    fn heads(names: &[&str]) -> Answer {
        Answer::Heads(names.iter().map(|name| (*name).to_string()).collect())
    }

    #[test]
    fn a_listing_fills_in_the_heads_and_ends_the_wait() {
        let mut cache = Cache::new();
        starting(&mut cache, "/src/app");

        let now = apply(&mut cache, "/src/app", heads(&["main", "topic"])).expect("still cached");

        assert_eq!(now.heads, vec!["main".to_string(), "topic".to_string()]);
        assert!(!now.loading, "the answer arrived, so nothing is waited for");
        assert_eq!(
            cache["/src/app"], now,
            "what is handed back is what is kept"
        );
    }

    #[test]
    fn a_listing_that_failed_ends_the_wait_and_leaves_what_was_there() {
        let mut cache = Cache::new();
        starting(&mut cache, "/src/app");
        apply(&mut cache, "/src/app", heads(&["main"]));
        starting(&mut cache, "/src/app");

        let now = apply(&mut cache, "/src/app", Answer::Unavailable).expect("still cached");

        assert_eq!(
            now.heads,
            vec!["main".to_string()],
            "an unreachable remote is not an empty remote"
        );
        assert!(!now.loading);
    }

    #[test]
    fn pull_requests_arrive_without_disturbing_the_heads() {
        let mut cache = Cache::new();
        starting(&mut cache, "/src/app");
        apply(&mut cache, "/src/app", heads(&["main"]));

        let now = apply(
            &mut cache,
            "/src/app",
            Answer::PullRequests(vec![pull_request(7, "topic")]),
        )
        .expect("still cached");

        assert_eq!(now.heads, vec!["main".to_string()]);
        assert_eq!(now.pull_requests, vec![pull_request(7, "topic")]);
    }

    /// The listing threads outlive the frame that asked for them, so an answer can be the
    /// first thing the cache hears about a repository.
    #[test]
    fn an_answer_for_a_repository_never_seen_before_creates_its_entry() {
        let mut cache = Cache::new();

        let now = apply(&mut cache, "/src/app", heads(&["main"])).expect("still cached");

        assert_eq!(now.heads, vec!["main".to_string()]);
        assert!(!now.loading);
    }

    /// A fetch rewrote `refs/remotes`, and `--prune` deleted some. Patching what is cached
    /// would put the pruned ones straight back, so what is cached goes instead.
    #[test]
    fn a_fetch_drops_what_was_remembered_rather_than_patching_it() {
        let mut cache = Cache::new();
        starting(&mut cache, "/src/app");
        apply(&mut cache, "/src/app", heads(&["main", "gone"]));

        let now = apply(&mut cache, "/src/app", Answer::Refetched);

        assert_eq!(now, None, "there is nothing left to show from cache");
        assert!(
            !cache.contains_key("/src/app"),
            "the next visit has to read the repository again"
        );
    }

    #[test]
    fn a_fetch_leaves_every_other_repository_alone() {
        let mut cache = Cache::new();
        apply(&mut cache, "/src/app", heads(&["main"]));
        apply(&mut cache, "/src/other", heads(&["develop"]));

        apply(&mut cache, "/src/app", Answer::Refetched);

        assert_eq!(cache["/src/other"].heads, vec!["develop".to_string()]);
    }

    #[test]
    fn two_repositories_are_remembered_separately() {
        let mut cache = Cache::new();

        apply(&mut cache, "/src/app", heads(&["main"]));
        apply(&mut cache, "/src/other", heads(&["develop"]));

        assert_eq!(cache["/src/app"].heads, vec!["main".to_string()]);
        assert_eq!(cache["/src/other"].heads, vec!["develop".to_string()]);
    }
}
