//! The two things this plugin borrows from herdr's own appearance so its pickers look like
//! herdr's session navigator rather than like a different program.
//!
//! herdr's socket API exposes no theme or palette — verified against every method in
//! `herdr api schema` — so the only way to match is to read the user's config. Just two
//! values are taken: the accent colour, which drives the border, the selected row, the
//! repository rows and the current-row marker; and the agent status glyph set.
//!
//! Parsing is pure: the caller reads the file, this decides what it means.

use serde::Deserialize;

/// herdr's agent status glyphs. `[ui] status_indicators`, default `dots`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndicatorStyle {
    #[default]
    Dots,
    Symbols,
}

/// A colour as herdr's config can express it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accent {
    Rgb(u8, u8, u8),
    /// One of the terminal's own colours, by name.
    Named(NamedColor),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
}

/// What the pickers need from herdr's configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chrome {
    pub accent: Accent,
    pub indicators: IndicatorStyle,
}

impl Default for Chrome {
    fn default() -> Self {
        // herdr's own default when nothing is configured.
        Self {
            accent: Accent::Named(NamedColor::Cyan),
            indicators: IndicatorStyle::Dots,
        }
    }
}

/// The accent of each theme herdr ships, taken from `Palette::from_name`.
///
/// Only the accent is mirrored, not the whole palette: it is the one colour the navigator
/// uses for structure, and a single value per theme stays cheap to keep current. A theme
/// this table has never heard of falls back to herdr's default rather than guessing.
fn theme_accent(name: &str) -> Option<Accent> {
    let accent = match normalize_theme_name(name).as_str() {
        "catppuccin" => Accent::Rgb(137, 180, 250),
        "catppuccin-latte" => Accent::Rgb(30, 102, 245),
        "terminal" => Accent::Named(NamedColor::Blue),
        "tokyo-night" => Accent::Rgb(122, 162, 247),
        "tokyo-night-day" => Accent::Rgb(46, 125, 233),
        "dracula" => Accent::Rgb(189, 147, 249),
        "nord" => Accent::Rgb(136, 192, 208),
        "gruvbox" => Accent::Rgb(215, 153, 33),
        "gruvbox-light" => Accent::Rgb(7, 102, 120),
        "one-dark" => Accent::Rgb(97, 175, 239),
        "one-light" => Accent::Rgb(64, 120, 242),
        "solarized" => Accent::Rgb(38, 139, 210),
        "solarized-light" => Accent::Rgb(38, 139, 210),
        "kanagawa" => Accent::Rgb(126, 156, 216),
        "kanagawa-lotus" => Accent::Rgb(77, 105, 155),
        "rose-pine" => Accent::Rgb(196, 167, 231),
        "rose-pine-dawn" => Accent::Rgb(144, 122, 169),
        "vesper" => Accent::Rgb(255, 199, 153),
        _ => return None,
    };
    Some(accent)
}

fn normalize_theme_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('_', "-")
}

#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(default)]
    theme: RawTheme,
    #[serde(default)]
    ui: RawUi,
}

#[derive(Debug, Default, Deserialize)]
struct RawTheme {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    custom: Option<RawCustom>,
}

