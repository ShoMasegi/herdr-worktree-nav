//! What the plugin currently sees, as text.
//!
//! `herdr-worktree-nav dump` is for the moment the picker shows something surprising: it
//! separates "herdr or git told us something odd" from "the UI drew it wrong". So it prints
//! what the rows are drawn from, and the answers the rows deliberately leave out — that git
//! would not read a repository's refs, that a branch has no upstream to be level with, and why
//! git would not read a working tree. Outside a sweep a row draws nothing for the first two
//! and only `?` for the third, because no marker beats a wrong one
//! ([`domain::model::Refs`](crate::domain::model::Refs),
//! [`domain::model::WorkingTree`](crate::domain::model::WorkingTree)); in a sweep a row it
//! would have judged says `refs unreadable`, which is that something is missing rather than
//! what.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::adapter::plugin_config::Loaded;
use crate::app::one_line;
use crate::domain::chrome::Chrome;
use crate::domain::model::{normalize_path, Branch, Refs, RepoNode, Tree, WorktreeNode};
use crate::port::{GitPort, GitRef, RefKind, Snapshot, Track};

/// What the second ref walk — the one `main` makes for this page, for the upstream names —
/// said for each repository whose first walk the tree already carries as read: the refs, or
/// git's words when it would not say them again. A repository whose first walk failed is
/// absent here; the tree says so on its own line.
pub type RefsByRepo = BTreeMap<String, Result<Vec<GitRef>, String>>;

/// What git said about each working tree, by checkout path, or its words when it would not
/// say. The picker keeps only that it would not —
/// [`WorkingTree::Unreadable`](crate::domain::model::WorkingTree::Unreadable) — and the words
/// are the half a person troubleshooting needs.
pub type WorkingTrees = BTreeMap<String, Result<bool, String>>;

/// Ask git again for what the rows are drawn from, and for what they leave out.
///
/// A second `for-each-ref` per repository, for the upstream names:
/// [`collect::collect_tree`](crate::app::collect::collect_tree) keeps what the rows draw, and
/// the rows do not draw those. A repository whose first walk went wrong is not asked again —
/// the tree already carries git's words for it — and one that goes wrong the second time has
/// its words kept, so the page can say that this read failed where the tree's did not.
pub fn read_refs(git: &dyn GitPort, tree: &Tree) -> RefsByRepo {
    tree.repos
        .iter()
        .filter(|repo| repo.refs.is_read())
        .map(|repo| {
            let read = match git.local_refs(&repo.repo_root) {
                Err(error) => Err(one_line(&format!("{error:#}"))),
                // The same call the tree made, so the same rule: a walk missing a ref is
                // not a list of this repository's refs — `port::RefWalk`.
                Ok(walk) => match walk.dropped {
                    Some(words) => Err(one_line(&words)),
                    None => Ok(walk.refs),
                },
            };
            (repo.repo_root.clone(), read)
        })
        .collect()
}

/// Walk every checkout's working tree, one after another.
///
/// The picker keeps what git answered minus its words — clean, dirty, or would not say.
/// Here the words are kept: they are the half of the answer somebody troubleshooting is
/// here for.
pub fn read_working_trees(git: &dyn GitPort, tree: &Tree) -> WorkingTrees {
    tree.repos
        .iter()
        .flat_map(|repo| &repo.worktrees)
        .map(|worktree| {
            let answer = git
                .is_dirty(&worktree.checkout_path)
                .map_err(|error| one_line(&format!("{error:#}")));
            (worktree.checkout_path.clone(), answer)
        })
        .collect()
}

