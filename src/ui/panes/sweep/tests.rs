use super::*;
use crate::domain::model::{Refs, RepoNode, Tree, Unlisted, WorkingTree, WorktreeNode};
use crate::domain::notice::Condition;
use crate::domain::sweep::{Half, Reason, Refusal};
use crate::port::{AgentStatus, PullRequestOutcome, SettledPullRequest, Track};
use crate::ui::panes::fixtures::*;
use ratatui::crossterm::event::KeyCode;

#[test]
fn the_sweep_opens_with_what_git_already_knew_marked() {
    let state = sweeping();
    assert!(state.is_sweeping());
    assert_eq!(
        mark_of(&state, "fix/crash"),
        Some(Mark::Going(Reason::Gone)),
        "clean, nothing running in it, and its upstream is gone"
    );
    assert_eq!(mark_of(&state, "feat/login"), Some(Mark::Staying));
    assert_eq!(
        mark_of(&state, "main"),
        Some(Mark::Refused(Refusal::Primary))
    );
    assert_eq!(
        mark_of(&state, "feat/wip"),
        Some(Mark::Refused(Refusal::Running)),
        "its upstream is gone too, and somebody is working in it"
    );
    assert_eq!(
        mark_of(&state, "claude"),
        None,
        "a pane is not a checkout and has nothing to sweep"
    );
    assert_eq!(state.chosen(), vec![at(&state, "/wt/app/fix-crash")]);
}

#[test]
fn a_working_tree_answering_clean_during_a_sweep_reaches_the_marks() {
    // `Clean` is the answer the sweep turns into a mark and the one answer no row draws,
    // so a rebuild keyed on the drawn markers never fires for it — and during a sweep
    // `/`, `r`, `b` and `d` are `Ignored`, so no other rebuild is in reach.
    let mut state = state();
    state.tree.repos[0].worktrees[1].panes.clear();
    state.tree.repos[0].worktrees[1].open_workspace_id = None;
    state.tree.repos[0].worktrees[2].track = Some(Track::Gone);
    state.replace_tree(state.tree.clone());
    state.handle_key(key(KeyCode::Char('S')));
    assert_eq!(
        state.marked_count(),
        0,
        "nobody has answered for anything yet"
    );

    state.set_working_trees(answers(&[("/wt/app/fix-crash", WorkingTree::Clean)]));
    assert_eq!(
        mark_of(&state, "fix/crash"),
        Some(Mark::Going(Reason::Gone)),
        "clean is what the sweep was waiting for"
    );
    assert_eq!(state.marked_count(), 1);
}

#[test]
fn space_adds_a_mark_and_takes_it_away_again() {
    let mut state = sweeping();
    select(&mut state, "feat/login");

    state.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(mark_of(&state, "feat/login"), Some(Mark::GoingByHand));
    assert_eq!(
        state.chosen(),
        vec![
            at(&state, "/wt/app/feat-login"),
            at(&state, "/wt/app/fix-crash")
        ]
    );

    state.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(mark_of(&state, "feat/login"), Some(Mark::Staying));
    assert_eq!(state.chosen(), vec![at(&state, "/wt/app/fix-crash")]);
}

#[test]
fn space_on_a_checkout_the_sweep_refuses_says_why_and_changes_nothing() {
    // The repository's own checkout, which `git worktree remove` will not take.
    let mut state = sweeping();
    select(&mut state, "main");

    state.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(state.message(), Some("the repository itself"));
    assert_eq!(
        mark_of(&state, "main"),
        Some(Mark::Refused(Refusal::Primary))
    );
    assert!(!state.chosen().contains(&at(&state, "/src/app")));
}

#[test]
fn leaving_the_sweep_forgets_what_was_marked_in_it() {
    // ADR 0011: nothing is deleted that was not on the screen with a mark against it.
    let mut state = sweeping();
    select(&mut state, "feat/login");
    state.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(state.chosen().len(), 2);

    assert_eq!(state.handle_key(key(KeyCode::Esc)), Action::Consumed);
    assert!(!state.is_sweeping());
    assert!(
        state.chosen().is_empty(),
        "and nothing is marked outside one"
    );
    assert_eq!(mark_of(&state, "fix/crash"), None);

    state.handle_key(key(KeyCode::Char('S')));
    assert_eq!(
        state.chosen(),
        vec![at(&state, "/wt/app/fix-crash")],
        "the next sweep opens on what it suggests, not on what the last was talked into"
    );
}

#[test]
fn a_sweep_judges_the_whole_list_because_it_opens_the_whole_list() {
    // `judge` walks the tree while the rows walk the filtered list, so a sweep entered
    // under a filter can mark checkouts nothing on screen mentions — ADR 0011.
    let mut state = sweeping();
    state.handle_key(key(KeyCode::Esc));
    state.handle_key(key(KeyCode::Char('/')));
    for character in "login".chars() {
        state.handle_key(key(KeyCode::Char(character)));
    }
    state.handle_key(key(KeyCode::Enter));
    assert!(
        !row_labels(&state).contains(&"fix/crash".to_string()),
        "the filter is on and fix/crash is off screen"
    );

    state.handle_key(key(KeyCode::Char('S')));
    assert_eq!(state.query(), "", "the sweep opens the list it is judging");
    assert!(row_labels(&state).contains(&"fix/crash".to_string()));
    assert_eq!(
        state.chosen(),
        vec![at(&state, "/wt/app/fix-crash")],
        "and what is marked is on the screen it was marked on"
    );
}

