//! Colours and glyphs.
//!
//! One palette, resolved once per frame from the user's chosen preset and
//! the terminal's capabilities. Foreground things (popups, forms, cards) get
//! their own background so they read as objects sitting on top of the board,
//! rather than text mixed into it. Everything degrades: 256 colours, then
//! 16, then plain attributes.

use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use serde::Deserialize;
use taskologic_core::prefs::{CustomColors, ThemePreset};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorMode {
    #[default]
    Full,
    Ansi16,
    Mono,
}

impl ColorMode {
    /// What the terminal says it can do. `NO_COLOR` and `TERM=dumb` mean
    /// monochrome, anything without 256 colour support gets the 16 colour
    /// palette.
    pub fn detect() -> ColorMode {
        if std::env::var_os("NO_COLOR").is_some() {
            return ColorMode::Mono;
        }
        let term = std::env::var("TERM")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if term.is_empty() || term == "dumb" || term.contains("mono") {
            return ColorMode::Mono;
        }
        let colorterm = std::env::var("COLORTERM")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if term.contains("256")
            || term.contains("direct")
            || colorterm.contains("truecolor")
            || colorterm.contains("24bit")
        {
            ColorMode::Full
        } else {
            ColorMode::Ansi16
        }
    }
}

pub const ASCII_BORDER: border::Set<'static> = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// A colour name, `#rrggbb`, or a 0-255 palette index.
pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim().to_ascii_lowercase();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            let n = u32::from_str_radix(hex, 16).ok()?;
            return Some(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8));
        }
        return None;
    }
    if let Ok(i) = s.parse::<u8>() {
        return Some(Color::Indexed(i));
    }
    Some(match s.as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        "white" => Color::White,
        _ => return None,
    })
}

/// Every colour the client uses. None means "leave it to the terminal",
/// which is what monochrome mode sets everything to.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Palette {
    pub screen_bg: Option<Color>,
    pub screen_fg: Option<Color>,
    pub bar_bg: Option<Color>,
    pub bar_fg: Option<Color>,
    pub surface_bg: Option<Color>,
    pub surface_fg: Option<Color>,
    pub surface_muted: Option<Color>,
    pub card_bg: Option<Color>,
    pub card_fg: Option<Color>,
    pub card_muted: Option<Color>,
    pub accent: Option<Color>,
    pub select_bg: Option<Color>,
    pub select_fg: Option<Color>,
    pub button_bg: Option<Color>,
    pub button_fg: Option<Color>,
    pub warn: Option<Color>,
    pub danger: Option<Color>,
    pub ok: Option<Color>,
}

const MONO: Palette = Palette {
    screen_bg: None,
    screen_fg: None,
    bar_bg: None,
    bar_fg: None,
    surface_bg: None,
    surface_fg: None,
    surface_muted: None,
    card_bg: None,
    card_fg: None,
    card_muted: None,
    accent: None,
    select_bg: None,
    select_fg: None,
    button_bg: None,
    button_fg: None,
    warn: None,
    danger: None,
    ok: None,
};

/// The classic terminal dialog look: a blue desktop with light dialog boxes,
/// dark text and red titles. This is the default.
fn default_palette(full: bool) -> Palette {
    let c = |f: Color, a: Color| Some(if full { f } else { a });
    Palette {
        screen_bg: c(Color::Indexed(18), Color::Blue),
        screen_fg: c(Color::Indexed(253), Color::White),
        bar_bg: c(Color::Indexed(25), Color::Blue),
        bar_fg: c(Color::Indexed(231), Color::White),
        surface_bg: c(Color::Indexed(255), Color::White),
        surface_fg: c(Color::Indexed(16), Color::Black),
        surface_muted: c(Color::Indexed(240), Color::DarkGray),
        card_bg: c(Color::Indexed(250), Color::Gray),
        card_fg: c(Color::Indexed(16), Color::Black),
        card_muted: c(Color::Indexed(238), Color::DarkGray),
        accent: c(Color::Indexed(124), Color::Red),
        select_bg: c(Color::Indexed(124), Color::Red),
        select_fg: c(Color::Indexed(231), Color::White),
        button_bg: c(Color::Indexed(250), Color::Gray),
        button_fg: c(Color::Indexed(16), Color::Black),
        warn: c(Color::Indexed(130), Color::Yellow),
        danger: c(Color::Indexed(124), Color::Red),
        ok: c(Color::Indexed(28), Color::Green),
    }
}

