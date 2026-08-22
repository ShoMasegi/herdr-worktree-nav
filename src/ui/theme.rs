//! Symbols and colours, kept in one place so the two views read as one UI.
//!
//! Only the sixteen ANSI colours are used, so the picker inherits the palette of whatever
//! theme the user has set rather than fighting it.

use ratatui::style::{Color, Modifier, Style};

use crate::port::AgentStatus;

pub const REPO_EXPANDED: &str = "\u{25be}"; // ▾
pub const REPO_COLLAPSED: &str = "\u{25b8}"; // ▸

/// Marks the repository's main checkout, to separate it from linked worktrees.
pub const PRIMARY_CHECKOUT: &str = "\u{25cf}"; // ●
pub const LINKED_CHECKOUT: &str = "\u{25cb}"; // ○

pub fn repo_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

pub fn worktree_style() -> Style {
    Style::default().fg(Color::White)
}

pub fn pane_style() -> Style {
    Style::default()
}

pub fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

pub fn selected() -> Style {
    Style::default()
        .bg(Color::DarkGray)
        .add_modifier(Modifier::BOLD)
}

pub fn header_active() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

pub fn header_inactive() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// A glyph, a short label, and a colour for what a branch currently is.
pub fn branch_glyph(
    state: &crate::domain::resolve::BranchState,
) -> (&'static str, &'static str, Style) {
    use crate::domain::resolve::BranchState;
    match state {
        BranchState::LivePane { .. } => ("\u{25cf}", "running", Style::default().fg(Color::Yellow)),
        BranchState::IdleWorktree { .. } => {
            ("\u{25cb}", "checked out", Style::default().fg(Color::Green))
        }
        BranchState::LocalRef => ("\u{00b7}", "local", dim()),
        BranchState::RemoteOnly => ("\u{2193}", "remote", Style::default().fg(Color::Blue)),
        BranchState::New => (
            "+",
            "create",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
    }
}

/// A glyph and colour for an agent's state. Unknown means herdr is not tracking an agent in
/// that pane at all, which is the normal state of a plain shell, so it is drawn quietly.
pub fn agent_glyph(status: AgentStatus) -> (&'static str, Style) {
    match status {
        AgentStatus::Working => ("\u{25cf}", Style::default().fg(Color::Yellow)),
        AgentStatus::Idle => ("\u{25cb}", Style::default().fg(Color::Green)),
        AgentStatus::Blocked => ("\u{25c6}", Style::default().fg(Color::Red)),
        AgentStatus::Done => ("\u{2713}", Style::default().fg(Color::Blue)),
        AgentStatus::Unknown => ("\u{00b7}", dim()),
    }
}
