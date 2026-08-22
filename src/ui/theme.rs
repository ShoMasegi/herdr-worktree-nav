//! Turning herdr's chrome into ratatui styles.
//!
//! Only the accent and the status glyph set come from herdr's configuration; everything
//! else uses the terminal's own sixteen colours, so the pickers inherit whatever palette
//! the user's terminal is set to rather than fighting it.

use ratatui::style::{Color, Modifier, Style};

use crate::domain::chrome::{Accent, Chrome, IndicatorStyle, NamedColor};
use crate::port::AgentStatus;

pub struct Theme {
    pub accent: Color,
    indicators: IndicatorStyle,
}

impl Theme {
    pub fn new(chrome: Chrome) -> Self {
        Self {
            accent: color(chrome.accent),
            indicators: chrome.indicators,
        }
    }

    /// The selected row: herdr fills it with the accent and writes on top in the panel's
    /// background colour. Black is the readable choice against every accent herdr ships.
    pub fn selected(&self) -> Style {
        Style::default().bg(self.accent).fg(Color::Black)
    }

    pub fn dim(&self) -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }

    /// Rules and inactive scrollbar track.
    pub fn rule(&self) -> Style {
        Style::default().fg(Color::DarkGray)
    }

    /// Tree glyphs recede one shade below the labels so the structure stays behind them.
    pub fn tree(&self) -> Style {
        Style::default().fg(Color::DarkGray)
    }

    /// herdr's glyph for an agent state, in `[ui] status_indicators` style.
    pub fn status_glyph(&self, status: AgentStatus) -> (&'static str, Style) {
        let glyph = match (self.indicators, status) {
            (IndicatorStyle::Dots, AgentStatus::Blocked) => "\u{25cf}",
            (IndicatorStyle::Dots, AgentStatus::Working) => "\u{25cf}",
            (IndicatorStyle::Dots, AgentStatus::Done) => "\u{25cf}",
            (IndicatorStyle::Dots, AgentStatus::Idle) => "\u{25cb}",
            (IndicatorStyle::Dots, AgentStatus::Unknown) => "\u{b7}",
            (IndicatorStyle::Symbols, AgentStatus::Blocked) => "\u{d7}",
            (IndicatorStyle::Symbols, AgentStatus::Working) => "\u{25d0}",
            (IndicatorStyle::Symbols, AgentStatus::Done) => "\u{2713}",
            (IndicatorStyle::Symbols, AgentStatus::Idle) => "\u{25cb}",
            (IndicatorStyle::Symbols, AgentStatus::Unknown) => "\u{b7}",
        };
        (glyph, self.status_style(status))
    }

    /// herdr's colour for an agent state.
    pub fn status_style(&self, status: AgentStatus) -> Style {
        match status {
            AgentStatus::Blocked => Style::default().fg(Color::Red),
            AgentStatus::Working => Style::default().fg(Color::Yellow),
            // herdr draws a finished-but-unseen agent in teal, apart from a resting one.
            AgentStatus::Done => Style::default().fg(Color::Cyan),
            AgentStatus::Idle => Style::default().fg(Color::Green),
            AgentStatus::Unknown => self.dim(),
        }
    }
}

fn color(accent: Accent) -> Color {
    match accent {
        Accent::Rgb(r, g, b) => Color::Rgb(r, g, b),
        Accent::Named(NamedColor::Black) => Color::Black,
        Accent::Named(NamedColor::Red) => Color::Red,
        Accent::Named(NamedColor::Green) => Color::Green,
        Accent::Named(NamedColor::Yellow) => Color::Yellow,
        Accent::Named(NamedColor::Blue) => Color::Blue,
        Accent::Named(NamedColor::Magenta) => Color::Magenta,
        Accent::Named(NamedColor::Cyan) => Color::Cyan,
        Accent::Named(NamedColor::White) => Color::White,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_the_glyph_set_herdr_is_configured_for() {
        let dots = Theme::new(Chrome {
            indicators: IndicatorStyle::Dots,
            ..Chrome::default()
        });
        let symbols = Theme::new(Chrome {
            indicators: IndicatorStyle::Symbols,
            ..Chrome::default()
        });
        assert_eq!(dots.status_glyph(AgentStatus::Working).0, "\u{25cf}");
        assert_eq!(symbols.status_glyph(AgentStatus::Working).0, "\u{25d0}");
        assert_eq!(dots.status_glyph(AgentStatus::Blocked).0, "\u{25cf}");
        assert_eq!(symbols.status_glyph(AgentStatus::Blocked).0, "\u{d7}");
        // Idle and unknown are the same in both, as they are in herdr.
        assert_eq!(dots.status_glyph(AgentStatus::Idle).0, "\u{25cb}");
        assert_eq!(symbols.status_glyph(AgentStatus::Idle).0, "\u{25cb}");
        assert_eq!(dots.status_glyph(AgentStatus::Unknown).0, "\u{b7}");
    }

    #[test]
    fn carries_the_accent_through_to_the_border_and_selection() {
        let theme = Theme::new(Chrome {
            accent: Accent::Rgb(137, 180, 250),
            ..Chrome::default()
        });
        assert_eq!(theme.accent, Color::Rgb(137, 180, 250));
        assert_eq!(theme.selected().bg, Some(Color::Rgb(137, 180, 250)));
    }
}
