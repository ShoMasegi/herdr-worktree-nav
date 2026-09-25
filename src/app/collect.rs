//! Gathering the inputs [`domain::tree::build`](crate::domain::tree::build) needs.
//!
//! This is the impure half of building the panes view: it asks herdr for the session, works
//! out which repository and checkout every pane is in, and asks herdr for each repository's
//! worktrees. The decisions all live in `domain`; this module only fetches.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::Result;

use crate::app::one_line;
use crate::domain::model::{normalize_path, Tree, Unlisted};
use crate::domain::tree::{self, PanePlacement, RepoInput};
use crate::port::{GitPort, HerdrPort, Snapshot};

/// Fetch everything and build the tree.
pub fn collect_tree(herdr: &dyn HerdrPort, git: &dyn GitPort) -> Result<(Snapshot, Tree)> {
    let snapshot = herdr.snapshot()?;
    let (placements, unplaced) = resolve_placements(&snapshot, git);
    let (mut repos, unlisted) = collect_repos(herdr, git, &placements);
    read_refs(git, &mut repos);
    // After `build` rather than through it: assembling the tree from what was read and
    // recording what could not be are different questions, and `build` is the pure half.
    let mut tree = tree::build(&snapshot, &repos, &placements);
    tree.trouble.unlisted = unlisted;
    tree.trouble.unplaced = unplaced;
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
/// known the moment git answers.
///
/// A repository git could not answer for carries no markers and says so: git's words go on
/// the repository, and the prompt line names it once —
/// [`domain::notice::conditions`](crate::domain::notice::conditions).
fn read_refs(git: &dyn GitPort, repos: &mut [RepoInput]) {
    // No chunking: repositories are however many the user has panes open in, which is a
    // handful — unlike working directories, where every pane can have its own.
    std::thread::scope(|scope| {
        let handles: Vec<_> = repos
            .iter()
            .map(|repo| {
                let repo_root = repo.repo_root.clone();
                scope.spawn(move || {
                    // A walk that dropped a ref is a refusal here too: that ref is the
                    // whole answer for the checkout it belongs to, and no marker beats the
                    // wrong one — `port::RefWalk` says why the branches view chooses
                    // otherwise.
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
            // `join` fails for one reason: the thread panicked, which only a debug build
            // reaches — `panic = "abort"` in the release profile ends the process before
            // this line. That repository's markers are missing, and it says so like any
            // other repository whose refs were not read.
            repo.refs = handle
                .join()
                .unwrap_or_else(|_| Err("the thread reading them did not finish".to_string()));
        }
    });
}

/// Work out which repository and checkout each pane sits in, and which panes git would not
/// answer for.
///
/// Two shortcuts keep this cheap. Panes are resolved per working directory rather than per
/// pane, because several panes usually share one; and when herdr already knows a workspace
/// is a worktree, its answer is reused instead of running git — but only for panes that are
/// still somewhere under that checkout, since a pane is free to `cd` into another repository.
///
/// A git that refused is kept apart from a path that is simply not in a repository. They are
/// one value away from each other here and nothing alike to a reader: the second is an
/// ordinary pane in an ordinary directory, and the first is every pane in the session at once
/// when `git` is not on the path herdr launched the plugin with. Issue #33.
fn resolve_placements(
    snapshot: &Snapshot,
    git: &dyn GitPort,
) -> (HashMap<String, PanePlacement>, BTreeMap<String, String>) {
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
    let mut unplaced = BTreeMap::new();

    for pane in &snapshot.panes {
        if placements.contains_key(&pane.pane_id) {
            continue;
        }
        let Some(cwd) = pane.effective_cwd() else {
            continue;
        };
        match resolved.get(cwd) {
            Some(Ok(Some(placement))) => {
                placements.insert(pane.pane_id.clone(), placement.clone());
            }
            // git answered, and the answer is that this path is not in a repository. An
            // ordinary pane in an ordinary directory; the section at the bottom is for it.
            Some(Ok(None)) | None => {}
            Some(Err(words)) => {
                unplaced.insert(pane.pane_id.clone(), words.clone());
            }
        }
    }

    (placements, unplaced)
}

/// Resolve every distinct working directory, several at a time.
///
/// A `git rev-parse` is a few milliseconds; a user with many panes open across many
/// repositories would feel them added up, and the picker has to open instantly.
type Placed = Result<Option<PanePlacement>, String>;

fn identify_all<'a>(git: &dyn GitPort, cwds: &HashSet<&'a str>) -> BTreeMap<&'a str, Placed> {
    /// Enough to hide the latency without flooding a laptop with git processes.
    const MAX_IN_FLIGHT: usize = 8;

    let cwds: Vec<&str> = cwds.iter().copied().collect();
    let mut resolved = BTreeMap::new();

    for chunk in cwds.chunks(MAX_IN_FLIGHT) {
        let results: Vec<Placed> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|cwd| scope.spawn(move || identify_one(git, cwd)))
                .collect();
            handles
                .into_iter()
                // A panicking git resolution must not take the picker down with it. The
                // pane ends up ungrouped and says why, the way `read_refs` does for a walk
                // whose thread did not finish.
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err("the thread asking git did not finish".into()))
                })
                .collect()
        });
        for (cwd, placement) in chunk.iter().zip(results) {
            resolved.insert(*cwd, placement);
        }
    }
    resolved
}

