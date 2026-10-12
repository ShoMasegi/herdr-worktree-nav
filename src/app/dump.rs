//! What the plugin currently sees, as text.
//!
//! `herdr-worktree-nav dump` is for the moment the picker shows something surprising: it
//! separates "herdr or git told us something odd" from "the UI drew it wrong". So it prints
//! what the rows are drawn from, and the answers the rows deliberately leave out — that git
//! would not read a repository's refs, that a branch has no upstream to be level with, and why
//! git would not read a working tree. Outside a sweep a row draws nothing for the first two
//! and only `?` for the third, because no marker beats a wrong one
//! ([`domain::model::Refs`](crate::domain::model::Refs),
//! [`domain::model::WorkingTree`](crate::domain::model::WorkingTree)); a sweep refuses every
//! row of a repository whose refs git would not read, and `Space` says so, which is that
//! something is missing rather than what.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::adapter::plugin_config::Loaded;
use crate::app::one_line;
use crate::domain::chrome::Chrome;
use crate::domain::model::{normalize_path, Branch, Position, Refs, RepoNode, Tree, WorktreeNode};
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
    if !tree.trouble.unlisted.is_empty() {
        // A repository with no section above, because herdr would not say what is in it.
        let _ = writeln!(out, "\nnot listed:");
        for repo in &tree.trouble.unlisted {
            let _ = writeln!(out, "  {}  [{}]", repo.name(), repo.repo_key);
            let _ = writeln!(out, "      {}", repo.words);
        }
    }
    if !tree.ungrouped.is_empty() {
        let _ = writeln!(out, "\nnot in any repository:");
        for pane in &tree.ungrouped {
            let _ = writeln!(out, "      {}", pane.pane_id);
            // Why, where there is a why. herdr not seeing into the pane, git not answering
            // about it, and herdr not listing the repository it was placed in all look the
            // same on this page otherwise, and the page exists to tell those apart.
            if let Some(words) = tree.trouble.unplaced.get(&pane.pane_id) {
                let _ = writeln!(out, "          {words}");
            } else if let Some(repo) = tree
                .trouble
                .unlisted
                .iter()
                .find(|repo| repo.panes.contains_key(&pane.pane_id))
            {
                let _ = writeln!(out, "          in {}, which is not listed", repo.name());
            }
        }
    }
    out
}

/// `upstream origin/x  track gone` — the two facts a marker is drawn from, each in a word
/// of its own: `level` for a branch even with its upstream, `none` for one with no upstream
/// to be even with, `unreadable` for one whose `:track` field git printed and the adapter
/// could not read, `contested` for a checkout more than one of the repository's refs names,
/// and `not read` when git would not read the refs at all — and, where this page has no
/// upstream to say `level` or `none` beside, `nothing reported` and `not known` ([`found`]).
///
/// A branch with no upstream is measured against where it would push — see
/// [`adapter::git_cli`](crate::adapter::git_cli) — so `upstream none` can still be followed by
/// a marker. Under `push.default = current` a branch nobody has pushed gets a push destination
/// git calls `[gone]`, which the adapter drops rather than believe.
///
/// Where this page knows less than the picker it says which: `not read` for an upstream its
/// own walk could not get, beside the marker the picker is drawing or `not known`. When git
/// lists no ref at this checkout — a branch with no commit yet, or git's list and herdr's
/// disagreeing about what is out where — it says so rather than `upstream none`, and when
/// git names more than one it names
/// each, because `domain::tree::tracks` declines to choose between them. Issue #48 is the
/// labels.
///
/// How many refs name the checkout is the row's answer, read off its [`Position`] rather
/// than counted again here (#47). This page's own walk is a second one, and where the two
/// counts differ the line says both rather than letting either stand for the other.
fn branch_words(repo: &RepoNode, worktree: &WorktreeNode, refs: &RefsByRepo) -> String {
    let read = RefsRead::of(repo, refs);
    let Branch::Out { name, position } = &worktree.branch else {
        return detached_words(worktree, read);
    };
    let found = found(*position);
    let read = match read {
        RefsRead::NotRead => return "upstream not read  track not read".to_string(),
        RefsRead::NotReadAgain => return format!("upstream not read  track {found}"),
        RefsRead::Read(read) => read,
    };
    // By the checkout git says has the branch, which is how the tree matched them too.
    let named = refs_at(read, &worktree.checkout_path);
    match (
        Count::of_position(*position),
        Count::of(&named),
        named.as_slice(),
    ) {
        (Count::None, Count::None, _) => {
            format!("no ref at this checkout for {name}  track {found}")
        }
        (Count::One, Count::One, [only]) => {
            let upstream = only.upstream.as_deref();
            let track = standing(*position, upstream);
            format!("upstream {}  track {track}", upstream.unwrap_or("none"))
        }
        (Count::More, Count::More, more) => {
            format!(
                "more than one ref at this checkout: {}  track {found}",
                each_of(more)
            )
        }
        (picker, _, []) => format!(
            "the picker's read named {} at this checkout and this one names none  track {found}",
            picker.words(),
        ),
        (picker, _, named) => format!(
            "the picker's read named {} at this checkout and this one names {}  track {found}",
            picker.words(),
            each_of(named),
        ),
    }
}

