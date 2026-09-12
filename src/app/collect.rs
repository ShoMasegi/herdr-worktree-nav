//! Gathering the inputs `domain::tree::build` needs.
//!
//! This is the impure half of building the panes view: it asks herdr for the session, works
//! out which repository and checkout every pane is in, and asks herdr for each repository's
//! worktrees. The decisions all live in `domain`; this module only fetches.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;

use crate::app::one_line;
use crate::domain::model::{normalize_path, Tree};
use crate::domain::tree::{self, PanePlacement, RepoInput};
use crate::port::{GitPort, HerdrPort, Snapshot};

/// Fetch everything and build the tree.
pub fn collect_tree(herdr: &dyn HerdrPort, git: &dyn GitPort) -> Result<(Snapshot, Tree)> {
    let snapshot = herdr.snapshot()?;
    let placements = resolve_placements(&snapshot, git);
    let mut repos = collect_repos(herdr, git, &placements);
    read_refs(git, &mut repos);
    let tree = tree::build(&snapshot, &repos, &placements);
    Ok((snapshot, tree))
}

/// Read every repository's refs, all at once.
///
/// One `for-each-ref` per repository, which is where ahead/behind and `gone` come from. They
/// ride on the format string of a call that has to be made anyway rather than on a
/// `rev-list --count` per branch, which is what makes one process per repository the whole
/// cost of them.
///
/// In front of the first frame on purpose: unlike whether a checkout is dirty, these are
/// known the moment git answers, so there is nothing to be gained by drawing the list
/// without them first.
///
/// A repository git could not answer for carries no markers and says so: git's words go on
/// the repository, and the prompt line names it once — `domain::rows::refs_trouble`.
fn read_refs(git: &dyn GitPort, repos: &mut [RepoInput]) {
    // No chunking: repositories are however many the user has panes open in, which is a
    // handful — unlike working directories, where every pane can have its own.
    std::thread::scope(|scope| {
        let handles: Vec<_> = repos
            .iter()
            .map(|repo| {
                let repo_root = repo.repo_root.clone();
                scope.spawn(move || {
                    // Two ways this repository ends up with no markers, and the rows cannot
                    // tell them apart from having nothing to report either way: git refused
                    // the call, or git answered without a ref it could not read. The second
                    // is the whole answer for a checkout whose ref that was, so it is a
                    // refusal here as well — `port::RefWalk` says why the branches view
                    // makes the other choice.
                    match git.local_refs(&repo_root) {
                        Err(error) => Err(one_line(&format!("{error:#}"))),
                        Ok(walk) => match walk.dropped {
                            Some(words) => Err(one_line(&words)),
                            None => Ok(walk.refs),
                        },
                    }
                })
            })
            .collect();
        for (repo, handle) in repos.iter_mut().zip(handles) {
            // `join` fails for one reason: the thread panicked. In the shipped binary that
            // is unreachable — `panic = "abort"` in the release profile ends the process
            // before this line — so what is chosen here only applies to a debug build,
            // where ratatui's hook has already restored the terminal and the picker would
            // carry on drawing onto it either way. That repository's markers are missing,
            // and it says so like any other repository whose refs were not read.
            repo.refs = handle
                .join()
                .unwrap_or_else(|_| Err("the thread reading them did not finish".to_string()));
        }
    });
}