#[test]
fn a_state_filter_is_opened_by_a_sweep_too() {
    let mut state = sweeping();
    state.handle_key(key(KeyCode::Esc));
    state.handle_key(key(KeyCode::Char('w')));
    assert!(state.state_filter().is_some());

    state.handle_key(key(KeyCode::Char('S')));
    assert!(state.state_filter().is_none());
    assert!(row_labels(&state).contains(&"fix/crash".to_string()));
}

#[test]
fn the_cursor_stops_on_every_checkout_a_sweep_has_something_to_say_about() {
    // Outside a sweep the cursor steps over a checkout with panes in it. In a sweep the
    // checkout is the subject, so a cursor that cannot reach it makes its refusal
    // unreachable text.
    let mut state = sweeping();
    let mut reached = Vec::new();
    for _ in 0..state.lines().len() {
        if let Some(row) = state.rows().get(match state.lines()[state.cursor] {
            DisplayLine::Row(index) => index,
            DisplayLine::Spacer => continue,
        }) {
            reached.push(words::row_label(row));
        }
        state.handle_key(key(KeyCode::Char('j')));
    }
    for checkout in ["main", "feat/login", "fix/crash", "feat/wip"] {
        assert!(
            reached.contains(&checkout.to_string()),
            "{checkout} is a checkout the sweep has an answer for, and `j` never reached it"
        );
    }

    // And the refusal it was stepping over is now askable.
    select(&mut state, "feat/wip");
    state.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(state.message(), Some("panes are running in it"));
}

#[test]
fn q_out_of_a_sweep_puts_the_picker_back_rather_than_closing_it() {
    let mut state = sweeping();
    assert_eq!(state.handle_key(key(KeyCode::Char('q'))), Action::Consumed);
    assert!(!state.is_sweeping());
    assert_eq!(
        state.handle_key(key(KeyCode::Char('q'))),
        Action::Quit,
        "and the second one closes it"
    );
}

#[test]
fn every_way_out_of_a_sweep_is_a_way_out_of_a_sweep() {
    // The footer offers `shift+s done` beside `esc done`, so falling through to
    // `Ignored` is a picker that looks frozen on the key it just told you to press.
    for out in [KeyCode::Char('S'), KeyCode::Esc, KeyCode::Char('q')] {
        let mut state = sweeping();
        select(&mut state, "feat/login");
        state.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(state.chosen().len(), 2);

        assert_eq!(state.handle_key(key(out)), Action::Consumed, "{out:?}");
        assert!(!state.is_sweeping(), "{out:?} did not leave the sweep");
        assert!(state.chosen().is_empty(), "{out:?} kept the marks");
    }
}

#[test]
fn entering_a_sweep_from_a_search_leaves_the_cursor_on_what_was_searched_for() {
    // The sweep opens the whole list, so a line index from the filtered list points at a
    // different checkout. On a pane row `Space` is answered and says nothing, so the key
    // the user reaches for next does nothing at all.
    for query in ["crash", "wip", "login"] {
        let mut state = sweeping();
        state.handle_key(key(KeyCode::Esc));
        state.handle_key(key(KeyCode::Char('/')));
        for character in query.chars() {
            state.handle_key(key(KeyCode::Char(character)));
        }
        state.handle_key(key(KeyCode::Enter));
        let found = cursor_label(&state);

        state.handle_key(key(KeyCode::Char('S')));
        assert_eq!(
            cursor_label(&state),
            found,
            "/{query} then Shift-S moved the cursor off the row it was on"
        );
    }

    // And the row it stays on is the one that was searched for, not merely some row.
    let mut state = sweeping();
    state.handle_key(key(KeyCode::Esc));
    state.handle_key(key(KeyCode::Char('/')));
    for character in "crash".chars() {
        state.handle_key(key(KeyCode::Char(character)));
    }
    state.handle_key(key(KeyCode::Enter));
    state.handle_key(key(KeyCode::Char('S')));
    assert_eq!(cursor_label(&state), "fix/crash");
    assert_eq!(row_labels(&state).len(), 9, "on the whole list");
}

#[test]
fn entering_a_sweep_leaves_the_cursor_where_it_was() {
    // Nothing about the list changes except what each row says about itself, and where
    // the cursor is is the whole of what the user is deciding about.
    let mut state = sweeping();
    state.handle_key(key(KeyCode::Esc));
    select(&mut state, "fix/crash");
    let at = state.cursor;

    state.handle_key(key(KeyCode::Char('S')));
    assert_eq!(state.cursor, at);
    assert_eq!(cursor_label(&state), "fix/crash");
}