fn dark_palette(full: bool) -> Palette {
    let c = |f: Color, a: Color| Some(if full { f } else { a });
    Palette {
        screen_bg: c(Color::Indexed(233), Color::Black),
        screen_fg: c(Color::Indexed(253), Color::White),
        bar_bg: c(Color::Indexed(238), Color::DarkGray),
        bar_fg: c(Color::Indexed(231), Color::White),
        surface_bg: c(Color::Indexed(235), Color::Black),
        surface_fg: c(Color::Indexed(253), Color::White),
        surface_muted: c(Color::Indexed(245), Color::Gray),
        card_bg: c(Color::Indexed(238), Color::Black),
        card_fg: c(Color::Indexed(253), Color::White),
        card_muted: c(Color::Indexed(245), Color::Gray),
        accent: c(Color::Indexed(45), Color::Cyan),
        select_bg: c(Color::Indexed(45), Color::Cyan),
        select_fg: c(Color::Indexed(16), Color::Black),
        button_bg: c(Color::Indexed(240), Color::Gray),
        button_fg: c(Color::Indexed(231), Color::White),
        warn: c(Color::Indexed(214), Color::Yellow),
        danger: c(Color::Indexed(203), Color::Red),
        ok: c(Color::Indexed(114), Color::Green),
    }
}

