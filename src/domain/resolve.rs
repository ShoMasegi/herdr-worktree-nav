//! Working out what a branch currently *is*, and what picking it should do.
//!
//! A branch name in the picker can mean five different things, and each one needs a
//! different first step. Getting this wrong is what makes worktree tooling annoying: a
//! second checkout of a branch you already have open, or a "branch not found" for something
//! that is plainly on GitHub.

use std::collections::BTreeMap;

use crate::domain::model::RepoNode;
use crate::domain::order::Order;
use crate::port::{GitRef, PullRequest, RefKind, Track};

/// What a branch resolved to, in the order the picker prefers to show them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchState {
    /// A pane is already working on this branch. Picking it should go there, not make a
    /// second copy of the same work.
    LivePane {
        pane_id: String,
        checkout_path: String,
    },
    /// The checkout exists but nothing is running in it.
    IdleWorktree { checkout_path: String },
    /// A local ref, with no checkout yet.
    LocalRef,
    /// Only on the remote — visible via `ls-remote` but never fetched, so there is no local
    /// ref to base a worktree on yet.
    RemoteOnly,
    /// A name that exists nowhere: the user typed it to create it.
    New,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchEntry {
    pub name: String,
    pub state: BranchState,
    /// Subject of the branch's tip commit, when git knew it.
    pub subject: Option<String>,
    pub committed_at: Option<i64>,
    /// Decoration only — an open pull request whose head is this branch.
    pub pull_request: Option<PullRequest>,
    /// Where the branch stands against what it tracks, when git said anything. Only ever set
    /// from a local ref: a branch that exists nowhere but the remote has nothing to be
    /// measured against.
    ///
    /// The whole value rather than the one bit the branches view draws today. The bit is a
    /// projection anybody can take — see [`BranchEntry::upstream_gone`] — and taking it at
    /// the boundary instead threw away ahead/behind on the way in, so a later reader wanting
    /// what the panes view already shows would have had to rebuild the boundary to get it.
    ///
    /// `None` is four different situations and does not tell them apart: the branch is level
    /// with what it tracks, it tracks nothing at all, git printed something unreadable, or
    /// the ref walk for that repository failed outright. That is deliberate and it is only
    /// safe while the drawing is *negative* — all four earn no marker, which is the honest
    /// rendering of every one of them. A positive marker for this being `None` (a `✓`, an
    /// "up to date") would say the second case is the first, and so tell every brand-new
    /// local branch that it is in sync with an upstream it does not have. Tell them apart
    /// first if that is ever wanted.
    pub track: Option<Track>,
}

impl BranchEntry {
    /// git cannot find the ref this branch tracks — the ordinary end of a branch whose pull
    /// request was merged and whose head the remote then deleted.
    pub fn upstream_gone(&self) -> bool {
        self.track == Some(Track::Gone)
    }
}

/// The first herdr/git step picking a branch requires. Everything after it — moving the new
/// pane to the chosen destination — is the same whichever plan this is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchPlan {
    /// Already being worked on: go there.
    Focus { pane_id: String },
    /// The checkout is there; open it.
    Open { checkout_path: String },
    /// Cut a worktree. `base` is set only when the branch has to be created.
    Create {
        branch: String,
        base: Option<String>,
    },
    /// Fetch the branch first, so the worktree has a real base to be cut from.
    FetchThenCreate { branch: String, base: String },
}