#[test]
fn a_removal_running_under_a_sweep_takes_the_checkout_out_of_it() {
    // Offering to delete a checkout that is already being deleted is the one refusal
    // about something happening right now rather than about what the checkout is.
    let mut state = sweeping();
    assert!(state.chosen().contains(&at(&state, "/wt/app/fix-crash")));

    state.set_removing(vec![CheckoutPath::for_test("/wt/app/fix-crash")]);
    assert_eq!(
        mark_of(&state, "fix/crash"),
        Some(Mark::Refused(Refusal::Removing))
    );
    assert!(!state.chosen().contains(&at(&state, "/wt/app/fix-crash")));

    // And when it ends without removing anything — git refused it, say — the row goes
    // back to being the sweep's to offer.
    state.set_removing(Vec::new());
    assert_eq!(
        mark_of(&state, "fix/crash"),
        Some(Mark::Going(Reason::Gone))
    );
}

#[test]
fn a_mark_does_not_move_to_whatever_is_at_that_path_next() {
    // A removal finishing re-reads the tree, and a second herdr session can have made a
    // worktree where the last one was.
    let mut state = sweeping();
    select(&mut state, "feat/login");
    state.handle_key(key(KeyCode::Char(' ')));
    assert!(state.chosen().contains(&at(&state, "/wt/app/feat-login")));

    let mut moved = state.tree.clone();
    moved.repos[0].worktrees[1].branch = Some("release/v2".into());
    state.replace_tree(moved);

    assert!(
        !state.chosen().contains(&at(&state, "/wt/app/feat-login")),
        "release/v2 has never been on the screen with a mark against it"
    );
    assert_eq!(mark_of(&state, "release/v2"), Some(Mark::Staying));
}

#[test]
fn a_tree_read_again_under_a_sweep_keeps_the_cursor_on_the_checkout_it_was_on() {
    // Put back by line index against a list one row shorter, the cursor lands on the
    // next checkout down, and the `Space` the user had lined up marks a checkout they
    // never pointed at. That is the list `Enter` deletes.
    let mut state = sweeping();
    let mut tree = state.tree().clone();
    tree.repos[0].worktrees[3].panes.clear();
    tree.repos[0].worktrees[3].open_workspace_id = None;
    state.replace_tree(tree);
    select(&mut state, "fix/crash");

    let mut shorter = state.tree().clone();
    shorter.repos[0].worktrees.remove(1);
    state.replace_tree(shorter);

    assert_eq!(cursor_label(&state), "fix/crash");
    state.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(
        mark_of(&state, "fix/crash"),
        Some(Mark::Staying),
        "the Space landed on the row the user was looking at"
    );
    assert!(
        state.chosen().contains(&at(&state, "/wt/app/feat-wip")),
        "and not on the one below it"
    );
}

#[test]
fn leaving_a_sweep_takes_the_cursor_off_a_checkout_it_may_no_longer_stop_on() {
    // Inside a sweep the cursor stops on a checkout with panes in it; outside it does
    // not, so a cursor put back there by name sits where the arrow keys can never take
    // it again.
    let mut state = sweeping();
    select(&mut state, "feat/wip");
    state.handle_key(key(KeyCode::Esc));

    assert!(!state.is_sweeping());
    let row = state.selected().expect("the cursor is on a row");
    assert!(row.is_selectable(), "on a row it may stop on");
    assert_eq!(
        row.name.as_deref(),
        Some("codex"),
        "the pane under the checkout, which is where to go"
    );
}

#[test]
fn a_tree_read_again_under_a_sweep_is_judged_again() {
    let mut state = sweeping();
    assert!(state.chosen().contains(&at(&state, "/wt/app/fix-crash")));

    // The upstream came back — somebody pushed the branch again.
    let mut tree = state.tree.clone();
    tree.repos[0].worktrees[2].track = None;
    state.replace_tree(tree);
    assert_eq!(mark_of(&state, "fix/crash"), Some(Mark::Staying));
    assert!(state.chosen().is_empty());
}

#[test]
fn space_on_a_row_that_is_not_a_checkout_is_answered_and_says_nothing() {
    // A repository heading, or a pane. `Ignored` would fall through to the loop as an
    // unhandled key, and a message would be noise on every stray press.
    let mut state = sweeping();
    select(&mut state, "claude");
    assert_eq!(state.handle_key(key(KeyCode::Char(' '))), Action::Consumed);
    assert_eq!(state.message(), None);
    assert_eq!(state.chosen(), vec![at(&state, "/wt/app/fix-crash")]);
}

#[test]
fn the_cursor_keys_go_the_way_they_are_drawn_during_a_sweep() {
    // `j` down and `k` up. Asserting only that the cursor moved and came back is
    // satisfied by a pair that both go the wrong way.
    let mut state = sweeping();
    select(&mut state, "main");
    state.handle_key(key(KeyCode::Char('j')));
    assert_eq!(cursor_label(&state), "claude", "j is down");
    state.handle_key(key(KeyCode::Char('k')));
    assert_eq!(cursor_label(&state), "main", "and k is back up");
}

