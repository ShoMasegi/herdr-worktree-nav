//! The branches picker, drawn: the four steps it walks and the chrome around them.
//!
//! [`draw`] is which step is on screen; the steps themselves are two modules. [`lists`] is
//! the two lists the picker starts with — the repositories and their branches, each with its
//! search line — and [`destination`] is what happens after one is chosen: naming a new
//! branch, and picking where its pane lands, with the diagram that shows it.

use super::*;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::domain::model::RepoNode;
use crate::domain::order::Order;
use crate::domain::preview::{Preview, PreviewPane};
use crate::domain::resolve::{BranchEntry, BranchState};
use crate::port::LayoutRect;
use crate::ui::branches::{Activity, BranchesState, Step};
use crate::ui::diagram::{Fit, Frame as DiagramFrame};
use crate::ui::theme::Theme;
use crate::ui::words;

mod destination;
mod lists;

use destination::{
    destination_areas, destination_prompt, name_prompt, render_destination_rows, render_preview,
};
use lists::{branch_search_line, render_branch_rows, render_repo_rows, repo_search_line};

const HELP_REPO: &[&str] = &[
    "\u{21b5} branches  j/k move  / search  \u{21e5} panes  q close",
    "\u{21b5} branches  / search  \u{21e5} panes  q close",
    "\u{21b5} branches  esc close",
];

const HELP_REPO_SEARCH: &[&str] = &[
    "\u{21b5} branches  ctrl+u clear  esc cancel  \u{2191}\u{2193} move",
    "\u{21b5} branches  esc cancel",
];

/// The branch step, when Esc has a repository list to go back to. As in the panes view, the
/// other view outranks a way of moving around this one, so `Tab` survives to the
/// second-to-last rung.
const HELP_BRANCH_BACK: &[&str] = &[
    "\u{21b5} choose  j/k move  / search  n new branch  f fetch  i order  shift+i reverse  \u{21e5} panes  esc back  q close",
    "\u{21b5} choose  j/k move  / search  n new branch  f fetch  i order  \u{21e5} panes  esc back  q close",
    "\u{21b5} choose  / search  n new branch  f fetch  \u{21e5} panes  esc back",
    "\u{21b5} choose  / search  n new branch  \u{21e5} panes  esc back",
    "\u{21b5} choose  / search  \u{21e5} panes  esc back",
    "\u{21b5} choose  esc",
];

/// The same, with only one repository open: Esc has nowhere to go but out.
const HELP_BRANCH: &[&str] = &[
    "\u{21b5} choose  j/k move  / search  n new branch  f fetch  i order  shift+i reverse  \u{21e5} panes  q close",
    "\u{21b5} choose  j/k move  / search  n new branch  f fetch  i order  \u{21e5} panes  q close",
    "\u{21b5} choose  / search  n new branch  f fetch  \u{21e5} panes  q close",
    "\u{21b5} choose  / search  n new branch  \u{21e5} panes  q close",
    "\u{21b5} choose  / search  \u{21e5} panes  q close",
    "\u{21b5} choose  esc",
];

/// While the name of a new branch is being typed.
const HELP_NAME: &[&str] = &[
    "\u{21b5} next  ctrl+u clear  esc back",
    "\u{21b5} next  esc back",
];

const HELP_DESTINATION: &[&str] = &[
    "\u{21b5} open here  \u{2191}\u{2193} move  esc back  q close",
    "\u{21b5} open  esc back",
];

/// While a step that can still be abandoned is running.
const HELP_WORKING_STOPPABLE: &[&str] = &["ctrl+c stop", "ctrl+c"];

/// While one that cannot: stopping now would leave a workspace nobody moved.
const HELP_WORKING: &[&str] = &["working\u{2026}"];

const HELP_FAILED: &[&str] = &["\u{21b5} close  esc close", "\u{21b5} close"];

/// Searching either list: the `Ctrl-` forms keep working here, which is how an order or a
/// fetch is reached without abandoning what has been typed.
const HELP_BRANCH_SEARCH: &[&str] = &[
    "\u{21b5} choose  ctrl+u clear  esc cancel  \u{2191}\u{2193} move  ctrl+f fetch  ctrl+o order  ctrl+r reverse",
    "\u{21b5} choose  ctrl+u clear  esc cancel  \u{2191}\u{2193} move  ctrl+f fetch",
    "\u{21b5} choose  esc cancel",
];