/// `Ok(None)` is git saying the path is not in a repository. `Err` is git not saying —
/// refused, or not started at all — which is a different thing to tell the user and the
/// difference between one odd pane and a session that cannot be grouped.
fn identify_one(git: &dyn GitPort, cwd: &str) -> Placed {
    let identity = match git.identify(cwd) {
        Ok(Some(identity)) => identity,
        Ok(None) => return Ok(None),
        Err(error) => return Err(one_line(&format!("{error:#}"))),
    };
    Ok(Some(PanePlacement {
        repo_key: normalize_path(&identity.repo_key).to_string(),
        checkout_path: normalize_path(&identity.checkout_path).to_string(),
    }))
}

/// Whether `path` is `root` or sits underneath it.
fn is_inside(path: &str, root: &str) -> bool {
    let path = normalize_path(path);
    let root = normalize_path(root);
    path == root || path.strip_prefix(root).is_some_and(|r| r.starts_with('/'))
}

/// Ask herdr for the worktrees of every repository a pane was found in.
///
/// A repository herdr refuses to list comes back as the second half rather than as nothing.
/// Dropped, it takes every checkout and every pane in it off the screen — and with a sweep's
/// `Enter` reading the tree again before it asks, that lands between the marks going on and
/// the question being asked, where `no longer marked:` with bare paths is all the reader
/// gets unless the reason travels with the tree. Issue #56.
fn collect_repos(
    herdr: &dyn HerdrPort,
    git: &dyn GitPort,
    placements: &HashMap<String, PanePlacement>,
) -> (Vec<RepoInput>, Vec<Unlisted>) {
    // Every checkout a pane was found in, by repository, in path order. herdr resolves the
    // whole repository from any path inside it, so the first checkout that answers is the
    // answer and the rest are asked only when one refuses. Whatever makes herdr refuse one
    // checkout and answer for another, which of a repository's checkouts happened to come
    // first out of a `HashMap` must not decide whether the repository is on screen.
    let mut panes: BTreeMap<&str, BTreeMap<&str, &str>> = BTreeMap::new();
    for (pane_id, placement) in placements {
        panes
            .entry(&placement.repo_key)
            .or_default()
            .insert(pane_id, &placement.checkout_path);
    }

    let mut repos = Vec::new();
    let mut unlisted = Vec::new();
    for (repo_key, panes) in panes {
        let probes: BTreeSet<&str> = panes.values().copied().collect();
        // What was said about the first checkout, which is what a reader would have been
        // told had it been the only one.
        let mut words = None;
        let listed = probes
            .iter()
            .find_map(|probe| match herdr.worktree_list(probe) {
                // An answer about another repository is not this one's listing: herdr
                // resolved the path to a different `repo_key` from the one the pane was
                // placed in. Filed under this key it would draw that repository in this
                // one's place, and this one nowhere, so it counts as a refusal.
                Ok(listed) if normalize_path(&listed.source.repo_key) != repo_key => {
                    words.get_or_insert_with(|| {
                        let other = normalize_path(&listed.source.repo_key);
                        format!("herdr listed {probe} as {other}")
                    });
                    None
                }
                Ok(listed) => Some(listed),
                Err(error) => {
                    words.get_or_insert_with(|| one_line(&format!("{error:#}")));
                    None
                }
            });
        let Some(listed) = listed else {
            unlisted.push(Unlisted {
                repo_key: repo_key.to_string(),
                // Never empty: a repository is here because a pane is in one of its
                // checkouts, and every one of them refused.
                words: words.unwrap_or_default(),
                panes: panes
                    .iter()
                    .map(|(pane_id, checkout)| (pane_id.to_string(), checkout.to_string()))
                    .collect(),
            });
            continue;
        };
        let repo_root = normalize_path(&listed.source.repo_root).to_string();
        let display_name = git
            .github_slug(&repo_root)
            .ok()
            .flatten()
            .map(|slug| slug.as_str().to_string())
            .unwrap_or_else(|| listed.source.repo_name.clone());
        repos.push(RepoInput {
            repo_key: repo_key.to_string(),
            repo_root,
            display_name,
            worktrees: listed.worktrees,
            // Read next, all at once — `read_refs`.
            refs: Ok(Vec::new()),
        });
    }
    (repos, unlisted)
}