#[test]
fn the_keys_that_act_on_one_row_are_not_answered_during_a_sweep() {
    for code in [
        KeyCode::Char('D'),
        KeyCode::Tab,
        KeyCode::Char('r'),
        KeyCode::Char('n'),
        KeyCode::Char('/'),
    ] {
        let mut state = sweeping();
        select(&mut state, "fix/crash");
        assert_eq!(
            state.handle_key(key(code)),
            Action::Ignored,
            "{code:?} means something outside a sweep and nothing in one"
        );
        assert!(state.is_sweeping(), "and it did not leave the sweep either");
    }
}

#[test]
fn the_cursor_still_walks_the_list_during_a_sweep() {
    let mut state = sweeping();
    let first = state.cursor;
    assert_eq!(state.handle_key(key(KeyCode::Char('j'))), Action::Consumed);
    assert_ne!(state.cursor, first);
    assert_eq!(state.handle_key(key(KeyCode::Char('k'))), Action::Consumed);
    assert_eq!(state.cursor, first);
}

#[test]
fn a_row_gh_could_not_judge_says_so_and_is_still_the_users_to_mark() {
    let mut state = sweeping();
    state.set_settled(
        BTreeMap::from([(RepoRoot::of(&state.tree.repos[0]), None)]),
        Some("gh could not be run: no such file or directory".to_string()),
        false,
    );

    assert_eq!(
        mark_of(&state, "feat/login"),
        Some(Mark::Unjudged(Half::PullRequests))
    );
    assert_eq!(
        mark_of(&state, "fix/crash"),
        Some(Mark::Going(Reason::Gone)),
        "gh may only widen a sweep: it never clears a mark git put there"
    );
    assert_eq!(
        state.sweep_trouble(),
        Some("gh could not be run: no such file or directory")
    );

    select(&mut state, "feat/login");
    state.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(
        mark_of(&state, "feat/login"),
        Some(Mark::GoingUnjudged(Half::PullRequests)),
        "not judged is not refused — and the row goes on saying it was not judged"
    );
}

#[test]
fn a_pull_request_gh_found_widens_the_sweep_and_says_which_one() {
    let mut state = sweeping();
    state.set_settled(
        BTreeMap::from([(
            RepoRoot::of(&state.tree.repos[0]),
            Some(SettledPullRequests::All(vec![SettledPullRequest {
                number: 42,
                head_ref: "feat/login".to_string(),
                from_a_fork: false,
                outcome: PullRequestOutcome::Merged,
            }])),
        )]),
        None,
        false,
    );

    assert_eq!(
        mark_of(&state, "feat/login"),
        Some(Mark::Going(Reason::PullRequest {
            number: 42,
            outcome: PullRequestOutcome::Merged,
        }))
    );
    assert_eq!(state.sweep_trouble(), None);
    assert_eq!(
        state.chosen(),
        vec![
            at(&state, "/wt/app/feat-login"),
            at(&state, "/wt/app/fix-crash")
        ]
    );
}

#[test]
fn gh_landing_does_not_move_the_cursor() {
    // It arrives on its own, after a frame the user is already reading: a cursor that
    // jumped would move the row under a `Space` about to be pressed.
    let mut state = sweeping();
    select(&mut state, "feat/login");
    let at = state.cursor;
    state.set_settled(
        BTreeMap::from([(RepoRoot::of(&state.tree.repos[0]), None)]),
        None,
        false,
    );
    assert_eq!(state.cursor, at);
}

#[test]
fn a_sweep_says_it_was_entered_once_and_then_stops_saying_so() {
    // The loop asks on every frame and answers a yes by asking `gh` again where it
    // refused, so a second yes is a `gh` call per frame for as long as it keeps
    // refusing.
    let mut state = state();
    assert!(!state.sweep_entered(), "no sweep, so nothing was entered");
    state.handle_key(key(KeyCode::Char('S')));
    assert!(state.sweep_entered());
    assert!(!state.sweep_entered(), "once per entry");
    state.handle_key(key(KeyCode::Esc));
    assert!(!state.sweep_entered(), "leaving is not entering");
    state.handle_key(key(KeyCode::Char('S')));
    assert!(state.sweep_entered(), "and once more on the next entry");
}

#[test]
fn git_answering_does_not_move_the_cursor_either() {
    // The working trees report in one at a time in the seconds after the picker opens.
    // Relisted from the top on each, the cursor is yanked back once per answer, with
    // `Shift-D` aimed at whatever is then beneath it.
    let mut state = state();
    select(&mut state, "fix/crash");
    let at = state.cursor;
    state.set_working_trees(answers(&[("/wt/app/feat-login", WorkingTree::Dirty)]));
    assert_eq!(state.cursor, at);
    assert_eq!(cursor_label(&state), "fix/crash");
}