/// How many refs named a checkout: the three answers `domain::tree::tracks` gives, so that
/// the picker's walk and this page's can be held side by side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Count {
    None,
    One,
    More,
}

impl Count {
    /// The picker's count, as the row carries it. A position is only ever said off one ref;
    /// [`Position::NotSaid`] reaches here only under refs git read, so it is no ref at all.
    fn of_position(position: Position) -> Self {
        match position {
            Position::Said(_) => Count::One,
            Position::Contested => Count::More,
            Position::NotSaid => Count::None,
        }
    }

    /// This page's own count.
    fn of(named: &[&GitRef]) -> Self {
        match named {
            [] => Count::None,
            [_] => Count::One,
            _ => Count::More,
        }
    }

    fn words(self) -> &'static str {
        match self {
            Count::None => "no ref",
            Count::One => "one ref",
            Count::More => "more than one ref",
        }
    }
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
                standing(Position::Said(git_ref.track), upstream)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the page says about a row with no branch on it.
///
/// Two rows reach here and they are not the same news: the checkout herdr listed with
/// nothing out, and the one herdr never listed, which `build` makes for a pane and where a
/// branch may well be out. The row says which ([`Branch`]), so the page does too: on the
/// second, `no branch reported` would be this side's inference dressed as herdr's answer.
/// Neither carries a position — [`Branch`] keeps one only on the answer that names a
/// branch — so the refs git names at the path are named and not explained (issue #49).
///
/// No `upstream …` either way: this row names no branch for one to be about.
fn detached_words(worktree: &WorktreeNode, read: RefsRead<'_>) -> String {
    let mut out = match worktree.branch {
        // Only a row naming no branch reaches here, `branch_words` having returned above
        // for the rest.
        Branch::NothingOut | Branch::Out { .. } => "no branch reported".to_string(),
        Branch::NotSaid => "herdr did not list this checkout".to_string(),
    };
    match read {
        RefsRead::NotRead => out.push_str("  refs not read"),
        RefsRead::NotReadAgain => out.push_str("  refs not read on the second read"),
        RefsRead::Read(read) => {
            let named = refs_at(read, &worktree.checkout_path);
            if !named.is_empty() {
                let _ = write!(out, "  git names at this path: {}", each_of(&named));
            }
        }
    }
    out
}

/// What the picker's own ref walk found for this checkout, where this page has no upstream
/// of its own to measure it against.
///
/// `level` and `none` are not among these: both need an upstream to be a claim about, and
/// where one is known [`standing`] says them. What is left is the marker, `nothing
/// reported` for the walk having found one ref and git having reported nothing about it,
/// `unreadable` for [`Track::Unreadable`], `contested` for [`Position::Contested`], and
/// `not known`, which is the walk having found no ref at this path at all.
fn found(position: Position) -> String {
    match position {
        Position::Said(Some(track)) => track_words(track),
        Position::Said(None) => "nothing reported".to_string(),
        Position::Contested => "contested".to_string(),
        Position::NotSaid => "not known".to_string(),
    }
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

/// The words for a position beside an upstream this page knows: the marker where git
/// reported one, else `level` with an upstream and `none` without one.
///
/// Both of those are claims — even with the ref git named, and nothing to be even with — so
/// neither may stand for a reading that did not read. [`Track::Unreadable`] is not an
/// absence and does not arrive here as one, and neither is [`Position::Contested`]: `level`
/// is exactly the claim `domain::tree::tracks` refused to make there, and this page has no
/// better grounds for making it.
fn standing(position: Position, upstream: Option<&str>) -> String {
    match position {
        Position::Said(Some(track)) => track_words(track),
        Position::Said(None) if upstream.is_some() => "level".to_string(),
        Position::Said(None) => "none".to_string(),
        Position::Contested | Position::NotSaid => found(position),
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