#[cfg(test)]
mod tests {
    use crate::app::fakes::{fake_git, fake_herdr, FakeGit, FakeHerdr};
    use std::collections::HashMap;

    use anyhow::Result;

    use super::{collect_repos, collect_tree, is_inside, read_refs, resolve_placements};
    use crate::domain::tree::{PanePlacement, RepoInput};
    use crate::port::{
        AgentStatus, Pane, Slug, Snapshot, Workspace, WorkspaceWorktree, Worktree, WorktreeList,
        WorktreeSource,
    };

    /// A herdr that knows one repository, and a git that may or may not know its name on
    /// GitHub. Between them they are everything `collect_repos` reads — and, with the
    /// session and the ref walk below, everything `collect_tree` reads as well.
    ///
    /// `collect_repos` is handed its placements, so it cannot reach `snapshot` whatever this
    /// answers; the other methods stay `unreachable!()`. The git half refuses `identify`,
    /// which is what the session below has one pane needing.
    struct Repository {
        slug: Result<Option<Slug>, ()>,
    }

    /// A git whose ref walk goes wrong for one repository root and answers for every other:
    /// refused outright, or answered with a ref it had to drop.
    struct RefsFailFor(&'static str, Trouble);

    enum Trouble {
        Refused,
        Dropped,
    }

    impl FakeGit for RefsFailFor {
        fn local_refs(&self, repo_root: &str) -> Result<crate::port::RefWalk> {
            if repo_root != self.0 {
                return Ok(crate::port::RefWalk::of(Vec::new()));
            }
            match self.1 {
                Trouble::Refused => {
                    anyhow::bail!("fatal: bad ref for\n  refs/heads/x (`git for-each-ref …`)")
                }
                // `GitCli` folds its own words to a line already and `RefWalk::dropped`
                // promises nothing about lines, so a fake is the only place `one_line` can
                // be seen doing anything on this path.
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
    }
    fake_git!(RefsFailFor);

    /// A git whose ref walk panics — a debug build's shape of a walk that did not finish.
    struct RefsPanic;

    impl FakeGit for RefsPanic {
        fn local_refs(&self, _repo_root: &str) -> Result<crate::port::RefWalk> {
            panic!("the walk did not finish")
        }
    }
    fake_git!(RefsPanic);

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
        // Why not `unwrap_or_default()`: the failure would become an empty list, which is
        // also what a repository with nothing to report looks like.
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
        // whose ref went missing has nothing to report — the wrong marker ADR 0011 will
        // not have.
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
        // Debug builds only — the release profile aborts on a panic — but the string is
        // what `Refs::Unreadable` carries, and this line is one `unwrap_or_default` away
        // from the silence #21 was about. The panic prints on stderr, from the thread.
        let mut repos = vec![repo_input("/src/app")];
        read_refs(&RefsPanic, &mut repos);
        assert_eq!(
            repos[0].refs,
            Err("the thread reading them did not finish".to_string())
        );
    }

    impl FakeHerdr for Repository {
        fn worktree_list(&self, cwd: &str) -> Result<WorktreeList> {
            // One probe path herdr will not answer for, so the other half of this call is
            // reachable without a second fake. Every other path is the one repository.
            if cwd.contains("refused") {
                anyhow::bail!("herdr rejected worktree.list: internal error");
            }
            Ok(WorktreeList {
                source: WorktreeSource {
                    repo_key: "/src/app/.git".into(),
                    // What herdr calls it, which is the directory: the fallback a slug is
                    // supposed to be better than.
                    repo_name: "app".into(),
                    repo_root: "/src/app".into(),
                    source_checkout_path: "/src/app".into(),
                    source_workspace_id: None,
                },
                worktrees: Vec::<Worktree>::new(),
            })
        }
        fn snapshot(&self) -> Result<Snapshot> {
            // One pane, in a workspace herdr already knows the worktree of, so no git is
            // asked to place it — and in the one repository this herdr will not list.
            Ok(Snapshot {
                workspaces: vec![Workspace {
                    workspace_id: "w1".into(),
                    label: String::new(),
                    number: 0,
                    focused: false,
                    active_tab_id: None,
                    agent_status: AgentStatus::Unknown,
                    worktree: Some(WorkspaceWorktree {
                        repo_key: "/src/refused/.git".into(),
                        repo_name: "refused".into(),
                        repo_root: "/src/refused".into(),
                        checkout_path: "/src/refused".into(),
                        is_linked_worktree: false,
                    }),
                }],
                panes: vec![
                    Pane {
                        pane_id: "w1:p1".into(),
                        tab_id: "w1:t1".into(),
                        workspace_id: "w1".into(),
                        terminal_id: String::new(),
                        cwd: Some("/src/refused".into()),
                        foreground_cwd: None,
                        focused: false,
                        agent: None,
                        agent_status: AgentStatus::Unknown,
                        title: None,
                        terminal_title_stripped: None,
                        label: None,
                    },
                    // In no workspace herdr knows a worktree of, so git is asked about it —
                    // and this git will not answer.
                    Pane {
                        pane_id: "w9:p9".into(),
                        tab_id: "w9:t1".into(),
                        workspace_id: "w9".into(),
                        terminal_id: String::new(),
                        cwd: Some("/home/me".into()),
                        foreground_cwd: None,
                        focused: false,
                        agent: None,
                        agent_status: AgentStatus::Unknown,
                        title: None,
                        terminal_title_stripped: None,
                        label: None,
                    },
                ],
                ..Snapshot::default()
            })
        }
    }
    fake_herdr!(Repository);

    impl FakeGit for Repository {
        fn github_slug(&self, _repo_root: &str) -> Result<Option<Slug>> {
            match &self.slug {
                Ok(slug) => Ok(slug.clone()),
                Err(()) => Err(anyhow::anyhow!("fatal: not a git repository")),
            }
        }
        fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            Err(anyhow::anyhow!(
                "git could not be run: no such file or directory (`git rev-parse`)"
            ))
        }
        fn local_refs(&self, _repo_root: &str) -> Result<crate::port::RefWalk> {
            // Reached only through `collect_tree`; a repository that was never listed is
            // never walked, so this answers for the ones that were.
            Ok(crate::port::RefWalk::of(Vec::new()))
        }
    }
    fake_git!(Repository);

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
        let (repos, _) = collect_repos(&port, &port, &one_pane_in("/src/app"));
        repos
            .into_iter()
            .next()
            .expect("herdr answered for the one repository")
            .display_name
    }