#[test]
fn what_gh_says_outside_a_sweep_has_nowhere_to_go() {
    let mut state = state();
    state.set_settled(
        BTreeMap::from([(RepoRoot::of(&state.tree.repos[0]), None)]),
        Some("gh could not be run".to_string()),
        false,
    );
    assert_eq!(state.sweep_trouble(), None);
    assert!(state.chosen().is_empty());
}

#[test]
fn a_sweep_temporarily_shows_worktrees_without_panes() {
    let mut state = state();
    state.set_show_worktrees_without_panes(false);
    assert!(!row_labels(&state).contains(&"fix/crash".to_string()));

    state.handle_key(key(KeyCode::Char('S')));
    assert!(row_labels(&state).contains(&"fix/crash".to_string()));

    state.handle_key(key(KeyCode::Esc));
    assert!(!row_labels(&state).contains(&"fix/crash".to_string()));
}

#[test]
fn a_sweeps_question_takes_the_panel_off_the_screen() {
    // Both are drawn over the list, and the one that can delete something owns the
    // space: a box nobody saw is a box whose `y` is armed over nothing.
    let mut state = sweeping();
    state.set_stale("herdr did not answer".into());
    // The order a user reaches this in: `Enter` asks for the reading the question waits
    // on, and the panel is opened while that reading is still out.
    state.handle_key(key(KeyCode::Enter));
    assert_eq!(state.handle_key(key(KeyCode::Char('!'))), Action::Consumed);
    assert!(state.is_showing_conditions(), "a sweep can read it too");

    state.set_waiting(false);
    state.confirm_sweep_if_settled();
    assert!(state.pending_sweep().is_some(), "the question is up");
    assert!(!state.is_showing_conditions());
}

#[test]
fn enter_with_nothing_marked_says_so_and_asks_for_no_re_read() {
    let mut state = sweeping();
    select(&mut state, "fix/crash");
    state.handle_key(key(KeyCode::Char(' ')));
    assert!(state.chosen().is_empty());

    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::Consumed);
    assert_eq!(state.message(), Some("nothing is marked"));
    state.set_waiting(false);
    state.confirm_sweep_if_settled();
    assert!(state.pending_sweep().is_none());
}

#[test]
fn enter_asks_for_a_re_read_before_it_asks_anything() {
    let mut state = sweeping();
    assert_eq!(state.handle_key(key(KeyCode::Enter)), Action::SweepReload);
    assert!(
        state.pending_sweep().is_none(),
        "the box waits for the re-read"
    );
    assert!(state.is_sweeping());
}

#[test]
fn the_question_waits_for_the_walk_to_finish() {
    let mut state = sweeping();
    state.handle_key(key(KeyCode::Enter));
    state.set_waiting(true);
    state.confirm_sweep_if_settled();
    assert!(state.pending_sweep().is_none(), "the walk is still out");

    state.set_waiting(false);
    state.confirm_sweep_if_settled();
    assert_eq!(box_labels(&state), ["fix/crash"]);
}

#[test]
fn a_row_the_re_read_added_is_in_the_box() {
    let mut state = state();
    state.tree.repos[0].worktrees[1].panes.clear();
    state.tree.repos[0].worktrees[1].open_workspace_id = None;
    state.tree.repos[0].worktrees[2].track = Some(Track::Gone);
    state.replace_tree(state.tree.clone());
    state.set_working_trees(answers(&[("/wt/app/feat-login", WorkingTree::Clean)]));
    state.handle_key(key(KeyCode::Char('S')));
    select(&mut state, "feat/login");
    state.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(state.chosen(), vec![at(&state, "/wt/app/feat-login")]);

    state.handle_key(key(KeyCode::Enter));
    state.set_working_trees(answers(&[
        ("/wt/app/feat-login", WorkingTree::Clean),
        ("/wt/app/fix-crash", WorkingTree::Clean),
    ]));
    state.set_waiting(false);
    state.confirm_sweep_if_settled();

    assert_eq!(box_labels(&state), ["feat/login", "fix/crash"]);
    assert_eq!(
        state.message(),
        None,
        "nothing was dropped, so nothing is said"
    );
}

#[test]
fn y_removes_exactly_what_the_box_listed_and_leaves_the_sweep() {
    let mut state = sweeping();
    ask(&mut state);
    assert_eq!(box_labels(&state), ["fix/crash"]);

    let Action::RemoveWorktrees(sweep) = state.handle_key(key(KeyCode::Char('y'))) else {
        panic!("`y` is the answer that goes ahead");
    };
    let paths: Vec<&str> = sweep
        .removals()
        .iter()
        .map(|r| r.checkout_path().as_str())
        .collect();
    assert_eq!(paths, ["/wt/app/fix-crash"]);
    assert!(
        !state.is_sweeping(),
        "the sweep is over once it is answered"
    );
    assert!(state.pending_sweep().is_none());
    assert!(state.chosen().is_empty(), "and the marks went with it");
}