/// Work out which repository and checkout each pane sits in.
///
/// Two shortcuts keep this cheap. Panes are resolved per working directory rather than per
/// pane, because several panes usually share one; and when herdr already knows a workspace
/// is a worktree, its answer is reused instead of running git — but only for panes that are
/// still somewhere under that checkout, since a pane is free to `cd` into another repository.
fn resolve_placements(snapshot: &Snapshot, git: &dyn GitPort) -> HashMap<String, PanePlacement> {
    let workspace_worktrees: HashMap<&str, PanePlacement> = snapshot
        .workspaces
        .iter()
        .filter_map(|workspace| {
            workspace.worktree.as_ref().map(|worktree| {
                (
                    workspace.workspace_id.as_str(),
                    PanePlacement {
                        repo_key: normalize_path(&worktree.repo_key).to_string(),
                        checkout_path: normalize_path(&worktree.checkout_path).to_string(),
                    },
                )
            })
        })
        .collect();

    let mut placements = HashMap::new();
    let mut unresolved: HashSet<&str> = HashSet::new();

    for pane in &snapshot.panes {
        let Some(cwd) = pane.effective_cwd() else {
            continue;
        };
        match workspace_worktrees.get(pane.workspace_id.as_str()) {
            Some(known) if is_inside(cwd, &known.checkout_path) => {
                placements.insert(pane.pane_id.clone(), known.clone());
            }
            _ => {
                unresolved.insert(cwd);
            }
        }
    }

    let resolved = identify_all(git, &unresolved);

    for pane in &snapshot.panes {
        if placements.contains_key(&pane.pane_id) {
            continue;
        }
        let Some(cwd) = pane.effective_cwd() else {
            continue;
        };
        if let Some(Some(placement)) = resolved.get(cwd) {
            placements.insert(pane.pane_id.clone(), placement.clone());
        }
    }

    placements
}

/// Resolve every distinct working directory, several at a time.
///
/// A `git rev-parse` is a few milliseconds; a user with many panes open across many
/// repositories would feel them added up, and the picker has to open instantly.
fn identify_all<'a>(
    git: &dyn GitPort,
    cwds: &HashSet<&'a str>,
) -> BTreeMap<&'a str, Option<PanePlacement>> {
    /// Enough to hide the latency without flooding a laptop with git processes.
    const MAX_IN_FLIGHT: usize = 8;

    let cwds: Vec<&str> = cwds.iter().copied().collect();
    let mut resolved = BTreeMap::new();

    for chunk in cwds.chunks(MAX_IN_FLIGHT) {
        let results: Vec<Option<PanePlacement>> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|cwd| scope.spawn(move || identify_one(git, cwd)))
                .collect();
            handles
                .into_iter()
                // A panicking git resolution must not take the picker down with it; the
                // pane just ends up ungrouped.
                .map(|handle| handle.join().unwrap_or(None))
                .collect()
        });
        for (cwd, placement) in chunk.iter().zip(results) {
            resolved.insert(*cwd, placement);
        }
    }
    resolved
}

fn identify_one(git: &dyn GitPort, cwd: &str) -> Option<PanePlacement> {
    // A path that is not in a repository, and a git that failed, are the same thing here:
    // the pane is simply not grouped.
    let identity = git.identify(cwd).ok().flatten()?;
    Some(PanePlacement {
        repo_key: normalize_path(&identity.repo_key).to_string(),
        checkout_path: normalize_path(&identity.checkout_path).to_string(),
    })
}

/// Whether `path` is `root` or sits underneath it.
fn is_inside(path: &str, root: &str) -> bool {
    let path = normalize_path(path);
    let root = normalize_path(root);
    path == root || path.strip_prefix(root).is_some_and(|r| r.starts_with('/'))
}

