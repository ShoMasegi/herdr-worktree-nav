//! Drawing the pickers.
//!
//! The layout is herdr's session navigator: a search line, a rule, the rows, a breadcrumb
//! for the row under the cursor, and a key hint. Reproduced from `src/ui/navigator.rs` in
//! herdr 0.7.4.
//!
//! This module is the chrome both pickers share: the panel they are laid out in, the rule,
//! the breadcrumb, the footer, the scrollbar, and the text fitting every row goes through.
//! What each picker draws inside that chrome is [`panes`] and [`branches`]; [`prompt`] is
//! the layer the panes view puts over its own rows.
//!
//! Rendering is a function of state, so the whole screen is covered by snapshot tests over
//! a `TestBackend` buffer rather than by looking at a terminal.

pub mod branches;
pub mod panes;
mod prompt;

#[cfg(test)]
pub(crate) mod fixtures;

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::domain::model::PaneNode;
use crate::domain::rows::{self};
use crate::ui::theme::Theme;

/// The four rows the picker lays out in, mirroring herdr's navigator geometry.
///
/// There is no panel to draw. The pickers open as popups, and herdr already frames a popup
/// with an accent-coloured border and a title — the same frame its navigator draws for
/// itself — so this fills what is inside it.
struct Panel {
    search: Rect,
    rule: Rect,
    body: Rect,
    detail: Rect,
    footer: Rect,
}

fn layout(frame: &Frame) -> Option<Panel> {
    let area = frame.area();
    if area.width < 4 || area.height < 4 {
        return None;
    }
    Some(Panel {
        search: Rect::new(area.x, area.y, area.width, 1),
        rule: Rect::new(area.x, area.y + 1, area.width, 1),
        body: Rect::new(area.x, area.y + 2, area.width, area.height - 4),
        detail: Rect::new(area.x, area.y + area.height - 2, area.width, 1),
        footer: Rect::new(area.x, area.y + area.height - 1, area.width, 1),
    })
}

fn render_rule(frame: &mut Frame, area: Rect, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new("\u{2500}".repeat(area.width as usize)).style(theme.rule()),
        area,
    );
}

/// The breadcrumb, drawn over a rule so the list has a bottom edge even when it is empty.
fn render_detail(frame: &mut Frame, detail: &str, theme: &Theme, area: Rect) {
    render_rule(frame, area, theme);
    if detail.is_empty() || area.width < 4 {
        return;
    }
    let text = truncate(detail, area.width.saturating_sub(2) as usize);
    frame.render_widget(Paragraph::new(format!(" {text} ")).style(theme.dim()), area);
}

fn first_that_fits(variants: &[&'static str], width: u16) -> &'static str {
    variants
        .iter()
        .find(|text| text.chars().count() < width as usize)
        .copied()
        .or_else(|| variants.last().copied())
        .unwrap_or_default()
}

fn footer(variants: &[&'static str], theme: &Theme, width: u16) -> Paragraph<'static> {
    let text = first_that_fits(variants, width);
    Paragraph::new(Line::from(Span::styled(format!(" {text}"), theme.dim())))
}

/// Keep the cursor on screen with as little movement as possible.
fn scroll_offset(cursor: usize, total: usize, viewport: usize) -> usize {
    if total <= viewport {
        return 0;
    }
    cursor
        .saturating_sub(viewport / 2)
        .min(total.saturating_sub(viewport))
}

fn render_scrollbar(
    frame: &mut Frame,
    scroll: usize,
    total: usize,
    viewport: usize,
    theme: &Theme,
    area: Rect,
) {
    if total <= viewport || area.width <= 1 {
        return;
    }
    let track = area.height as usize;
    let thumb = ((viewport * track) / total).max(1).min(track);
    let span = total.saturating_sub(viewport);
    // `span` is zero when the whole list fits, and there is nowhere to scroll it to then.
    let top = (scroll * track.saturating_sub(thumb))
        .checked_div(span)
        .unwrap_or(0);
    for offset in 0..track {
        let filled = offset >= top && offset < top + thumb;
        let rect = Rect::new(area.x + area.width - 1, area.y + offset as u16, 1, 1);
        frame.render_widget(
            Paragraph::new(if filled { "\u{2588}" } else { "\u{2502}" }).style(if filled {
                Style::default().fg(theme.accent)
            } else {
                theme.tree()
            }),
            rect,
        );
    }
}

/// Cut out of the middle, keeping both ends. A path's head says which tree it is in and its
/// tail says which checkout, so losing the middle costs the least. Matches herdr's own
/// `middle_elide`: an even split around a single ellipsis.
pub(crate) fn middle_elide(text: &str, width: usize) -> String {
    let length = text.chars().count();
    if length <= width {
        return text.to_string();
    }
    if width <= 1 {
        return "\u{2026}".to_string();
    }
    let content = width - 1;
    let left = content / 2;
    let right = content - left;
    let prefix: String = text.chars().take(left).collect();
    let suffix: String = text.chars().skip(length - right).collect();
    let mut out = prefix;
    out.push('\u{2026}');
    out.push_str(&suffix);
    out
}

/// Cut to `width` characters, with an ellipsis when something was dropped.
pub(crate) fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

// ---------------------------------------------------------------------------------------
// Branches view
// ---------------------------------------------------------------------------------------

/// Right-pad to `width` characters so a column lines up.
fn pad(text: &str, width: usize) -> String {
    let len = text.chars().count();
    let mut out = text.to_string();
    out.extend(std::iter::repeat_n(' ', width.saturating_sub(len)));
    out
}

/// Braille rather than a bar because none of the waits have a length to measure: a fetch is
/// as long as somebody else's network.
const SPINNER: [&str; 10] = [
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280f}",
];

pub(super) fn spinner(frame: usize) -> &'static str {
    SPINNER[frame % SPINNER.len()]
}

/// What an agent is doing, in the words the list behind the box uses. Empty for a pane with
/// no agent: the glyph beside it already says there is nothing to report, and a column of
/// `unknown` would be noise.
pub(super) fn agent_state(pane: &PaneNode) -> &'static str {
    rows::status_label(pane.agent_status).unwrap_or("")
}

/// `1 branch`, `5 branches`. A list that says "1 branches" reads like it is guessing.
pub(super) fn count_of(n: usize, singular: &str, plural: &str) -> String {
    format!("{n} {}", if n == 1 { singular } else { plural })
}