#[test]
fn any_other_key_takes_the_sweeps_question_back() {
    for code in [
        KeyCode::Char('n'),
        KeyCode::Esc,
        KeyCode::Enter,
        KeyCode::Char('D'),
        KeyCode::Down,
        KeyCode::Char('Y'),
        KeyCode::Char(' '),
    ] {
        let mut state = sweeping();
        ask(&mut state);
        assert_eq!(state.handle_key(key(code)), Action::Consumed, "{code:?}");
        assert!(state.pending_sweep().is_none(), "{code:?}");
        assert!(
            state.is_sweeping(),
            "{code:?}: the sweep stays; its question went"
        );
        assert_eq!(
            state.chosen(),
            vec![at(&state, "/wt/app/fix-crash")],
            "{code:?}: and the marks stay with it"
        );
    }
}

#[test]
fn a_tree_that_changes_under_the_sweeps_question_takes_it_back() {
    let mut state = sweeping();
    ask(&mut state);
    assert!(state.pending_sweep().is_some());

    state.replace_tree(state.tree.clone());
    assert!(state.pending_sweep().is_none());
    assert_eq!(
        state.message(),
        Some("the list changed while that was up — ask again")
    );
    assert_eq!(state.handle_key(key(KeyCode::Char('y'))), Action::Ignored);

    // The re-read `Enter` asks for replaces the tree too, and that is not a change under
    // the question: it comes before it.
    let mut state = sweeping();
    state.handle_key(key(KeyCode::Enter));
    state.replace_tree(state.tree.clone());
    state.set_waiting(false);
    state.confirm_sweep_if_settled();
    assert_eq!(box_labels(&state), ["fix/crash"]);
}

#[test]
fn only_a_swept_checkout_with_a_branch_deletes_one() {
    // A checkout with nothing out: the sweep offers it nothing, the user marks it by
    // hand, and its label is a directory name that must never reach `git branch -d`.
    let mut state = sweeping();
    state.tree.repos[0].worktrees.push(WorktreeNode {
        branch: None,
        checkout_path: "/wt/app/scratch".into(),
        is_primary: false,
        open_workspace_id: None,
        track: None,
        panes: vec![],
    });
    state.replace_tree(state.tree.clone());
    state.set_working_trees(answers(&[
        ("/src/app", WorkingTree::Clean),
        ("/wt/app/feat-login", WorkingTree::Clean),
        ("/wt/app/fix-crash", WorkingTree::Clean),
        ("/wt/app/feat-wip", WorkingTree::Clean),
        ("/wt/app/scratch", WorkingTree::Clean),
    ]));
    select(&mut state, "scratch");
    state.handle_key(key(KeyCode::Char(' ')));
    ask(&mut state);

    let sweep = state.pending_sweep().expect("a question should be up");
    let deleting: Vec<(&str, bool)> = sweep
        .removals()
        .iter()
        .map(|r| (r.label(), r.delete_branch()))
        .collect();
    assert_eq!(deleting, [("fix/crash", true), ("scratch", false)]);
    assert_eq!(sweep.branches(), 1);
}

#[test]
fn nothing_left_after_the_re_read_asks_nothing() {
    let mut state = sweeping();
    state.handle_key(key(KeyCode::Enter));
    state.set_working_trees(answers(&[
        ("/src/app", WorkingTree::Clean),
        ("/wt/app/feat-login", WorkingTree::Clean),
        ("/wt/app/fix-crash", WorkingTree::Dirty),
        ("/wt/app/feat-wip", WorkingTree::Clean),
    ]));
    state.set_waiting(false);
    state.confirm_sweep_if_settled();

    assert!(state.pending_sweep().is_none());
    assert_eq!(
        state.message(),
        Some("no longer marked: fix/crash — nothing left to remove")
    );
    assert!(state.is_sweeping(), "the sweep is still there to mark in");
}