/// Merge everything known about a repository's branches into one list.
///
/// `remote_heads` may be empty — it needs the network, and the picker shows local answers
/// immediately and folds the remote in when it arrives.
pub fn resolve(
    repo: &RepoNode,
    local_refs: &[GitRef],
    remote_heads: &[String],
    pull_requests: &[PullRequest],
) -> Vec<BranchEntry> {
    // Keyed by branch name so a branch that exists locally, on the remote, and as a
    // checkout collapses into one row rather than three.
    let mut entries: BTreeMap<&str, BranchEntry> = BTreeMap::new();

    for git_ref in local_refs {
        let entry = entries.entry(&git_ref.name).or_insert_with(|| BranchEntry {
            name: git_ref.name.clone(),
            state: BranchState::RemoteOnly,
            subject: None,
            committed_at: None,
            pull_request: None,
            track: None,
        });
        // A local ref beats a remote-only one; a remote ref only fills in missing detail.
        if git_ref.kind == RefKind::Local {
            entry.state = BranchState::LocalRef;
            entry.track = git_ref.track;
        }
        if entry.committed_at.is_none() || git_ref.kind == RefKind::Local {
            entry.committed_at = git_ref.committed_at.or(entry.committed_at);
            entry.subject = git_ref.subject.clone().or_else(|| entry.subject.clone());
        }
    }

    for name in remote_heads {
        entries.entry(name).or_insert_with(|| BranchEntry {
            name: name.clone(),
            state: BranchState::RemoteOnly,
            subject: None,
            committed_at: None,
            pull_request: None,
            track: None,
        });
    }

    // Checkouts override whatever the refs said: an open worktree is the strongest fact
    // about a branch.
    for worktree in &repo.worktrees {
        let Some(branch) = worktree.branch.as_deref() else {
            continue;
        };
        let entry = entries.entry(branch).or_insert_with(|| BranchEntry {
            name: branch.to_string(),
            state: BranchState::LocalRef,
            subject: None,
            committed_at: None,
            pull_request: None,
            track: None,
        });
        entry.state = match worktree.panes.first() {
            Some(pane) => BranchState::LivePane {
                pane_id: pane.pane_id.clone(),
                checkout_path: worktree.checkout_path.clone(),
            },
            None => BranchState::IdleWorktree {
                checkout_path: worktree.checkout_path.clone(),
            },
        };
    }

    for pull_request in pull_requests {
        if let Some(entry) = entries.get_mut(pull_request.head_ref.as_str()) {
            entry.pull_request = Some(pull_request.clone());
        }
    }

    let mut list: Vec<BranchEntry> = entries.into_values().collect();
    // The picker reorders this as the user asks; sorting here only decides what an
    // untouched list looks like.
    Order::default().sort(&mut list);
    list
}

/// The row offered when the typed name matches nothing: create it.
pub fn new_branch(name: &str) -> BranchEntry {
    BranchEntry {
        name: name.to_string(),
        state: BranchState::New,
        subject: None,
        committed_at: None,
        pull_request: None,
        track: None,
    }
}

/// What the picker was left holding when the user chose.
///
/// Two shapes rather than one, because the branch list can be asked for two different
/// things: the row itself, or a branch that does not exist yet cut from that row. The second
/// is deliberately not a [`BranchState`] — that enum says what a row *is*, and every renderer
/// matches on it, while this never appears as a row at all. See
/// `docs/adr/0013-two-ways-to-start-a-branch.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chosen {
    /// A branch already in the list.
    Listed(BranchEntry),
    /// A branch to create, cut from one that is in the list.
    NewFrom { name: String, base: BranchEntry },
}

impl Chosen {
    /// The branch being worked on, for anything that reports progress. For a new branch that
    /// is the name being created, not the one it is cut from.
    pub fn name(&self) -> &str {
        match self {
            Chosen::Listed(entry) => &entry.name,
            Chosen::NewFrom { name, .. } => name,
        }
    }
}