/// The report, one repository at a time, each checkout with a line of its own for what git
/// said about it.
pub fn report(
    snapshot: &Snapshot,
    chrome: &Chrome,
    plugin_config: &Loaded,
    tree: &Tree,
    refs: &RefsByRepo,
    working_trees: &WorkingTrees,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "herdr {} (protocol {})",
        snapshot.version, snapshot.protocol
    );
    let _ = writeln!(
        out,
        "chrome: accent {:?}, indicators {:?}",
        chrome.accent, chrome.indicators
    );
    match &plugin_config.path {
        Some(path) => {
            let _ = writeln!(out, "plugin config: {}", path.display());
        }
        None => {
            let _ = writeln!(out, "plugin config: missing, defaults");
        }
    }
    if let Some(complaint) = &plugin_config.complaint {
        let _ = writeln!(out, "plugin config problem: {complaint}");
    }
    let _ = writeln!(
        out,
        "[panes].show_worktrees_without_panes: {}",
        plugin_config.settings.panes.show_worktrees_without_panes
    );
    let _ = writeln!(
        out,
        "{} panes in {} repos",
        snapshot.panes.len(),
        tree.repos.len()
    );
    for repo in &tree.repos {
        let _ = writeln!(out, "\n{}  [{}]", repo.display_name, repo.repo_root);
        match (&repo.refs, refs.get(&repo.repo_root)) {
            (Refs::Unreadable(words), _) => {
                let _ = writeln!(out, "  refs unreadable: {words}");
            }
            (Refs::Read, Some(Err(words))) => {
                let _ = writeln!(out, "  refs unreadable on the second read: {words}");
            }
            (Refs::Read, Some(Ok(_)) | None) => {}
        }
        for worktree in &repo.worktrees {
            let open = match &worktree.open_workspace_id {
                Some(id) => format!(" open in {id}"),
                None => String::new(),
            };
            let _ = writeln!(
                out,
                "  {} {}{}  {}",
                if worktree.is_primary { "*" } else { "-" },
                worktree.label(),
                open,
                worktree.checkout_path
            );
            let _ = writeln!(
                out,
                "      {}  {}",
                branch_words(repo, worktree, refs),
                working_tree_words(working_trees.get(&worktree.checkout_path))
            );
            for pane in &worktree.panes {
                let _ = writeln!(
                    out,
                    "      {}  {:?}  {}",
                    pane.pane_id,
                    pane.agent_status,
                    pane.display_name.as_deref().unwrap_or("")
                );
            }
        }
    }
    if !tree.ungrouped.is_empty() {
        let _ = writeln!(out, "\nnot in any repository:");
        for pane in &tree.ungrouped {
            let _ = writeln!(out, "      {}", pane.pane_id);
        }
    }
    out
}

/// `upstream origin/x  track gone` — the two facts a marker is drawn from, in words the
/// marker cannot hold: `level` for a branch even with its upstream, `none` for one with no
/// upstream to be even with, `unreadable` for one whose `:track` field git printed and the
/// adapter could not read, and `not read` when git would not read the refs at all.
///
/// A branch with no upstream is measured against where it would push — see
/// [`adapter::git_cli`](crate::adapter::git_cli) — so `upstream none` can still be followed by
/// a marker. Under `push.default = current` a branch nobody has pushed gets a push destination
/// git calls `[gone]`, which the adapter drops rather than believe.
///
/// Where this page knows less than the picker it says which: `not read` for an upstream its
/// own walk could not get, beside the marker the picker is drawing or `not known`. When git
/// lists no ref at this checkout — its list and herdr's disagree about what is checked out
/// where — it says so rather than `upstream none`, and when git names more than one it names
/// each, because `domain::tree::tracks` declines to choose between them and this page has no
/// better grounds. Issue #48 is the labels.
fn branch_words(repo: &RepoNode, worktree: &WorktreeNode, refs: &RefsByRepo) -> String {
    let read = RefsRead::of(repo, refs);
    let Some(branch) = worktree.branch.name() else {
        return detached_words(worktree, read);
    };
    let tree_track = || match worktree.track {
        Some(track) => track_words(track),
        None => "not known".to_string(),
    };
    let read = match read {
        RefsRead::NotRead => return "upstream not read  track not read".to_string(),
        RefsRead::NotReadAgain => return format!("upstream not read  track {}", tree_track()),
        RefsRead::Read(read) => read,
    };
    // By the checkout git says has the branch, which is how the tree matched them too.
    let named = refs_at(read, &worktree.checkout_path);
    let git_ref = match named.as_slice() {
        [only] => only,
        [] => {
            return format!(
                "no ref at this checkout for {branch}  track {}",
                tree_track()
            )
        }
        more => {
            return format!(
                "more than one ref at this checkout: {}  track {}",
                each_of(more),
                tree_track()
            );
        }
    };
    let upstream = git_ref.upstream.as_deref();
    let track = standing(worktree.track, upstream);
    format!("upstream {}  track {track}", upstream.unwrap_or("none"))
}

/// Which of the two walks failed: under `NotRead` the picker has no markers either, and
/// under `NotReadAgain` — a second walk that failed or was never made — it has the ones this
/// page cannot name.
enum RefsRead<'a> {
    NotRead,
    NotReadAgain,
    Read(&'a [GitRef]),
}

impl<'a> RefsRead<'a> {
    fn of(repo: &RepoNode, refs: &'a RefsByRepo) -> Self {
        match (&repo.refs, refs.get(&repo.repo_root)) {
            (Refs::Unreadable(_), _) => Self::NotRead,
            (Refs::Read, Some(Err(_)) | None) => Self::NotReadAgain,
            (Refs::Read, Some(Ok(read))) => Self::Read(read),
        }
    }
}