    #[test]
    fn a_repository_is_labelled_by_what_github_calls_it() {
        // Nothing else in the suite reads the header row above a repository's worktrees,
        // so blanking it costs nothing a test notices and everything a user does.
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

    /// A git that answers only for paths outside `/src/app`, so a pane inside it has to
    /// take herdr's route — and spells `repo_key` with a trailing slash, so
    /// `identify_one`'s normalization has something to do.
    struct IdentifiesWithASlash;

    impl FakeGit for IdentifiesWithASlash {
        fn identify(&self, cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            if is_inside(cwd, "/src/app") {
                return Ok(None);
            }
            Ok(Some(crate::port::RepoIdentity {
                repo_key: "/src/app/.git/".to_string(),
                checkout_path: cwd.to_string(),
                branch: None,
            }))
        }
    }
    fake_git!(IdentifiesWithASlash);

    #[test]
    fn one_repository_is_asked_about_once_however_many_panes_are_in_it() {
        // Two `RepoInput`s for one repository would let a readable answer reach the
        // checkouts of an unreadable node in `domain::tree::tracks`, whose key tells
        // repositories apart and not spellings of one.
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
        let (repos, _) = collect_repos(&port, &port, &two_panes);
        assert_eq!(
            repos.iter().map(|repo| &repo.repo_key).collect::<Vec<_>>(),
            ["/src/app/.git"],
            "one repository, asked about once"
        );
    }

    #[test]
    fn a_repository_herdr_would_not_list_is_kept_with_its_words_and_takes_nothing_with_it() {
        // Dropped, it takes every checkout and every pane in it off the screen, and the
        // prompt line has nothing to name — `Refs::Unreadable` hangs off the repository
        // node, which is exactly what does not exist here. Issue #56.
        let port = Repository { slug: Ok(None) };
        let two_repos = HashMap::from([
            (
                "w1:p1".to_string(),
                PanePlacement {
                    repo_key: "/src/app/.git".to_string(),
                    checkout_path: "/src/app".to_string(),
                },
            ),
            (
                "w2:p1".to_string(),
                PanePlacement {
                    repo_key: "/src/old/.git".to_string(),
                    checkout_path: "/src/refused".to_string(),
                },
            ),
        ]);

        let (repos, unlisted) = collect_repos(&port, &port, &two_repos);

        assert_eq!(
            repos.iter().map(|repo| &repo.repo_key).collect::<Vec<_>>(),
            ["/src/app/.git"],
            "the repository that answered is untouched"
        );
        assert_eq!(unlisted.len(), 1, "and the other is kept, not dropped");
        assert_eq!(unlisted[0].repo_key, "/src/old/.git");
        assert_eq!(
            unlisted[0]
                .panes
                .iter()
                .map(|(pane, checkout)| (pane.as_str(), checkout.as_str()))
                .collect::<Vec<_>>(),
            [("w2:p1", "/src/refused")],
            "and where its pane is, which no row of it will say"
        );
        assert_eq!(
            unlisted[0].words, "herdr rejected worktree.list: internal error",
            "herdr's own words, on one line"
        );
    }

    #[test]
    fn a_repository_one_of_whose_checkouts_herdr_refuses_is_listed_through_another() {
        // Two panes in one repository, one of them in a checkout herdr will not answer for.
        // Asking only one of them makes the answer turn on which one that is — and taken
        // from the `HashMap` of placements, the same session draws the repository on one
        // reading and `not listed` on the next.
        let port = Repository { slug: Ok(None) };
        let two_checkouts = HashMap::from([
            (
                "w1:p1".to_string(),
                PanePlacement {
                    repo_key: "/src/app/.git".to_string(),
                    // First in path order, so it is the one asked first.
                    checkout_path: "/src/refused".to_string(),
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

        let (repos, unlisted) = collect_repos(&port, &port, &two_checkouts);

        assert_eq!(
            repos.iter().map(|repo| &repo.repo_key).collect::<Vec<_>>(),
            ["/src/app/.git"],
            "the checkout that answered speaks for the repository"
        );
        assert!(unlisted.is_empty(), "{unlisted:?}");
    }

    /// A herdr that answers for no checkout at all, naming each one it was asked about.
    struct RefusesEach;

    impl FakeHerdr for RefusesEach {
        fn worktree_list(&self, cwd: &str) -> Result<WorktreeList> {
            anyhow::bail!("herdr rejected worktree.list: no checkout at {cwd}")
        }
    }

    fake_herdr!(RefusesEach);

    #[test]
    fn a_repository_every_checkout_of_which_herdr_refuses_is_named_in_what_it_said_first() {
        // One sentence for the repository, and the one a reader would have met had its first
        // checkout been the only one asked about.
        let git = Repository { slug: Ok(None) };
        let two_checkouts = HashMap::from([
            (
                "w1:p1".to_string(),
                PanePlacement {
                    repo_key: "/src/app/.git".to_string(),
                    checkout_path: "/wt/second".to_string(),
                },
            ),
            (
                "w1:p2".to_string(),
                PanePlacement {
                    repo_key: "/src/app/.git".to_string(),
                    checkout_path: "/src/first".to_string(),
                },
            ),
        ]);

        let (repos, unlisted) = collect_repos(&RefusesEach, &git, &two_checkouts);

        assert!(repos.is_empty());
        assert_eq!(
            unlisted
                .iter()
                .map(|repo| repo.words.as_str())
                .collect::<Vec<_>>(),
            ["herdr rejected worktree.list: no checkout at /src/first"]
        );
    }

    /// A herdr that refuses one checkout and answers for another as a different repository.
    struct AnswersForAnother;

    impl FakeHerdr for AnswersForAnother {
        fn worktree_list(&self, cwd: &str) -> Result<WorktreeList> {
            if cwd.contains("refused") {
                anyhow::bail!("herdr rejected worktree.list: internal error");
            }
            Ok(WorktreeList {
                source: WorktreeSource {
                    repo_key: "/src/other/.git/".into(),
                    repo_name: "other".into(),
                    repo_root: "/src/other".into(),
                    source_checkout_path: cwd.into(),
                    source_workspace_id: None,
                },
                worktrees: Vec::<Worktree>::new(),
            })
        }
    }

    fake_herdr!(AnswersForAnother);

    #[test]
    fn a_checkout_herdr_answers_for_as_another_repository_is_not_this_ones_listing() {
        // Asking the next checkout after a refusal must not turn the refusal into another
        // repository's listing filed under this key: that draws `other` in `app`'s place,
        // and says nothing about `app`.
        let git = Repository { slug: Ok(None) };
        let placement = |checkout: &str| PanePlacement {
            repo_key: "/src/app/.git".to_string(),
            checkout_path: checkout.to_string(),
        };
        let words = |unlisted: &[crate::domain::model::Unlisted]| {
            unlisted
                .iter()
                .map(|repo| repo.words.clone())
                .collect::<Vec<_>>()
        };

        let (repos, unlisted) = collect_repos(
            &AnswersForAnother,
            &git,
            &HashMap::from([
                ("w1:p1".to_string(), placement("/src/refused")),
                ("w1:p2".to_string(), placement("/wt/recloned")),
            ]),
        );
        assert!(repos.is_empty(), "no listing of `app` came back");
        assert_eq!(
            words(&unlisted),
            ["herdr rejected worktree.list: internal error"],
            "and the first thing herdr said about it is what is said"
        );

        let (repos, unlisted) = collect_repos(
            &AnswersForAnother,
            &git,
            &HashMap::from([("w1:p2".to_string(), placement("/wt/recloned"))]),
        );
        assert!(repos.is_empty());
        assert_eq!(
            words(&unlisted),
            ["herdr listed /wt/recloned as /src/other/.git"],
            "the only answer is the one about another repository, and that is the reason"
        );
    }

    #[test]
    fn a_reading_hands_the_tree_what_it_could_not_list() {
        // The other half of the same issue: `collect_repos` keeping the words is worth
        // nothing until they are on the tree, which is the one thing above this that can
        // say them. Deleted, every test below still passes and the picker says nothing.
        let port = Repository { slug: Ok(None) };

        let (_, tree) = collect_tree(&port, &port).expect("herdr described its session");

        assert!(tree.repos.is_empty(), "the one repository was not listed");
        assert_eq!(
            tree.trouble
                .unlisted
                .iter()
                .map(|repo| (repo.name(), repo.words.as_str()))
                .collect::<Vec<_>>(),
            [("refused", "herdr rejected worktree.list: internal error")],
            "and the tree carries what herdr said about it"
        );
        assert_eq!(
            tree.trouble
                .unplaced
                .iter()
                .map(|(pane, words)| (pane.as_str(), words.as_str()))
                .collect::<Vec<_>>(),
            [(
                "w9:p9",
                "git could not be run: no such file or directory (`git rev-parse`)"
            )],
            "and what git said about the pane it could not place"
        );
    }

    #[test]
    fn a_placement_carries_one_spelling_of_a_repository_key_whichever_answered() {
        // The `BTreeMap` above compares the strings it is given, so two spellings of one
        // repository would be two entries. Here herdr and git each spell it with a
        // trailing slash, and one pane takes each route.
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

        let (placements, _) = resolve_placements(&snapshot, &IdentifiesWithASlash);
        for pane in ["w1:p1", "w1:p2"] {
            assert_eq!(
                placements[pane].repo_key, "/src/app/.git",
                "{pane} took its route and got one spelling: {placements:?}"
            );
        }
    }

    /// A git that will not start at all — what a `PATH` without git looks like from here.
    /// It answers for nothing, and the same way every time.
    struct NoGit;

    impl FakeGit for NoGit {
        fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            Err(anyhow::anyhow!(
                "git could not be run: no such file or directory (`git rev-parse`)"
            ))
        }
    }
    fake_git!(NoGit);

    #[test]
    fn a_pane_git_would_not_answer_about_is_kept_apart_from_one_that_is_simply_outside() {
        // Read as one answer, a `git` that is not on the path draws the whole session
        // under "not in any repository" — the one thing the troubleshooting page says
        // means herdr could not see into the pane. Issue #33.
        let snapshot: Snapshot = serde_json::from_value(serde_json::json!({
            "version": "0.7.4",
            "protocol": 16,
            "workspaces": [],
            "tabs": [],
            "panes": [{
                "pane_id": "w1:p1",
                "tab_id": "w1:t1",
                "workspace_id": "w1",
                "terminal_id": "t1",
                "cwd": "/home/me",
            }],
        }))
        .expect("snapshot fixture should deserialize");

        let (placements, unplaced) = resolve_placements(&snapshot, &NoGit);
        assert!(placements.is_empty(), "git answered for nothing");
        assert_eq!(
            unplaced.get("w1:p1").map(String::as_str),
            Some("git could not be run: no such file or directory (`git rev-parse`)"),
            "and said why, in git's own words"
        );

        // The other half of the same call: a path git says is not in a repository is an
        // ordinary pane, and has nothing to say about it.
        let (placements, unplaced) = resolve_placements(&snapshot, &IdentifiesNothing);
        assert!(placements.is_empty());
        assert!(unplaced.is_empty(), "git answered, and the answer was no");
    }

    /// A git that answers, and says every path is outside a repository.
    struct IdentifiesNothing;

    impl FakeGit for IdentifiesNothing {
        fn identify(&self, _cwd: &str) -> Result<Option<crate::port::RepoIdentity>> {
            Ok(None)
        }
    }
    fake_git!(IdentifiesNothing);

    #[test]
    fn recognises_a_pane_that_is_still_inside_its_workspace_checkout() {
        assert!(is_inside("/src/app", "/src/app"));
        assert!(is_inside("/src/app/", "/src/app"));
        assert!(is_inside("/src/app/src/ui", "/src/app"));
    }

    #[test]
    fn rejects_a_sibling_directory_that_merely_shares_a_prefix() {
        assert!(!is_inside("/src/app-tools", "/src/app"));
        assert!(!is_inside("/src/other", "/src/app"));
        assert!(!is_inside("/src", "/src/app"));
    }
}