/// Custom colours, with the default theme filling in anything unparseable.
fn custom_palette(c: &CustomColors, full: bool) -> Palette {
    let base = default_palette(full);
    let p = |s: &str, fallback: Option<Color>| parse_color(s).map(Some).unwrap_or(fallback);
    let screen = p(&c.screen, base.screen_bg);
    let surface = p(&c.surface, base.surface_bg);
    let card = p(&c.card, base.card_bg);
    let text = p(&c.text, base.surface_fg);
    let muted = p(&c.muted, base.surface_muted);
    let accent = p(&c.accent, base.accent);
    let select = p(&c.select, base.select_bg);
    let button = p(&c.button, base.button_bg);
    let bar = p(&c.bar, base.bar_bg);
    Palette {
        screen_bg: screen,
        screen_fg: base.screen_fg,
        bar_bg: bar,
        bar_fg: base.bar_fg,
        surface_bg: surface,
        surface_fg: text,
        surface_muted: muted,
        card_bg: card,
        card_fg: text,
        card_muted: muted,
        accent,
        select_bg: select,
        select_fg: base.select_fg,
        button_bg: button,
        button_fg: base.button_fg,
        warn: p(&c.warn, base.warn),
        danger: p(&c.danger, base.danger),
        ok: p(&c.ok, base.ok),
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Theme {
    pub mode: ColorMode,
    pub ascii: bool,
    pub touch: bool,
    /// Where the pointer is, so anything clickable can light up under it.
    pub mouse: Option<(u16, u16)>,
    pub palette: Palette,
}

impl Theme {
    pub fn new(mode: ColorMode, ascii: bool) -> Theme {
        Theme::preset(ThemePreset::Default, &CustomColors::default(), mode, ascii)
    }

    pub fn preset(
        preset: ThemePreset,
        custom: &CustomColors,
        mode: ColorMode,
        ascii: bool,
    ) -> Theme {
        let palette = match (mode, preset) {
            (ColorMode::Mono, _) => MONO,
            (m, ThemePreset::Default) => default_palette(m == ColorMode::Full),
            (m, ThemePreset::Dark) => dark_palette(m == ColorMode::Full),
            (m, ThemePreset::Custom) => custom_palette(custom, m == ColorMode::Full),
        };
        Theme {
            mode,
            ascii,
            touch: false,
            mouse: None,
            palette,
        }
    }

    pub fn with_touch(mut self, touch: bool) -> Theme {
        self.touch = touch;
        self
    }

    pub fn with_mouse(mut self, mouse: Option<(u16, u16)>) -> Theme {
        self.mouse = mouse;
        self
    }

    /// True when the pointer sits inside this area.
    pub fn hovered(&self, area: ratatui::layout::Rect) -> bool {
        match self.mouse {
            Some((x, y)) => area.contains(ratatui::layout::Position::new(x, y)),
            None => false,
        }
    }

    /// Marks whatever is under the pointer, the same way a menu marks the
    /// entry you are on: the selection colour, not a decoration.
    pub fn hover(&self, _base: Style) -> Style {
        self.selected()
    }

    /// The same, applied only when the pointer is actually there.
    pub fn hover_if(&self, base: Style, area: ratatui::layout::Rect) -> Style {
        if self.hovered(area) {
            self.hover(base)
        } else {
            base
        }
    }

    fn style(fg: Option<Color>, bg: Option<Color>) -> Style {
        let mut s = Style::default();
        if let Some(fg) = fg {
            s = s.fg(fg);
        }
        if let Some(bg) = bg {
            s = s.bg(bg);
        }
        s
    }

    fn mono(&self) -> bool {
        self.mode == ColorMode::Mono
    }

    pub fn border_set(&self) -> border::Set<'static> {
        if self.ascii {
            ASCII_BORDER
        } else {
            border::PLAIN
        }
    }

    /// Painted behind the whole frame.
    pub fn screen(&self) -> Style {
        Self::style(self.palette.screen_fg, self.palette.screen_bg)
    }

    /// Plain text on the board itself.
    pub fn base(&self) -> Style {
        self.screen()
    }

    pub fn dim(&self) -> Style {
        if self.mono() {
            return Style::default().add_modifier(Modifier::DIM);
        }
        Self::style(self.palette.surface_muted, self.palette.screen_bg)
    }

    pub fn title(&self) -> Style {
        Self::style(self.palette.screen_fg, self.palette.screen_bg).add_modifier(Modifier::BOLD)
    }

    pub fn title_bar(&self) -> Style {
        if self.mono() {
            return Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD);
        }
        Self::style(self.palette.bar_fg, self.palette.bar_bg).add_modifier(Modifier::BOLD)
    }

    /// Everything inside a popup, form or dialog.
    pub fn surface(&self) -> Style {
        Self::style(self.palette.surface_fg, self.palette.surface_bg)
    }

    pub fn surface_dim(&self) -> Style {
        if self.mono() {
            return Style::default().add_modifier(Modifier::DIM);
        }
        Self::style(self.palette.surface_muted, self.palette.surface_bg)
    }

    pub fn surface_border(&self) -> Style {
        Self::style(self.palette.accent, self.palette.surface_bg)
    }

    pub fn surface_title(&self) -> Style {
        Self::style(self.palette.accent, self.palette.surface_bg).add_modifier(Modifier::BOLD)
    }

    /// A card sitting on the board.
    pub fn card(&self) -> Style {
        Self::style(self.palette.card_fg, self.palette.card_bg)
    }

    pub fn card_dim(&self) -> Style {
        if self.mono() {
            return Style::default().add_modifier(Modifier::DIM);
        }
        Self::style(self.palette.card_muted, self.palette.card_bg)
    }

    pub fn card_border(&self, selected: bool) -> Style {
        if selected {
            Self::style(self.palette.accent, self.palette.card_bg).add_modifier(Modifier::BOLD)
        } else {
            Self::style(self.palette.card_muted, self.palette.card_bg)
        }
    }

    /// A card that has been picked up for moving.
    pub fn card_picked(&self) -> Style {
        if self.mono() {
            return Style::default().add_modifier(Modifier::REVERSED);
        }
        Self::style(self.palette.warn, self.palette.card_bg).add_modifier(Modifier::BOLD)
    }

    /// The column a drag is hovering over.
    pub fn drop_target(&self) -> Style {
        if self.mono() {
            return Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        }
        Self::style(self.palette.warn, self.palette.screen_bg).add_modifier(Modifier::BOLD)
    }

    pub fn column_border(&self, selected: bool) -> Style {
        if selected {
            Self::style(self.palette.accent, self.palette.screen_bg).add_modifier(Modifier::BOLD)
        } else {
            Self::style(self.palette.screen_fg, self.palette.screen_bg)
        }
    }

    /// Selected row in a list.
    pub fn selected(&self) -> Style {
        if self.mono() {
            return Style::default().add_modifier(Modifier::REVERSED);
        }
        Self::style(self.palette.select_fg, self.palette.select_bg).add_modifier(Modifier::BOLD)
    }

    pub fn button(&self) -> Style {
        if self.mono() {
            return Style::default().add_modifier(Modifier::REVERSED | Modifier::DIM);
        }
        Self::style(self.palette.button_fg, self.palette.button_bg)
    }

    pub fn button_focus(&self) -> Style {
        if self.mono() {
            return Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD);
        }
        Self::style(self.palette.select_fg, self.palette.select_bg).add_modifier(Modifier::BOLD)
    }

    pub fn button_armed(&self) -> Style {
        if self.mono() {
            return Style::default().add_modifier(Modifier::REVERSED | Modifier::UNDERLINED);
        }
        Self::style(self.palette.surface_fg, self.palette.warn).add_modifier(Modifier::BOLD)
    }

    /// The shortcut letter inside a button label.
    pub fn button_key(&self) -> Style {
        self.button()
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    }

    /// An editable field inside a popup.
    pub fn field(&self) -> Style {
        if self.mono() {
            return Style::default().add_modifier(Modifier::UNDERLINED);
        }
        Self::style(self.palette.button_fg, self.palette.button_bg)
    }

    pub fn field_focus(&self) -> Style {
        if self.mono() {
            return Style::default().add_modifier(Modifier::REVERSED);
        }
        Self::style(self.palette.select_fg, self.palette.select_bg)
    }

    pub fn field_select(&self) -> Style {
        if self.mono() {
            return Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD);
        }
        Self::style(self.palette.surface_fg, self.palette.warn)
    }

    pub fn cursor(&self) -> Style {
        Style::default().add_modifier(Modifier::REVERSED)
    }

    pub fn error(&self) -> Style {
        if self.mono() {
            return Style::default().add_modifier(Modifier::BOLD);
        }
        Self::style(self.palette.danger, self.palette.surface_bg).add_modifier(Modifier::BOLD)
    }

    pub fn severity(&self, s: taskologic_proto::Severity) -> Style {
        use taskologic_proto::Severity::*;
        if self.mono() {
            return match s {
                Info | Success => Style::default().add_modifier(Modifier::REVERSED),
                Warning => Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
                Error => Style::default()
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD | Modifier::UNDERLINED),
            };
        }
        let bg = match s {
            Info => self.palette.button_bg,
            Success => self.palette.ok,
            Warning => self.palette.warn,
            Error => self.palette.danger,
        };
        Self::style(self.palette.button_fg, bg).add_modifier(Modifier::BOLD)
    }

    /// Column role markers. Plain ASCII, the same in every terminal.
    pub fn role_glyph(&self, role: taskologic_core::board::ColumnRole) -> &'static str {
        use taskologic_core::board::ColumnRole::*;
        match role {
            Started => ">",
            Paused => "||",
            Finished => "*",
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::new(ColorMode::Full, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styles(t: &Theme) -> Vec<Style> {
        vec![
            t.screen(),
            t.base(),
            t.dim(),
            t.title(),
            t.title_bar(),
            t.surface(),
            t.surface_dim(),
            t.card(),
            t.card_dim(),
            t.selected(),
            t.button(),
            t.button_focus(),
            t.field(),
            t.error(),
            t.severity(taskologic_proto::Severity::Error),
            t.card_border(true),
            t.column_border(false),
        ]
    }

    #[test]
    fn monochrome_never_emits_a_colour() {
        for preset in ThemePreset::ALL {
            let t = Theme::preset(preset, &CustomColors::default(), ColorMode::Mono, true);
            for s in styles(&t) {
                assert_eq!(s.fg, None, "{preset:?} {s:?}");
                assert_eq!(s.bg, None, "{preset:?} {s:?}");
            }
        }
    }

    #[test]
    fn sixteen_colour_mode_stays_in_the_basic_palette() {
        for preset in [ThemePreset::Default, ThemePreset::Dark] {
            let t = Theme::preset(preset, &CustomColors::default(), ColorMode::Ansi16, false);
            for s in styles(&t) {
                for c in [s.fg, s.bg].into_iter().flatten() {
                    assert!(
                        !matches!(c, Color::Indexed(_) | Color::Rgb(..)),
                        "{preset:?} {c:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_default_theme_is_light_dialogs_on_a_blue_desktop() {
        let t = Theme::preset(
            ThemePreset::Default,
            &CustomColors::default(),
            ColorMode::Ansi16,
            false,
        );
        assert_eq!(t.screen().bg, Some(Color::Blue));
        assert_eq!(t.surface().bg, Some(Color::White));
        assert_eq!(t.surface().fg, Some(Color::Black));
        assert_eq!(t.surface_title().fg, Some(Color::Red));
        // Cards sit on the board, not in a dialog, so they are grey.
        assert_eq!(t.card().bg, Some(Color::Gray));
        assert_eq!(t.button().bg, Some(Color::Gray));
        assert_eq!(
            t.button().fg,
            Some(Color::Black),
            "grey buttons need dark text"
        );
        assert_ne!(t.card().bg, t.surface().bg);
        assert_ne!(t.card().bg, t.screen().bg);
        // The dark theme is the other way round.
        let d = Theme::preset(
            ThemePreset::Dark,
            &CustomColors::default(),
            ColorMode::Ansi16,
            false,
        );
        assert_eq!(d.surface().fg, Some(Color::White));
        assert_ne!(d.screen().bg, t.screen().bg);
    }

    #[test]
    fn colours_parse_by_name_index_and_hex() {
        assert_eq!(parse_color("lightcyan"), Some(Color::LightCyan));
        assert_eq!(parse_color(" Blue "), Some(Color::Blue));
        assert_eq!(parse_color("235"), Some(Color::Indexed(235)));
        assert_eq!(parse_color("#1c2333"), Some(Color::Rgb(0x1c, 0x23, 0x33)));
        assert_eq!(parse_color("chartreuse"), None);
        assert_eq!(parse_color("#12345"), None);
    }

    #[test]
    fn a_bad_custom_colour_falls_back_instead_of_breaking_the_screen() {
        let custom = CustomColors {
            surface: "not a colour".into(),
            accent: "green".into(),
            ..Default::default()
        };
        let t = Theme::preset(ThemePreset::Custom, &custom, ColorMode::Ansi16, false);
        assert_eq!(
            t.surface().bg,
            Some(Color::White),
            "fell back to the default"
        );
        assert_eq!(
            t.surface_title().fg,
            Some(Color::Green),
            "and kept what did parse"
        );
    }
}