#[derive(Debug, Default, Deserialize)]
struct RawCustom {
    #[serde(default)]
    accent: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawUi {
    #[serde(default)]
    accent: Option<String>,
    #[serde(default)]
    status_indicators: Option<IndicatorStyle>,
}

/// herdr's own default for `[ui] accent`. A value equal to it means "not chosen", which is
/// what makes the theme's accent win.
const DEFAULT_UI_ACCENT: &str = "cyan";

/// Read the chrome out of a herdr `config.toml`.
///
/// The resolution order mirrors herdr's: an explicit `[theme.custom] accent` wins, then a
/// `[ui] accent` that has been changed from its default, then the theme's own accent.
/// Anything unparseable falls back to herdr's defaults rather than failing — a picker that
/// refuses to open because a colour name was misspelt would be worse than a cyan border.
pub fn parse(config_toml: &str) -> Chrome {
    let raw: RawConfig = toml::from_str(config_toml).unwrap_or_default();

    let custom_accent = raw
        .theme
        .custom
        .as_ref()
        .and_then(|custom| custom.accent.as_deref())
        .and_then(parse_color);

    let ui_accent = raw
        .ui
        .accent
        .as_deref()
        .filter(|value| !value.trim().eq_ignore_ascii_case(DEFAULT_UI_ACCENT))
        .and_then(parse_color);

    let theme_accent = raw.theme.name.as_deref().and_then(theme_accent);

    Chrome {
        accent: custom_accent
            .or(ui_accent)
            .or(theme_accent)
            .unwrap_or(Chrome::default().accent),
        indicators: raw.ui.status_indicators.unwrap_or_default(),
    }
}

/// `#rrggbb`, `rgb(r, g, b)`, or a colour name — the three forms herdr documents.
fn parse_color(value: &str) -> Option<Accent> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |range: std::ops::Range<usize>| u8::from_str_radix(&hex[range], 16).ok();
        return Some(Accent::Rgb(byte(0..2)?, byte(2..4)?, byte(4..6)?));
    }
    if let Some(args) = value
        .strip_prefix("rgb(")
        .or_else(|| value.strip_prefix("rgb ("))
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let parts: Vec<u8> = args
            .split(',')
            .map(|part| part.trim().parse::<u8>())
            .collect::<Result<_, _>>()
            .ok()?;
        let [r, g, b] = parts[..] else { return None };
        return Some(Accent::Rgb(r, g, b));
    }
    let named = match value.to_ascii_lowercase().as_str() {
        "black" => NamedColor::Black,
        "red" => NamedColor::Red,
        "green" => NamedColor::Green,
        "yellow" => NamedColor::Yellow,
        "blue" => NamedColor::Blue,
        "magenta" | "purple" => NamedColor::Magenta,
        "cyan" | "teal" => NamedColor::Cyan,
        "white" => NamedColor::White,
        _ => return None,
    };
    Some(Accent::Named(named))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_or_broken_config_gives_herdrs_own_defaults() {
        for input in ["", "not valid toml {{{", "[unrelated]\nkey = 1"] {
            assert_eq!(parse(input), Chrome::default(), "failed for {input:?}");
        }
        assert_eq!(Chrome::default().accent, Accent::Named(NamedColor::Cyan));
        assert_eq!(Chrome::default().indicators, IndicatorStyle::Dots);
    }

    #[test]
    fn the_themes_own_accent_is_used_when_nothing_overrides_it() {
        let chrome = parse("[theme]\nname = \"catppuccin\"\n");
        assert_eq!(chrome.accent, Accent::Rgb(137, 180, 250));
    }

    #[test]
    fn a_theme_name_this_table_has_never_heard_of_falls_back_rather_than_guessing() {
        assert_eq!(
            parse("[theme]\nname = \"some-future-theme\"\n").accent,
            Accent::Named(NamedColor::Cyan)
        );
    }

    #[test]
    fn theme_names_are_matched_loosely_enough_to_survive_underscores_and_case() {
        for name in [
            "tokyo-night",
            "Tokyo-Night",
            "tokyo_night",
            "  tokyo-night  ",
        ] {
            assert_eq!(
                parse(&format!("[theme]\nname = \"{name}\"\n")).accent,
                Accent::Rgb(122, 162, 247),
                "failed for {name}"
            );
        }
    }

    #[test]
    fn a_custom_accent_beats_the_theme_and_the_ui_setting() {
        // Mirrors herdr: theme.custom.accent suppresses the legacy ui.accent entirely.
        let chrome = parse(
            "[theme]\nname = \"catppuccin\"\n[theme.custom]\naccent = \"#ff0088\"\n[ui]\naccent = \"red\"\n",
        );
        assert_eq!(chrome.accent, Accent::Rgb(0xff, 0x00, 0x88));
    }

    #[test]
    fn a_ui_accent_that_was_actually_changed_beats_the_theme() {
        let chrome = parse("[theme]\nname = \"catppuccin\"\n[ui]\naccent = \"magenta\"\n");
        assert_eq!(chrome.accent, Accent::Named(NamedColor::Magenta));
    }

    #[test]
    fn a_ui_accent_left_at_its_default_does_not_beat_the_theme() {
        // "cyan" is herdr's default, so it means "not chosen" rather than "chosen cyan".
        let chrome = parse("[theme]\nname = \"dracula\"\n[ui]\naccent = \"cyan\"\n");
        assert_eq!(chrome.accent, Accent::Rgb(189, 147, 249));
    }

    #[test]
    fn accepts_every_colour_form_herdr_documents() {
        assert_eq!(parse_color("#89b4fa"), Some(Accent::Rgb(137, 180, 250)));
        assert_eq!(
            parse_color("rgb(137, 180, 250)"),
            Some(Accent::Rgb(137, 180, 250))
        );
        assert_eq!(
            parse_color("rgb(137,180,250)"),
            Some(Accent::Rgb(137, 180, 250))
        );
        assert_eq!(parse_color("cyan"), Some(Accent::Named(NamedColor::Cyan)));
        assert_eq!(
            parse_color("  Blue "),
            Some(Accent::Named(NamedColor::Blue))
        );
    }

    #[test]
    fn rejects_colours_it_cannot_understand_instead_of_inventing_one() {
        for bad in [
            "#89b4f",
            "#89b4fag",
            "rgb(300,0,0)",
            "rgb(1,2)",
            "rgb(1,2,3,4)",
            "chartreuse",
            "",
        ] {
            assert_eq!(parse_color(bad), None, "failed for {bad:?}");
        }
    }

    #[test]
    fn a_bad_accent_value_falls_back_instead_of_failing_the_picker() {
        let chrome = parse("[theme]\nname = \"nord\"\n[theme.custom]\naccent = \"chartreuse\"\n");
        assert_eq!(
            chrome.accent,
            Accent::Rgb(136, 192, 208),
            "an unparseable override is ignored, not fatal"
        );
    }

    #[test]
    fn reads_the_status_indicator_style() {
        assert_eq!(parse("").indicators, IndicatorStyle::Dots);
        assert_eq!(
            parse("[ui]\nstatus_indicators = \"symbols\"\n").indicators,
            IndicatorStyle::Symbols
        );
        assert_eq!(
            parse("[ui]\nstatus_indicators = \"nonsense\"\n").indicators,
            IndicatorStyle::Dots,
            "an unknown style is the default, not an error"
        );
    }

    #[test]
    fn reads_the_real_shape_of_a_herdr_config() {
        // Trimmed from an actual ~/.config/herdr/config.toml, including sections this
        // parser must ignore without complaint.
        let chrome = parse(
            r#"
# onboarding = true
[theme]
name = "catppuccin"

[terminal]
shell_mode = "auto"

[keys]
previous_agent = "prefix+ctrl+p"

[[keys.command]]
key = "prefix+d"
type = "popup"
command = "tig"

[ui]
status_indicators = "symbols"

[advanced]
"#,
        );
        assert_eq!(chrome.accent, Accent::Rgb(137, 180, 250));
        assert_eq!(chrome.indicators, IndicatorStyle::Symbols);
    }
}