#[test]
fn a_repository_the_re_read_could_not_list_is_named_rather_than_nothing_left() {
    // The rows did not go because the checkouts went: they went because herdr would not
    // say what was in the repository, and every mark in it came off the box at once.
    // Told `nothing left to remove`, the reader reads a sweep that found nothing — with
    // herdr's words nowhere on screen. Issue #56.
    let mut state = sweeping();
    state.handle_key(key(KeyCode::Enter));
    let mut unlisted = Tree {
        repos: Vec::new(),
        ..state.tree.clone()
    };
    unlisted.trouble.unlisted.push(Unlisted {
        repo_key: "/src/app/.git".into(),
        words: "herdr rejected worktree.list: internal error".into(),
        panes: Default::default(),
    });
    state.replace_tree(unlisted);
    state.set_waiting(false);
    state.confirm_sweep_if_settled();

    assert!(
        state.pending_sweep().is_none(),
        "nothing is left to ask about"
    );
    assert_eq!(
        state.message(),
        Some(
            "no longer marked: /wt/app/fix-crash — app: not listed: herdr rejected \
             worktree.list: internal error"
        )
    );

    // And only a listing that failed *in the repository these rows were in*. Another
    // repository herdr would not list is on the prompt line already and took none of these
    // away, so naming it here would answer with someone else's trouble.
    let mut state = sweeping();
    state.handle_key(key(KeyCode::Enter));
    let mut elsewhere = state.tree.clone();
    elsewhere.repos[0].worktrees.clear();
    elsewhere.trouble.unlisted.push(Unlisted {
        repo_key: "/src/other/.git".into(),
        words: "herdr rejected worktree.list: internal error".into(),
        panes: Default::default(),
    });
    state.replace_tree(elsewhere);
    state.set_waiting(false);
    state.confirm_sweep_if_settled();
    assert_eq!(
        state.message(),
        Some("no longer marked: /wt/app/fix-crash — nothing left to remove"),
        "the row was in me/app, and me/app was listed"
    );

    // And refs that went unread are a condition
    // too, and they take no rows away, so naming them as the reason would name the wrong one.
    let mut state = sweeping();
    state.handle_key(key(KeyCode::Enter));
    let mut unread = state.tree.clone();
    unread.repos[0].refs = Refs::Unreadable("fatal: bad ref".into());
    unread.repos[0].worktrees.clear();
    state.replace_tree(unread);
    state.set_waiting(false);
    state.confirm_sweep_if_settled();
    assert!(
        state
            .conditions()
            .iter()
            .any(|condition| matches!(condition, Condition::RefsUnreadable { .. })),
        "there is another condition to be named by mistake"
    );
    assert_eq!(
        state.message(),
        Some("no longer marked: /wt/app/fix-crash — nothing left to remove")
    );
}

#[test]
fn a_question_a_key_took_back_does_not_come_back_on_the_next_frame() {
    // The loop asks `confirm_sweep_if_settled` on every frame, and a no is a no.
    for code in [KeyCode::Esc, KeyCode::Char('n'), KeyCode::Down] {
        let mut state = sweeping();
        ask(&mut state);
        assert_eq!(state.handle_key(key(code)), Action::Consumed, "{code:?}");
        state.set_waiting(false);
        state.confirm_sweep_if_settled();
        assert!(
            state.pending_sweep().is_none(),
            "{code:?}: the question came back"
        );
    }
}

#[test]
fn a_row_dropped_by_the_re_read_is_named_by_the_repository_its_key_names() {
    // Two repositories list one path, and the re-read finds a pane in the second's
    // checkout. The directory is running, so both rows go — each named by the row its own
    // key finds, which is `fix/crash` for one and `chore/deps` for the other.
    let mut state = sweeping();
    let mut tree = state.tree.clone();
    tree.repos.push(RepoNode {
        repo_key: "/src/old/.git".into(),
        repo_root: "/src/old".into(),
        display_name: "me/old".into(),
        refs: Refs::Read,
        worktrees: vec![WorktreeNode {
            branch: Some("chore/deps".into()),
            checkout_path: "/wt/app/fix-crash".into(),
            is_primary: false,
            open_workspace_id: None,
            track: Some(Track::Gone),
            panes: vec![],
        }],
    });
    state.replace_tree(tree);
    assert_eq!(state.chosen().len(), 2, "both rows at the path are offered");

    state.handle_key(key(KeyCode::Enter));
    let mut re_read = state.tree.clone();
    re_read.repos[1].worktrees[0]
        .panes
        .push(pane("w5:p1", "codex", AgentStatus::Working));
    state.replace_tree(re_read);
    state.set_waiting(false);
    state.confirm_sweep_if_settled();

    assert_eq!(
        state.message(),
        Some("no longer marked: fix/crash, chore/deps — nothing left to remove")
    );
    assert!(box_labels(&state).is_empty());
}

/// A sweep with a mark in each of two repositories — `me/app`'s `fix/crash` and `me/old`'s
/// `chore/deps`, which a stale registration puts at the same path — after `Enter`.
fn two_marks_asked() -> PanesState {
    let mut state = sweeping();
    let mut tree = state.tree.clone();
    tree.repos.push(RepoNode {
        repo_key: "/src/old/.git".into(),
        repo_root: "/src/old".into(),
        display_name: "me/old".into(),
        refs: Refs::Read,
        worktrees: vec![WorktreeNode {
            branch: Some("chore/deps".into()),
            checkout_path: "/wt/app/fix-crash".into(),
            is_primary: false,
            open_workspace_id: None,
            track: Some(Track::Gone),
            panes: vec![],
        }],
    });
    state.replace_tree(tree);
    assert_eq!(state.chosen().len(), 2);
    state.handle_key(key(KeyCode::Enter));
    state
}

/// `me/app` as herdr would not list it, on the re-read.
fn app_unlisted(tree: &Tree) -> Tree {
    let mut tree = tree.clone();
    tree.repos.retain(|repo| repo.repo_key != "/src/app/.git");
    tree.trouble.unlisted.push(Unlisted {
        repo_key: "/src/app/.git".into(),
        words: "herdr rejected worktree.list: internal error".into(),
        panes: Default::default(),
    });
    tree
}