/// Ask herdr for the worktrees of every repository a pane was found in.
fn collect_repos(
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    placements: &HashMap<String, PanePlacement>,
) -> Vec<RepoInput> {
    // One checkout per repo is enough to ask about: herdr resolves the whole repository
    // from any path inside it. BTreeMap keeps the result deterministic.
    let mut probe_paths: BTreeMap<&str, &str> = BTreeMap::new();
    for placement in placements.values() {
        probe_paths
            .entry(&placement.repo_key)
            .or_insert(&placement.checkout_path);
    }

    probe_paths
        .into_iter()
        .filter_map(|(repo_key, probe)| {
            let listed = herdr.worktree_list(probe).ok()?;
            let repo_root = normalize_path(&listed.source.repo_root).to_string();
            let display_name = git
                .github_slug(&repo_root)
                .ok()
                .flatten()
                .map(|slug| slug.as_str().to_string())
                .unwrap_or_else(|| listed.source.repo_name.clone());
            Some(RepoInput {
                repo_key: repo_key.to_string(),
                repo_root,
                display_name,
                worktrees: listed.worktrees,
                // Read next, all at once — `read_refs`.
                refs: Ok(Vec::new()),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use anyhow::Result;

    use super::{collect_repos, is_inside, read_refs, resolve_placements};
    use crate::domain::tree::{PanePlacement, RepoInput};
    use crate::port::{
        GitPort, HerdrPort, PaneDestination, PaneSplit, Slug, Snapshot, Worktree, WorktreeCreate,
        WorktreeList, WorktreeOpen, WorktreeOpened, WorktreeSource,
    };

    /// A herdr that knows one repository, and a git that may or may not know its name on
    /// GitHub. Between them they are everything `collect_repos` reads.
    struct Repository {
        slug: Result<Option<Slug>, ()>,
    }

    /// A git whose ref walk goes wrong for one repository root and answers for every other:
    /// refused outright, or answered with a ref it had to drop.
    struct RefsFailFor(&'static str, Trouble);

    /// Which of the two ways the walk went wrong.
    enum Trouble {
        Refused,
        Dropped,
    }

    impl GitPort for RefsFailFor {
        fn local_refs(&self, repo_root: &str) -> Result<crate::port::RefWalk> {
            if repo_root != self.0 {
                return Ok(crate::port::RefWalk::of(Vec::new()));
            }
            match self.1 {
                Trouble::Refused => {
                    anyhow::bail!("fatal: bad ref for\n  refs/heads/x (`git for-each-ref …`)")
                }
                // A warning as the port allows one: the refs git could read, and its words
                // about the ones it could not. `GitCli` folds its own words to a line
                // already and `RefWalk::dropped` promises nothing about lines, so a fake is
                // the only place `one_line` can be seen doing anything on this path.
                Trouble::Dropped => Ok(crate::port::RefWalk {
                    refs: vec![crate::port::GitRef {
                        name: "main".to_string(),
                        kind: crate::port::RefKind::Local,
                        committed_at: None,
                        subject: None,
                        upstream: None,
                        track: Some(crate::port::Track::Gone),
                        worktree_path: Some("/src/app".to_string()),
                    }],
                    dropped: Some(
                        "warning: ignoring broken ref refs/heads/x\n  warning: ignoring \
                         ref with broken name refs/heads/a..b"
                            .to_string(),
                    ),
                }),
            }
        }
        fn github_slug(&self, _repo_root: &str) -> Result<Option<Slug>> {
            unreachable!("only local_refs is asked of this port")
        }
        fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            unreachable!("only local_refs is asked of this port")
        }
        fn remote_heads(&self, _repo_root: &str) -> Result<Vec<String>> {
            unreachable!("only local_refs is asked of this port")
        }
        fn fetch_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!("only local_refs is asked of this port")
        }
        fn fetch_all(&self, _repo_root: &str) -> Result<()> {
            unreachable!("only local_refs is asked of this port")
        }
        fn remove_worktree(&self, _repo_root: &str, _checkout_path: &str) -> Result<()> {
            unreachable!("only local_refs is asked of this port")
        }
        fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
            unreachable!("only local_refs is asked of this port")
        }
        fn head_ref(&self, _repo_root: &str) -> Result<String> {
            unreachable!("only local_refs is asked of this port")
        }
    }

    /// A git whose ref walk panics — a debug build's shape of a walk that did not finish.
    struct RefsPanic;

    impl GitPort for RefsPanic {
        fn local_refs(&self, _repo_root: &str) -> Result<crate::port::RefWalk> {
            panic!("the walk did not finish")
        }
        fn github_slug(&self, _repo_root: &str) -> Result<Option<Slug>> {
            unreachable!("only local_refs is asked of this port")
        }
        fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            unreachable!("only local_refs is asked of this port")
        }
        fn remote_heads(&self, _repo_root: &str) -> Result<Vec<String>> {
            unreachable!("only local_refs is asked of this port")
        }
        fn fetch_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!("only local_refs is asked of this port")
        }
        fn fetch_all(&self, _repo_root: &str) -> Result<()> {
            unreachable!("only local_refs is asked of this port")
        }
        fn remove_worktree(&self, _repo_root: &str, _checkout_path: &str) -> Result<()> {
            unreachable!("only local_refs is asked of this port")
        }
        fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
            unreachable!("only local_refs is asked of this port")
        }
        fn head_ref(&self, _repo_root: &str) -> Result<String> {
            unreachable!("only local_refs is asked of this port")
        }
    }

    fn repo_input(repo_root: &str) -> RepoInput {
        RepoInput {
            repo_key: format!("{repo_root}/.git"),
            repo_root: repo_root.to_string(),
            display_name: repo_root.to_string(),
            worktrees: Vec::new(),
            refs: Ok(Vec::new()),
        }
    }

    #[test]
    fn a_ref_walk_that_failed_keeps_gits_words_on_one_line_and_touches_no_other_repository() {
        // What used to be `unwrap_or_default()`: the failure became an empty list, which is
        // also what a repository with nothing to report looks like. The words are what the
        // prompt line shows, and it is one line, so git's several are folded here.
        let mut repos = vec![repo_input("/src/app"), repo_input("/src/site")];
        read_refs(&RefsFailFor("/src/app", Trouble::Refused), &mut repos);
        assert_eq!(
            repos[0].refs,
            Err("fatal: bad ref for refs/heads/x (`git for-each-ref …`)".to_string())
        );
        assert_eq!(
            repos[1].refs,
            Ok(Vec::new()),
            "the other repository answered"
        );
    }

    #[test]
    fn a_walk_git_dropped_a_ref_from_is_not_this_repository_s_refs() {
        // git exits 0 and lists everything it could read, so the refs are right there and
        // one of them even carries a `gone`. Believing them is believing that the checkout
        // whose ref went missing has nothing to report, which is the wrong marker ADR 0011
        // will not have — so the panes view takes the words instead and every row of the
        // repository goes bare. The branches view is where the other choice is made.
        let mut repos = vec![repo_input("/src/app"), repo_input("/src/site")];
        read_refs(&RefsFailFor("/src/app", Trouble::Dropped), &mut repos);
        assert_eq!(
            repos[0].refs,
            Err(
                "warning: ignoring broken ref refs/heads/x warning: ignoring ref with \
                 broken name refs/heads/a..b"
                    .to_string()
            ),
            "git's words, on one line"
        );
        assert_eq!(
            repos[1].refs,
            Ok(Vec::new()),
            "the other repository answered"
        );
    }

    #[test]
    fn a_ref_walk_whose_thread_did_not_finish_is_not_an_empty_answer() {
        // Debug builds only — the release profile aborts on a panic — but the string is what
        // `Refs::Unreadable` carries, and this line is one `unwrap_or_default` away from
        // the silence #21 was about. The panic prints on stderr; that is the thread's, not
        // this test's.
        let mut repos = vec![repo_input("/src/app")];
        read_refs(&RefsPanic, &mut repos);
        assert_eq!(
            repos[0].refs,
            Err("the thread reading them did not finish".to_string())
        );
    }

    impl HerdrPort for Repository {
        fn worktree_list(&self, _cwd: &str) -> Result<WorktreeList> {
            Ok(WorktreeList {
                source: WorktreeSource {
                    repo_key: "/src/app/.git".into(),
                    // What herdr calls it, which is the directory. The fallback, and the
                    // thing a slug is supposed to be better than.
                    repo_name: "app".into(),
                    repo_root: "/src/app".into(),
                    source_checkout_path: "/src/app".into(),
                    source_workspace_id: None,
                },
                worktrees: Vec::<Worktree>::new(),
            })
        }
        fn snapshot(&self) -> Result<Snapshot> {
            unreachable!("only worktree_list is asked of this port")
        }
        fn worktree_create(&self, _req: &WorktreeCreate) -> Result<WorktreeOpened> {
            unreachable!("only worktree_list is asked of this port")
        }
        fn worktree_open(&self, _req: &WorktreeOpen) -> Result<WorktreeOpened> {
            unreachable!("only worktree_list is asked of this port")
        }
        fn pane_focus(&self, _pane_id: &str) -> Result<()> {
            unreachable!("only worktree_list is asked of this port")
        }
        fn pane_split(&self, _req: &PaneSplit) -> Result<crate::port::Pane> {
            unreachable!("only worktree_list is asked of this port")
        }
        fn pane_move(&self, _pane: &str, _dest: &PaneDestination, _focus: bool) -> Result<()> {
            unreachable!("only worktree_list is asked of this port")
        }
        fn pane_close(&self, _pane_id: &str) -> Result<()> {
            unreachable!("only worktree_list is asked of this port")
        }
        fn workspace_focus(&self, _workspace_id: &str) -> Result<()> {
            unreachable!("only worktree_list is asked of this port")
        }
        fn tab_focus(&self, _tab_id: &str) -> Result<()> {
            unreachable!("only worktree_list is asked of this port")
        }
        fn plugin_pane_open(
            &self,
            _req: &crate::port::PluginPaneOpen,
        ) -> Result<Option<crate::port::OpenRefusal>> {
            unreachable!("only worktree_list is asked of this port")
        }
        fn notify(&self, _notification: &crate::port::Notification) -> Result<()> {
            unreachable!("only worktree_list is asked of this port")
        }
    }

    impl GitPort for Repository {
        fn github_slug(&self, _repo_root: &str) -> Result<Option<Slug>> {
            match &self.slug {
                Ok(slug) => Ok(slug.clone()),
                Err(()) => Err(anyhow::anyhow!("fatal: not a git repository")),
            }
        }
        fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            unreachable!("only github_slug is asked of this port")
        }
        fn local_refs(&self, _repo_root: &str) -> Result<crate::port::RefWalk> {
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

    fn one_pane_in(checkout_path: &str) -> HashMap<String, PanePlacement> {
        HashMap::from([(
            "w1:p1".to_string(),
            PanePlacement {
                repo_key: "/src/app/.git".to_string(),
                checkout_path: checkout_path.to_string(),
            },
        )])
    }

    fn named(slug: Result<Option<Slug>, ()>) -> String {
        let port = Repository { slug };
        let repos = collect_repos(&port, &port, &one_pane_in("/src/app"));
        repos
            .into_iter()
            .next()
            .expect("herdr answered for the one repository")
            .display_name
    }

    #[test]
    fn a_repository_is_labelled_by_what_github_calls_it() {
        // The header row above every repository's worktrees, on every picker open. Nothing
        // else in the suite reads it, so blanking it here costs nothing that a test notices
        // and everything that a user does.
        assert_eq!(
            named(Ok(Slug::owner_repo("ShoMasegi", "app"))),
            "ShoMasegi/app"
        );
    }

    #[test]
    fn a_repository_github_does_not_know_keeps_the_name_herdr_gave_it() {
        // And a git that would not answer is the same case: neither is a reason to show a
        // path, and neither is a reason to show nothing.
        for slug in [Ok(None), Err(())] {
            assert_eq!(named(slug), "app");
        }
    }

    /// A git that identifies every path as one repository, spelling `repo_key` with a
    /// trailing slash — which `git rev-parse --git-common-dir` does for a worktree.
    struct IdentifiesWithASlash;

    impl GitPort for IdentifiesWithASlash {
        fn identify(&self, cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            Ok(Some(crate::port::RepoIdentity {
                repo_key: "/src/app/.git/".to_string(),
                checkout_path: cwd.to_string(),
                branch: None,
            }))
        }
        fn github_slug(&self, _repo_root: &str) -> Result<Option<Slug>> {
            unreachable!("only identify is asked of this port")
        }
        fn local_refs(&self, _repo_root: &str) -> Result<crate::port::RefWalk> {
            unreachable!("only identify is asked of this port")
        }
        fn remote_heads(&self, _repo_root: &str) -> Result<Vec<String>> {
            unreachable!("only identify is asked of this port")
        }
        fn fetch_branch(&self, _repo_root: &str, _branch: &str) -> Result<()> {
            unreachable!("only identify is asked of this port")
        }
        fn fetch_all(&self, _repo_root: &str) -> Result<()> {
            unreachable!("only identify is asked of this port")
        }
        fn remove_worktree(&self, _repo_root: &str, _checkout_path: &str) -> Result<()> {
            unreachable!("only identify is asked of this port")
        }
        fn is_dirty(&self, _checkout_path: &str) -> Result<bool> {
            unreachable!("only identify is asked of this port")
        }
        fn head_ref(&self, _repo_root: &str) -> Result<String> {
            unreachable!("only identify is asked of this port")
        }
    }

    #[test]
    fn one_repository_is_asked_about_once_however_many_panes_are_in_it() {
        // What `domain::model::Refs::Unreadable`'s doc leans on. Two `RepoInput`s for one
        // repository would pool a readable answer under an unreadable node in
        // `domain::tree::tracks`, whose key separates repositories and not spellings of
        // one — and a repository whose refs git would not read would come back marked.
        // Nothing in `domain` prevents that; this is where it is prevented, and every other
        // test here hands `collect_repos` a single placement, which has nothing to dedupe.
        let port = Repository {
            slug: Ok(Slug::owner_repo("ShoMasegi", "app")),
        };
        let two_panes = HashMap::from([
            (
                "w1:p1".to_string(),
                PanePlacement {
                    repo_key: "/src/app/.git".to_string(),
                    checkout_path: "/src/app".to_string(),
                },
            ),
            (
                "w1:p2".to_string(),
                PanePlacement {
                    repo_key: "/src/app/.git".to_string(),
                    checkout_path: "/wt/feat-login".to_string(),
                },
            ),
        ]);
        let repos = collect_repos(&port, &port, &two_panes);
        assert_eq!(
            repos.iter().map(|repo| &repo.repo_key).collect::<Vec<_>>(),
            ["/src/app/.git"],
            "one repository, asked about once"
        );
    }

    #[test]
    fn a_placement_carries_one_spelling_of_a_repository_key_whichever_answered() {
        // And this is why the `BTreeMap` above is enough: it keys on the string it is
        // given, so two spellings of one repository would be two entries and the dedupe
        // would not happen. Both places a `PanePlacement` is made normalize first — herdr's
        // own record of a workspace, and git's answer for a pane that has wandered out of
        // one — so the strings it compares are already the strings `domain::tree` will key
        // on. Here herdr spells it with a slash and git spells it with a slash, and one
        // pane takes each route.
        let snapshot: Snapshot = serde_json::from_value(serde_json::json!({
            "version": "0.7.4",
            "protocol": 16,
            "workspaces": [{
                "workspace_id": "w1",
                "worktree": {
                    "repo_key": "/src/app/.git/",
                    "repo_name": "app",
                    "repo_root": "/src/app",
                    "checkout_path": "/src/app",
                },
            }],
            "tabs": [],
            "panes": [
                {
                    "pane_id": "w1:p1",
                    "tab_id": "w1:t1",
                    "workspace_id": "w1",
                    "terminal_id": "t1",
                    "cwd": "/src/app/src",
                },
                {
                    "pane_id": "w1:p2",
                    "tab_id": "w1:t1",
                    "workspace_id": "w1",
                    "terminal_id": "t2",
                    "cwd": "/wt/feat-login",
                },
            ],
        }))
        .expect("snapshot fixture should deserialize");

        let placements = resolve_placements(&snapshot, &IdentifiesWithASlash);
        let mut keys: Vec<&str> = placements
            .values()
            .map(|placement| placement.repo_key.as_str())
            .collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(
            keys,
            ["/src/app/.git"],
            "herdr's route and git's route agree on the spelling: {placements:?}"
        );
    }

    #[test]
    fn recognises_a_pane_that_is_still_inside_its_workspace_checkout() {
        assert!(is_inside("/src/app", "/src/app"));
        assert!(is_inside("/src/app/", "/src/app"));
        assert!(is_inside("/src/app/src/ui", "/src/app"));
    }

    #[test]
    fn rejects_a_sibling_directory_that_merely_shares_a_prefix() {
        // The case that a naive starts_with would get wrong.
        assert!(!is_inside("/src/app-tools", "/src/app"));
        assert!(!is_inside("/src/other", "/src/app"));
        assert!(!is_inside("/src", "/src/app"));
    }
}