/// The `/` lights up when the search field has the keyboard, exactly as it does in the
/// panes view: it is the only thing on screen that says which of the two modes you are in.
fn prompt_style(state: &BranchesState, theme: &Theme) -> Style {
    if state.is_filtering() {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        theme.dim()
    }
}

pub fn draw(frame: &mut Frame, state: &BranchesState, theme: &Theme) {
    let Some(panel) = layout(frame) else {
        return;
    };

    match state.step() {
        Step::Repo => {
            frame.render_widget(
                repo_search_line(state, theme, panel.search.width),
                panel.search,
            );
            render_rule(frame, panel.rule, theme);
            render_repo_rows(frame, state, theme, panel.body);
            render_detail(frame, &state.repo_detail(), theme, panel.detail);
            let variants = if state.is_filtering() {
                HELP_REPO_SEARCH
            } else {
                HELP_REPO
            };
            frame.render_widget(footer(variants, theme, panel.footer.width), panel.footer);
        }
        Step::Branch => {
            frame.render_widget(
                branch_search_line(state, theme, panel.search.width),
                panel.search,
            );
            // The rule the other steps draw plain is a heading here: a list of branches
            // means nothing without knowing whose branches they are.
            render_detail(frame, &state.repo_heading(), theme, panel.rule);
            render_branch_rows(frame, state, theme, panel.body);
            render_detail(frame, &state.detail(), theme, panel.detail);
            let variants = match (state.is_filtering(), state.has_repo_step()) {
                (true, _) => HELP_BRANCH_SEARCH,
                (false, true) => HELP_BRANCH_BACK,
                (false, false) => HELP_BRANCH,
            };
            frame.render_widget(footer(variants, theme, panel.footer.width), panel.footer);
        }
        // The list stays under the prompt because the base is on it: taking it away would
        // ask what to call the new branch without showing what it is cut from.
        Step::Name => {
            frame.render_widget(name_prompt(state, theme, panel.search.width), panel.search);
            render_detail(frame, &state.repo_heading(), theme, panel.rule);
            render_branch_rows(frame, state, theme, panel.body);
            render_detail(frame, &state.detail(), theme, panel.detail);
            frame.render_widget(footer(HELP_NAME, theme, panel.footer.width), panel.footer);
        }
        Step::Destination => {
            frame.render_widget(
                destination_prompt(state, theme, panel.search.width),
                panel.search,
            );
            render_rule(frame, panel.rule, theme);
            let (list, preview) = destination_areas(panel.body);
            render_destination_rows(frame, state, theme, list);
            if let Some(preview) = preview {
                render_preview(frame, &state.preview(), theme, preview);
            }
            render_detail(frame, &state.destination_detail(), theme, panel.detail);
            let variants = match state.activity() {
                Activity::Choosing => HELP_DESTINATION,
                Activity::Working { stage, .. } if stage.interruptible() => HELP_WORKING_STOPPABLE,
                Activity::Working { .. } => HELP_WORKING,
                Activity::Failed { .. } => HELP_FAILED,
            };
            frame.render_widget(footer(variants, theme, panel.footer.width), panel.footer);
        }
    }
}

#[cfg(test)]
mod tests {

    use crate::ui::render::fixtures::*;

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::domain::progress::Stage;

    use crate::port::Track;

    use crate::ui::branches::BranchData;

    #[test]
    fn draws_the_repositories_the_session_has_open() {
        insta::assert_snapshot!(branches_screen(&branches_picker(), 92, 12));
    }

    #[test]
    fn a_branch_whose_upstream_is_gone_says_so_beside_what_it_is() {
        // The ordinary end of a merged branch: GitHub deleted the head, a pruning fetch
        // noticed, and the local branch and its checkout are all that is left.
        let mut state = branches_picker();
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let mut data = branch_data();
        data.local_refs[1].track = Some(Track::Gone);
        data.local_refs[3].track = Some(Track::Gone);
        state.set_data(data);
        insta::assert_snapshot!(branches_screen(&state, 92, 12));
    }