#[test]
fn the_reason_goes_with_the_rows_it_took_and_no_others() {
    // `me/app` is lost and `me/old`'s mark survives: the reason is about the row that went,
    // and the question still stands for the one that did not.
    let mut state = two_marks_asked();
    state.replace_tree(app_unlisted(&state.tree));
    state.set_waiting(false);
    state.confirm_sweep_if_settled();
    assert_eq!(
        state.message(),
        Some(
            "no longer marked: /wt/app/fix-crash — app: not listed: herdr rejected \
             worktree.list: internal error"
        )
    );
    assert_eq!(box_labels(&state), ["chore/deps"]);

    // `me/app` is lost, and `me/old`'s row went too — for its own reason, a pane that
    // opened in it, which its row shows. One `— app: not listed` after both would read as
    // the reason for both, so neither is given one here: the listing that failed is a
    // condition already.
    let mut state = two_marks_asked();
    let mut re_read = app_unlisted(&state.tree);
    re_read.repos[0].worktrees[0]
        .panes
        .push(pane("w5:p1", "codex", AgentStatus::Working));
    state.replace_tree(re_read);
    state.set_waiting(false);
    state.confirm_sweep_if_settled();
    assert_eq!(
        state.message(),
        Some("no longer marked: /wt/app/fix-crash, chore/deps — nothing left to remove")
    );

    // The same, with `me/app` entered twice. Two entries for one repository are not a second
    // repository accounted for.
    let mut state = two_marks_asked();
    let mut re_read = app_unlisted(&state.tree);
    re_read.repos[0].worktrees[0]
        .panes
        .push(pane("w5:p1", "codex", AgentStatus::Working));
    let twice = re_read.trouble.unlisted[0].clone();
    re_read.trouble.unlisted.push(twice);
    state.replace_tree(re_read);
    state.set_waiting(false);
    state.confirm_sweep_if_settled();
    assert_eq!(
        state.message(),
        Some("no longer marked: /wt/app/fix-crash, chore/deps — nothing left to remove")
    );

    // Both lost: both are the reason, the first said and the other counted.
    let mut state = two_marks_asked();
    let mut re_read = app_unlisted(&state.tree);
    re_read.repos.clear();
    re_read.trouble.unlisted.push(Unlisted {
        repo_key: "/src/old/.git".into(),
        words: "herdr rejected worktree.list: timeout".into(),
        panes: Default::default(),
    });
    state.replace_tree(re_read);
    state.set_waiting(false);
    state.confirm_sweep_if_settled();
    assert_eq!(
        state.message(),
        Some(
            "no longer marked: /wt/app/fix-crash, /wt/app/fix-crash — app: not listed: herdr \
             rejected worktree.list: internal error (+1 more)"
        )
    );
}

#[test]
fn a_checkout_that_left_the_tree_during_the_re_read_is_named_by_its_path() {
    // Another session removed it between `Shift-S` and `Enter`, so there is no row left
    // to take a label from.
    let mut state = sweeping();
    state.handle_key(key(KeyCode::Enter));
    let mut without = state.tree.clone();
    without.repos[0].worktrees.remove(2);
    state.replace_tree(without);
    state.set_waiting(false);
    state.confirm_sweep_if_settled();

    assert!(state.pending_sweep().is_none());
    assert_eq!(
        state.message(),
        Some("no longer marked: /wt/app/fix-crash — nothing left to remove")
    );
}

#[test]
fn a_message_already_on_the_prompt_line_survives_a_re_read_that_dropped_nothing() {
    // A removal started earlier can report a refusal while the re-read is out.
    let mut state = sweeping();
    state.handle_key(key(KeyCode::Enter));
    state.set_message("could not remove feat/wip: fatal: refused".into());
    state.set_waiting(false);
    state.confirm_sweep_if_settled();

    assert_eq!(box_labels(&state), ["fix/crash"]);
    assert_eq!(
        state.message(),
        Some("could not remove feat/wip: fatal: refused")
    );
}

#[test]
fn every_row_the_re_read_dropped_is_named_in_path_order() {
    // Two marks overruled by one re-read — a pane opened in one checkout, work written
    // into the other — and both named, so neither loss is silent.
    let mut state = sweeping();
    select(&mut state, "feat/login");
    state.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(state.chosen().len(), 2);

    state.handle_key(key(KeyCode::Enter));
    let mut busy = state.tree.clone();
    busy.repos[0].worktrees[1]
        .panes
        .push(pane("w2:p1", "claude", AgentStatus::Working));
    state.replace_tree(busy);
    state.set_working_trees(answers(&[("/wt/app/fix-crash", WorkingTree::Dirty)]));
    state.set_waiting(false);
    state.confirm_sweep_if_settled();

    assert!(state.pending_sweep().is_none());
    assert_eq!(
        state.message(),
        Some("no longer marked: feat/login, fix/crash — nothing left to remove")
    );
}