/// What picking this should do first.
///
/// `head_ref` is what a brand new branch is based on when nothing else names a base; `remote`
/// is the remote a never-fetched branch is fetched from.
pub fn plan(chosen: &Chosen, head_ref: &str, remote: &str) -> BranchPlan {
    let entry = match chosen {
        Chosen::Listed(entry) => entry,
        Chosen::NewFrom { name, base } => {
            return match &base.state {
                // The base has never been fetched, so there is no local ref to cut from.
                // Fetch it and cut from the remote-tracking ref — the same answer choosing
                // that row on its own would have given.
                BranchState::RemoteOnly => BranchPlan::FetchThenCreate {
                    branch: name.clone(),
                    base: format!("{remote}/{}", base.name),
                },
                // A base that does not exist yet cannot be cut from. The branch list refuses
                // to offer it; `head_ref` is the honest answer if it ever gets here.
                BranchState::New => BranchPlan::Create {
                    branch: name.clone(),
                    base: Some(head_ref.to_string()),
                },
                // Everything else names a ref git already has. Whether it is checked out, or
                // has an agent running in it, says nothing about cutting a branch from it.
                BranchState::LivePane { .. }
                | BranchState::IdleWorktree { .. }
                | BranchState::LocalRef => BranchPlan::Create {
                    branch: name.clone(),
                    base: Some(base.name.clone()),
                },
            };
        }
    };
    match &entry.state {
        BranchState::LivePane { pane_id, .. } => BranchPlan::Focus {
            pane_id: pane_id.clone(),
        },
        BranchState::IdleWorktree { checkout_path } => BranchPlan::Open {
            checkout_path: checkout_path.clone(),
        },
        // The branch already exists locally, so herdr checks it out; no base is involved.
        BranchState::LocalRef => BranchPlan::Create {
            branch: entry.name.clone(),
            base: None,
        },
        // Fetch into refs/remotes first, then cut the branch from the remote-tracking ref
        // rather than from HEAD, which is what makes this the branch GitHub has.
        BranchState::RemoteOnly => BranchPlan::FetchThenCreate {
            branch: entry.name.clone(),
            base: format!("{remote}/{}", entry.name),
        },
        BranchState::New => BranchPlan::Create {
            branch: entry.name.clone(),
            base: Some(head_ref.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{PaneNode, WorktreeNode};
    use crate::port::AgentStatus;
    use std::num::NonZeroU32;

    fn pane(id: &str) -> PaneNode {
        PaneNode {
            pane_id: id.into(),
            workspace_id: id.split(':').next().unwrap().into(),
            tab_id: format!("{}:t1", id.split(':').next().unwrap()),
            display_name: Some("claude".into()),
            agent_status: AgentStatus::Idle,
            focused: false,
        }
    }

    fn worktree(branch: &str, panes: Vec<PaneNode>) -> WorktreeNode {
        WorktreeNode {
            branch: Some(branch.into()),
            checkout_path: format!("/wt/{}", branch.replace('/', "-")),
            is_primary: branch == "main",
            open_workspace_id: panes.first().map(|p| p.workspace_id.clone()),
            track: None,
            panes,
        }
    }

    fn repo(worktrees: Vec<WorktreeNode>) -> RepoNode {
        RepoNode {
            repo_key: "/src/app/.git".into(),
            repo_root: "/src/app".into(),
            display_name: "me/app".into(),
            worktrees,
        }
    }

    fn local(name: &str, at: i64) -> GitRef {
        GitRef {
            name: name.into(),
            kind: RefKind::Local,
            committed_at: Some(at),
            subject: Some(format!("work on {name}")),
            track: None,
            worktree_path: None,
        }
    }

    fn remote_ref(name: &str, at: i64) -> GitRef {
        GitRef {
            kind: RefKind::Remote,
            ..local(name, at)
        }
    }

    #[test]
    fn a_local_branch_whose_upstream_is_gone_says_so() {
        // And a remote-only branch never can: there is nothing it was tracking that could
        // have been deleted.
        let mut local_ref = local("fix/crash", 20);
        local_ref.track = Some(Track::Gone);
        let entries = resolve(
            &repo(vec![]),
            &[local_ref, remote_ref("feat/search", 10)],
            &[],
            &[],
        );
        assert!(entry_named(&entries, "fix/crash").upstream_gone());
        assert!(!entry_named(&entries, "feat/search").upstream_gone());
    }

    #[test]
    fn a_branch_that_has_merely_moved_is_not_gone() {
        // `Track` grew from "gone or not" to four states, and `upstream_gone` is the
        // projection that has to stay narrow. Widened to "git said anything", every branch
        // that is simply ahead reads as `gone` — and `docs/adr/0011-what-may-be-swept.md`
        // marks on `gone`, so the sweep would offer to delete the one class of branch whose
        // commits exist nowhere else.
        let two = NonZeroU32::new(2).unwrap();
        let one = NonZeroU32::new(1).unwrap();
        for track in [
            Track::Ahead(two),
            Track::Behind(one),
            Track::Diverged {
                ahead: two,
                behind: one,
            },
        ] {
            let mut local_ref = local("fix/crash", 20);
            local_ref.track = Some(track);
            let entries = resolve(&repo(vec![]), &[local_ref], &[], &[]);
            assert!(
                !entry_named(&entries, "fix/crash").upstream_gone(),
                "{track:?} is a branch that moved, not one whose upstream went"
            );
        }
    }

    fn entry_named<'a>(entries: &'a [BranchEntry], name: &str) -> &'a BranchEntry {
        entries
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("{name} should be listed"))
    }

    fn state_of<'a>(entries: &'a [BranchEntry], name: &str) -> &'a BranchState {
        &entries
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("{name} should be listed"))
            .state
    }

    #[test]
    fn a_branch_with_a_pane_on_it_resolves_to_that_pane() {
        let entries = resolve(
            &repo(vec![worktree("feat/login", vec![pane("w2:p1")])]),
            &[local("feat/login", 100)],
            &[],
            &[],
        );
        assert_eq!(
            state_of(&entries, "feat/login"),
            &BranchState::LivePane {
                pane_id: "w2:p1".into(),
                checkout_path: "/wt/feat-login".into(),
            }
        );
    }

    #[test]
    fn a_checkout_with_no_pane_resolves_to_an_idle_worktree() {
        let entries = resolve(
            &repo(vec![worktree("fix/crash", vec![])]),
            &[local("fix/crash", 100)],
            &[],
            &[],
        );
        assert_eq!(
            state_of(&entries, "fix/crash"),
            &BranchState::IdleWorktree {
                checkout_path: "/wt/fix-crash".into()
            }
        );
    }

    #[test]
    fn a_local_ref_with_no_checkout_resolves_to_a_local_ref() {
        let entries = resolve(&repo(vec![]), &[local("chore/deps", 100)], &[], &[]);
        assert_eq!(state_of(&entries, "chore/deps"), &BranchState::LocalRef);
    }

    #[test]
    fn a_branch_only_on_the_remote_resolves_to_remote_only() {
        let entries = resolve(&repo(vec![]), &[], &["feat/search".into()], &[]);
        assert_eq!(state_of(&entries, "feat/search"), &BranchState::RemoteOnly);
    }

    #[test]
    fn a_branch_that_is_local_and_remote_collapses_into_one_local_row() {
        let entries = resolve(
            &repo(vec![]),
            &[local("main", 200), remote_ref("main", 150)],
            &["main".into()],
            &[],
        );
        assert_eq!(entries.len(), 1, "one row, not three");
        assert_eq!(state_of(&entries, "main"), &BranchState::LocalRef);
        assert_eq!(
            entries[0].committed_at,
            Some(200),
            "the local ref's date wins"
        );
    }

    #[test]
    fn a_fetched_remote_ref_alone_still_needs_fetching_before_it_can_be_cut() {
        // refs/remotes without refs/heads: known locally, but not a branch yet.
        let entries = resolve(&repo(vec![]), &[remote_ref("feat/api", 100)], &[], &[]);
        assert_eq!(state_of(&entries, "feat/api"), &BranchState::RemoteOnly);
        assert_eq!(
            entries[0].committed_at,
            Some(100),
            "its detail is still useful"
        );
    }

    #[test]
    fn pull_requests_annotate_branches_without_creating_them() {
        let pull_request = PullRequest {
            number: 123,
            title: "Add login".into(),
            head_ref: "feat/login".into(),
            is_draft: true,
        };
        let ghost = PullRequest {
            number: 999,
            title: "Deleted branch".into(),
            head_ref: "gone".into(),
            is_draft: false,
        };
        let entries = resolve(
            &repo(vec![]),
            &[local("feat/login", 100)],
            &[],
            &[pull_request.clone(), ghost],
        );
        assert_eq!(
            entries.len(),
            1,
            "a PR for a branch we do not have adds no row"
        );
        assert_eq!(entries[0].pull_request, Some(pull_request));
    }

    #[test]
    fn orders_work_in_progress_first_then_the_most_recent() {
        let entries = resolve(
            &repo(vec![
                worktree("feat/live", vec![pane("w2:p1")]),
                worktree("feat/idle", vec![]),
            ]),
            &[
                local("feat/live", 10),
                local("feat/idle", 20),
                local("old", 30),
                local("recent", 400),
            ],
            &["never-fetched".into()],
            &[],
        );
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["feat/live", "feat/idle", "recent", "old", "never-fetched"]
        );
    }

    #[test]
    fn picking_a_live_branch_goes_to_the_work_instead_of_copying_it() {
        let entry = &resolve(
            &repo(vec![worktree("feat/login", vec![pane("w2:p1")])]),
            &[local("feat/login", 100)],
            &[],
            &[],
        )[0];
        assert_eq!(
            plan(&Chosen::Listed(entry.clone()), "main", "origin"),
            BranchPlan::Focus {
                pane_id: "w2:p1".into()
            }
        );
    }

    #[test]
    fn picking_an_idle_checkout_opens_it() {
        let entry = &resolve(&repo(vec![worktree("fix/crash", vec![])]), &[], &[], &[])[0];
        assert_eq!(
            plan(&Chosen::Listed(entry.clone()), "main", "origin"),
            BranchPlan::Open {
                checkout_path: "/wt/fix-crash".into()
            }
        );
    }

    #[test]
    fn picking_a_local_ref_cuts_a_worktree_with_no_base() {
        let entry = &resolve(&repo(vec![]), &[local("chore/deps", 100)], &[], &[])[0];
        assert_eq!(
            plan(&Chosen::Listed(entry.clone()), "main", "origin"),
            BranchPlan::Create {
                branch: "chore/deps".into(),
                base: None,
            }
        );
    }

    #[test]
    fn picking_a_never_fetched_branch_fetches_it_and_bases_the_worktree_on_the_remote() {
        // Basing on HEAD instead would silently give the user an empty branch that merely
        // shares a name with the one on GitHub.
        let entry = &resolve(&repo(vec![]), &[], &["feat/search".into()], &[])[0];
        assert_eq!(
            plan(&Chosen::Listed(entry.clone()), "main", "origin"),
            BranchPlan::FetchThenCreate {
                branch: "feat/search".into(),
                base: "origin/feat/search".into(),
            }
        );
    }

    #[test]
    fn picking_a_name_that_exists_nowhere_creates_it_from_head() {
        assert_eq!(
            plan(
                &Chosen::Listed(new_branch("feat/brand-new")),
                "develop",
                "origin"
            ),
            BranchPlan::Create {
                branch: "feat/brand-new".into(),
                base: Some("develop".into()),
            }
        );
    }

    fn cut_from(base: &BranchEntry) -> BranchPlan {
        plan(
            &Chosen::NewFrom {
                name: "feat/new".into(),
                base: base.clone(),
            },
            "develop",
            "origin",
        )
    }

    #[test]
    fn a_new_branch_is_cut_from_the_local_ref_it_was_started_from() {
        let base = &resolve(&repo(vec![]), &[local("main", 100)], &[], &[])[0];
        assert_eq!(
            cut_from(base),
            BranchPlan::Create {
                branch: "feat/new".into(),
                base: Some("main".into()),
            }
        );
    }

    /// A checkout is a fact about where a branch is, not about whether it can be cut from.
    #[test]
    fn a_checked_out_base_is_still_cut_from_the_branch_rather_than_opened() {
        let base = &resolve(&repo(vec![worktree("fix/crash", vec![])]), &[], &[], &[])[0];
        assert_eq!(
            cut_from(base),
            BranchPlan::Create {
                branch: "feat/new".into(),
                base: Some("fix/crash".into()),
            }
        );
    }

    /// Choosing this row on its own goes to the pane. Starting a branch from it must not:
    /// the user asked for new work, not for the work that is already open.
    #[test]
    fn a_running_base_is_cut_from_rather_than_jumped_to() {
        let base = &resolve(
            &repo(vec![worktree("feat/login", vec![pane("w2:p1")])]),
            &[local("feat/login", 100)],
            &[],
            &[],
        )[0];
        assert_eq!(
            cut_from(base),
            BranchPlan::Create {
                branch: "feat/new".into(),
                base: Some("feat/login".into()),
            }
        );
    }

    /// The base has no local ref at all, so it is fetched first — and what is cut from is the
    /// remote-tracking ref, which is the branch the remote actually has.
    #[test]
    fn a_never_fetched_base_is_fetched_before_the_branch_is_cut_from_it() {
        let base = &resolve(&repo(vec![]), &[], &["feat/search".into()], &[])[0];
        assert_eq!(
            cut_from(base),
            BranchPlan::FetchThenCreate {
                branch: "feat/new".into(),
                base: "origin/feat/search".into(),
            }
        );
    }

    /// Unreachable through the branch list, which does not offer `n` on the create row. The
    /// answer is still an honest one rather than a ref git would reject.
    #[test]
    fn a_base_that_does_not_exist_yet_falls_back_to_head() {
        assert_eq!(
            cut_from(&new_branch("feat/not-yet")),
            BranchPlan::Create {
                branch: "feat/new".into(),
                base: Some("develop".into()),
            }
        );
    }

    #[test]
    fn a_detached_checkout_contributes_no_branch_row() {
        let detached = WorktreeNode {
            branch: None,
            ..worktree("main", vec![])
        };
        assert!(resolve(&repo(vec![detached]), &[], &[], &[]).is_empty());
    }
}