    #[test]
    fn draws_branches_in_the_same_chrome_as_the_panes_view() {
        insta::assert_snapshot!(branches_screen(&branches_state(), 92, 12));
    }

    /// Entered from the second repository rather than the first, so a heading wired to
    /// whichever one the picker opened on would show.
    #[test]
    fn names_the_repository_above_the_list_and_not_on_every_row() {
        let mut state = branches_picker();
        state.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        state.set_data(branch_data());
        insta::assert_snapshot!(branches_screen(&state, 92, 12));
    }

    #[test]
    fn draws_a_spinner_beside_whatever_it_is_waiting_for() {
        let mut state = branches_state();
        state.set_data(BranchData {
            fetching: true,
            ..branch_data()
        });
        state.tick();
        insta::assert_snapshot!(branches_screen(&state, 110, 8));
    }

    #[test]
    fn draws_the_listing_it_starts_with_as_a_quieter_wait_of_the_same_shape() {
        let mut state = branches_state();
        state.set_data(BranchData {
            loading: true,
            ..branch_data()
        });
        insta::assert_snapshot!(branches_screen(&state, 110, 8));
    }

    #[test]
    fn the_branch_search_field_hides_its_placeholder_too() {
        let mut state = branches_state();
        state.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        insta::assert_snapshot!(branches_screen(&state, 92, 6));
    }

    #[test]
    fn draws_the_order_beside_the_count_and_reorders_the_list_to_match() {
        let mut state = branches_state();
        state.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        insta::assert_snapshot!(branches_screen(&state, 92, 12));
    }

    #[test]
    fn draws_the_offer_to_create_a_branch_that_does_not_exist() {
        let mut state = branches_state();
        search(&mut state, "feat/brand-new");
        insta::assert_snapshot!(branches_screen(&state, 92, 10));
    }

    #[test]
    fn draws_the_step_it_is_on_instead_of_the_question_it_already_asked() {
        let mut state = branches_state();
        search(&mut state, "chore");
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        state.start_working(Stage::Fetching {
            remote: "origin".into(),
            branch: "chore/deps".into(),
        });
        insta::assert_snapshot!(branches_screen(&state, 110, 14));
    }

    #[test]
    fn draws_a_failure_where_the_step_was() {
        let mut state = branches_state();
        search(&mut state, "chore");
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        state.start_working(Stage::Fetching {
            remote: "origin".into(),
            branch: "chore/deps".into(),
        });
        state.fail("could not read from remote repository".into());
        insta::assert_snapshot!(branches_screen(&state, 110, 14));
    }

    #[test]
    fn draws_a_warning_instead_of_a_diagram_for_a_zoomed_tab() {
        let mut state = branches_state();
        search(&mut state, "chore");
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        // Move to the zoomed tab, which is the last destination in this fixture.
        state.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        insta::assert_snapshot!(branches_screen(&state, 110, 14));
    }

    #[test]
    fn draws_the_destination_preview_at_a_realistic_size() {
        let mut state = branches_state();
        search(&mut state, "chore");
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        insta::assert_snapshot!(branches_screen(&state, 110, 22));
    }

    #[test]
    fn draws_the_prompt_for_a_branch_started_from_the_one_under_the_cursor() {
        let mut state = branches_state();
        state.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        for c in "feat/login-v2".chars() {
            state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        insta::assert_snapshot!(branches_screen(&state, 92, 12));
    }

    /// A name git would take is not the same as a name this repository has room for.
    #[test]
    fn draws_the_reason_a_name_was_refused_where_the_name_is() {
        let mut state = branches_state();
        state.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        for c in "main".chars() {
            state.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        insta::assert_snapshot!(branches_screen(&state, 92, 12));
    }

    #[test]
    fn draws_the_destination_step_with_split_here_selected() {
        let mut state = branches_state();
        search(&mut state, "chore");
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        insta::assert_snapshot!(branches_screen(&state, 92, 12));
    }
}