/// The local refs git says have this checkout out.
///
/// By the checkout rather than by the branch name, which is how the tree matched them too.
/// `find` would take whichever came first and print an upstream the empty marker beside it
/// does not stand for. Local only, as `domain::tree::tracks` is.
fn refs_at<'a>(read: &'a [GitRef], checkout_path: &str) -> Vec<&'a GitRef> {
    read.iter()
        .filter(|git_ref| git_ref.kind == RefKind::Local)
        .filter(|git_ref| {
            git_ref
                .worktree_path
                .as_deref()
                .is_some_and(|path| normalize_path(path) == checkout_path)
        })
        .collect()
}

/// Each of them as `<branch> → <upstream> <where it stands>`, because the names alone are
/// bare words a reader has nothing to do with. Not a tell: which entry is stale is a fact
/// about git's worktree registrations, where an upstream went is a fact about the remote,
/// and nothing ties the two together. What this compact form loses is issue #48.
fn each_of(named: &[&GitRef]) -> String {
    named
        .iter()
        .map(|git_ref| {
            let upstream = git_ref.upstream.as_deref();
            format!(
                "{} \u{2192} {} {}",
                git_ref.name,
                upstream.unwrap_or("none"),
                standing(git_ref.track, upstream)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the page says about a row with no branch on it.
///
/// Two rows reach here and they are not the same news: the checkout herdr listed with
/// nothing out, and the one herdr never listed, which `build` makes for a pane and where a
/// branch may well be out. The row says which ([`Branch`]), so the page does too — on the
/// second, `no branch reported` would be this side's inference dressed as herdr's answer.
/// The refs git names at the path are still named and not explained, because which of them
/// is out is what nobody said (issue #49).
///
/// No `upstream …` either way: this row names no branch for one to be about.
fn detached_words(worktree: &WorktreeNode, read: RefsRead<'_>) -> String {
    let mut out = match worktree.branch {
        // Only a row naming no branch reaches here, `branch_words` having returned above
        // for the rest.
        Branch::NothingOut | Branch::Out(_) => "no branch reported".to_string(),
        Branch::NotSaid => "herdr did not list this checkout".to_string(),
    };
    match read {
        RefsRead::NotRead => out.push_str("  refs not read"),
        RefsRead::NotReadAgain => out.push_str("  refs not read on the second read"),
        RefsRead::Read(read) => {
            let named = refs_at(read, &worktree.checkout_path);
            if !named.is_empty() {
                let _ = write!(out, "  git names at this path: {}", each_of(&named));
            } else if worktree.track.is_some() {
                // This page's walk names no ref here and the picker's walk did: a track on
                // this row comes off a ref git named at this path, whether or not it is one
                // the row can draw. The two walks disagreeing is the fact worth printing.
                out.push_str("  no ref at this checkout");
            }
        }
    }
    if let Some(track) = worktree.track {
        let _ = write!(out, "  track {}", track_words(track));
    }
    out
}

/// The marks the list draws for a track, with no row in front of them.
///
/// The page's own copy rather than the picker's, because the picker's carry the gap that
/// separates them from a label and a page with no label has nothing to separate them from.
fn track_words(track: Track) -> String {
    match track {
        Track::Gone => "gone".to_string(),
        Track::Ahead(ahead) => format!("\u{2191}{ahead}"),
        Track::Behind(behind) => format!("\u{2193}{behind}"),
        Track::Diverged { ahead, behind } => format!("\u{2191}{ahead}\u{2193}{behind}"),
        // A word rather than nothing, because this page is the one surface with room for
        // it: the row draws the same blank a branch with nothing to report draws, and
        // somebody reading this page is reading it to find out which. `unreadable` is what
        // the page already calls a working tree git would not look at.
        Track::Unreadable => "unreadable".to_string(),
    }
}

/// The words for a track: the marker where git reported one, else `level` beside an
/// upstream and `none` without one.
///
/// Both of those are claims — even with the ref git named, and nothing to be even with —
/// so neither may stand for a reading that did not read. [`Track::Unreadable`] is not an
/// absence and does not arrive here as one.
fn standing(track: Option<Track>, upstream: Option<&str>) -> String {
    match track {
        Some(track) => track_words(track),
        None if upstream.is_some() => "level".to_string(),
        None => "none".to_string(),
    }
}

/// `working tree clean`, `dirty`, `unreadable: <git's words>`, or `not asked`.
fn working_tree_words(answer: Option<&Result<bool, String>>) -> String {
    match answer {
        Some(Ok(false)) => "working tree clean".to_string(),
        Some(Ok(true)) => "working tree dirty".to_string(),
        Some(Err(words)) => format!("working tree unreadable: {words}"),
        None => "working tree not asked".to_string(),
    }
}

#[cfg(test)]
mod tests;
