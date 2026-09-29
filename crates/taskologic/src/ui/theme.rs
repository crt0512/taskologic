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
use taskologic_core::prefs::{Contrast, CustomColors, Darkness, ThemePreset};

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
    /// The frame of the selected card or column: the accent, unless the
    /// accent is too dark to read as a highlight.
    pub outline: Option<Color>,
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
    outline: None,
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
        outline: c(Color::Indexed(124), Color::Red),
        select_bg: c(Color::Indexed(124), Color::Red),
        select_fg: c(Color::Indexed(231), Color::White),
        button_bg: c(Color::Indexed(250), Color::Gray),
        button_fg: c(Color::Indexed(16), Color::Black),
        warn: c(Color::Indexed(130), Color::Yellow),
        danger: c(Color::Indexed(124), Color::Red),
        ok: c(Color::Indexed(28), Color::Green),
    }
}

/// Deep reds from the desktop up, light grey text, white on the selection.
/// The bars sit a shade above the buttons, cards share the button red. On a
/// sixteen colour terminal it is black with red accents. Low is the original
/// look, whose muted text and borders sit at roughly 2.6:1 and 1.4:1 against
/// the reds; normal lifts them to about 5:1 and 3:1, high to 8:1 and 5:1 and
/// spreads the layers apart.
fn blood_palette(full: bool, contrast: Contrast) -> Palette {
    let c = |f: Color, a: Color| Some(if full { f } else { a });
    let rgb = |n: u32| Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8);
    // Desktop, dialogs, bar, cards and buttons; text, muted text, borders and
    // titles, and the selected row.
    let (screen, surface, bar, card, button, text, muted, accent, select) = match contrast {
        Contrast::Low => (0x260000, 0x310000, 0x4A0000, 0x450000, 0x450000, 0xD3D3D3, 0xA43136, 0x610001, 0x610001),
        Contrast::Normal => (0x260000, 0x310000, 0x4A0000, 0x450000, 0x450000, 0xE8E8E8, 0xD0666B, 0xB8232B, 0xB8232B),
        Contrast::High => (0x1A0000, 0x2C0000, 0x520000, 0x4C0000, 0x5C0000, 0xFFFFFF, 0xF08A90, 0xF04048, 0xC0202A),
    };
    let (text, muted, accent, select) = (rgb(text), rgb(muted), rgb(accent), rgb(select));
    Palette {
        screen_bg: c(rgb(screen), Color::Black),
        screen_fg: c(text, Color::Gray),
        bar_bg: c(rgb(bar), Color::DarkGray),
        bar_fg: c(text, Color::White),
        surface_bg: c(rgb(surface), Color::Black),
        surface_fg: c(text, Color::Gray),
        surface_muted: c(muted, Color::DarkGray),
        card_bg: c(rgb(card), Color::Black),
        card_fg: c(text, Color::Gray),
        card_muted: c(muted, Color::DarkGray),
        accent: c(accent, Color::Red),
        outline: c(rgb(0xFFFFFF), Color::White),
        select_bg: c(select, Color::Red),
        select_fg: c(rgb(0xFFFFFF), Color::White),
        button_bg: c(rgb(button), Color::DarkGray),
        button_fg: c(text, Color::Gray),
        warn: c(Color::Indexed(214), Color::Yellow),
        danger: c(Color::Indexed(203), Color::LightRed),
        ok: c(Color::Indexed(114), Color::Green),
    }
}

/// The one thing the dark themes differ in: the colour of borders, titles
/// and the selected row.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Accent {
    Blue,
    Red,
    Orange,
    Yellow,
    Green,
}

impl Accent {
    /// The accent in the 256 colour palette and in the basic sixteen, and
    /// the text colour that reads on it as a selection background.
    fn colors(self) -> (Color, Color, Color, Color) {
        match self {
            Accent::Blue => (Color::Indexed(45), Color::Cyan, Color::Indexed(16), Color::Black),
            Accent::Red => (Color::Indexed(196), Color::Red, Color::Indexed(231), Color::White),
            Accent::Orange => (Color::Indexed(214), Color::Yellow, Color::Indexed(16), Color::Black),
            Accent::Yellow => (Color::Indexed(226), Color::LightYellow, Color::Indexed(16), Color::Black),
            Accent::Green => (Color::Indexed(46), Color::Green, Color::Indexed(16), Color::Black),
        }
    }
}

/// The greys of a dark theme at each darkness level, in the 256 colour
/// palette: desktop, dialogs, cards and bar, buttons. The sixteen colour
/// palette has no such steps and uses the same three greys throughout.
fn dark_greys(level: Darkness) -> (Color, Color, Color, Color) {
    let i = Color::Indexed;
    match level {
        Darkness::Black => (i(16), i(234), i(236), i(237)),
        Darkness::Darker => (i(233), i(234), i(236), i(237)),
        Darkness::Default => (i(233), i(235), i(238), i(240)),
        Darkness::Lighter => (i(234), i(236), i(239), i(241)),
    }
}

/// Dark surfaces with the accent in the borders, titles and selections;
/// dialogs and cards sit on greys above the desktop, at the default level
/// until [`Theme::with_darkness`] says otherwise.
fn dark_palette(full: bool, accent: Accent) -> Palette {
    let c = |f: Color, a: Color| Some(if full { f } else { a });
    let (accent_full, accent_ansi, on_full, on_ansi) = accent.colors();
    let (screen, surface, card, button) = dark_greys(Darkness::Default);
    Palette {
        screen_bg: c(screen, Color::Black),
        screen_fg: c(Color::Indexed(253), Color::White),
        bar_bg: c(card, Color::DarkGray),
        bar_fg: c(Color::Indexed(231), Color::White),
        surface_bg: c(surface, Color::Black),
        surface_fg: c(Color::Indexed(253), Color::White),
        surface_muted: c(Color::Indexed(245), Color::Gray),
        card_bg: c(card, Color::Black),
        card_fg: c(Color::Indexed(253), Color::White),
        card_muted: c(Color::Indexed(245), Color::Gray),
        accent: c(accent_full, accent_ansi),
        outline: c(accent_full, accent_ansi),
        select_bg: c(accent_full, accent_ansi),
        select_fg: c(on_full, on_ansi),
        button_bg: c(button, Color::Gray),
        button_fg: c(Color::Indexed(231), Color::White),
        warn: c(Color::Indexed(214), Color::Yellow),
        danger: c(Color::Indexed(203), Color::Red),
        ok: c(Color::Indexed(114), Color::Green),
    }
}

/// Custom colours, with the dark blue theme filling in anything
/// unparseable: the fields start out as that theme, so a custom theme is
/// dark blue with whatever was changed.
fn custom_palette(c: &CustomColors, full: bool) -> Palette {
    let base = dark_palette(full, Accent::Blue);
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
        outline: accent,
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
    /// One of the dark grey presets, which the darkness level applies to.
    pub dark: bool,
    /// The blood theme, which the contrast level applies to.
    pub blood: bool,
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
            (m, ThemePreset::DarkBlue) => dark_palette(m == ColorMode::Full, Accent::Blue),
            (m, ThemePreset::DarkRed) => dark_palette(m == ColorMode::Full, Accent::Red),
            (m, ThemePreset::DarkOrange) => dark_palette(m == ColorMode::Full, Accent::Orange),
            (m, ThemePreset::DarkYellow) => dark_palette(m == ColorMode::Full, Accent::Yellow),
            (m, ThemePreset::DarkGreen) => dark_palette(m == ColorMode::Full, Accent::Green),
            (m, ThemePreset::Blood) => blood_palette(m == ColorMode::Full, Contrast::Low),
            (m, ThemePreset::Custom) => custom_palette(custom, m == ColorMode::Full),
        };
        Theme {
            mode,
            ascii,
            touch: false,
            dark: preset.is_dark(),
            blood: preset == ThemePreset::Blood,
            mouse: None,
            palette,
        }
    }

    pub fn with_touch(mut self, touch: bool) -> Theme {
        self.touch = touch;
        self
    }

    /// How dark a dark theme is: moves the desktop, dialogs, cards, bar and
    /// buttons together. The light and custom themes ignore it, and so does
    /// a sixteen colour terminal, which has no steps between its greys.
    pub fn with_darkness(mut self, level: Darkness) -> Theme {
        if self.dark && self.mode == ColorMode::Full {
            let (screen, surface, card, button) = dark_greys(level);
            self.palette.screen_bg = Some(screen);
            self.palette.surface_bg = Some(surface);
            self.palette.card_bg = Some(card);
            self.palette.bar_bg = Some(card);
            self.palette.button_bg = Some(button);
        }
        self
    }

    /// How strongly the blood theme stands out. The other themes ignore it,
    /// and so does a sixteen colour terminal, which has no such steps.
    pub fn with_contrast(mut self, level: Contrast) -> Theme {
        if self.blood && self.mode == ColorMode::Full {
            self.palette = blood_palette(true, level);
        }
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
            Self::style(self.palette.outline, self.palette.card_bg).add_modifier(Modifier::BOLD)
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

    /// A column's frame: the accent when it is the current column, otherwise
    /// the same muted line an unselected card has.
    pub fn column_border(&self, selected: bool) -> Style {
        if selected {
            Self::style(self.palette.outline, self.palette.screen_bg).add_modifier(Modifier::BOLD)
        } else {
            Self::style(self.palette.surface_muted, self.palette.screen_bg)
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

    /// The frame of a resting big button: a muted line, like an unselected
    /// card's. Focus and hover frame it in the selection colours instead.
    pub fn button_border(&self) -> Style {
        if self.mono() {
            return self.button();
        }
        Self::style(self.palette.surface_muted, self.palette.button_bg)
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
            t.button_border(),
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
        for preset in [
            ThemePreset::Default,
            ThemePreset::DarkBlue,
            ThemePreset::DarkRed,
            ThemePreset::DarkOrange,
            ThemePreset::DarkYellow,
            ThemePreset::DarkGreen,
            ThemePreset::Blood,
        ] {
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
        // The dark themes are the other way round.
        let d = Theme::preset(
            ThemePreset::DarkBlue,
            &CustomColors::default(),
            ColorMode::Ansi16,
            false,
        );
        assert_eq!(d.surface().fg, Some(Color::White));
        assert_ne!(d.screen().bg, t.screen().bg);
    }

    #[test]
    fn blood_is_red_all_the_way_down_and_keeps_its_colours() {
        let t = Theme::preset(ThemePreset::Blood, &CustomColors::default(), ColorMode::Full, false);
        let p = t.palette;
        assert_eq!(p.screen_bg, Some(Color::Rgb(0x26, 0x00, 0x00)));
        assert_eq!(p.surface_bg, Some(Color::Rgb(0x31, 0x00, 0x00)));
        assert_eq!(p.button_bg, Some(Color::Rgb(0x45, 0x00, 0x00)));
        assert_eq!(p.accent, Some(Color::Rgb(0x61, 0x00, 0x01)));
        assert_eq!(p.bar_bg, Some(Color::Rgb(0x4A, 0x00, 0x00)));
        assert_eq!(p.select_bg, p.accent);
        assert_eq!(p.select_fg, Some(Color::Rgb(0xFF, 0xFF, 0xFF)));
        // The accent is too dark to frame the selected card or column.
        assert_eq!(p.outline, Some(Color::Rgb(0xFF, 0xFF, 0xFF)));
        assert_eq!(t.column_border(true).fg, p.outline);
        assert_eq!(t.card_border(true).fg, p.outline);
        assert_eq!(p.surface_muted, Some(Color::Rgb(0xA4, 0x31, 0x36)));
        assert_eq!(p.screen_fg, p.surface_fg);
        // Not a grey theme: the darkness level has nothing to move.
        assert!(!ThemePreset::Blood.is_dark());
        assert_eq!(t.with_darkness(Darkness::Black).palette, p);
        assert_eq!(
            serde_json::from_str::<ThemePreset>("\"blood\"").unwrap(),
            ThemePreset::Blood
        );
    }

    /// WCAG contrast ratio of two RGB colours.
    fn ratio(a: Option<Color>, b: Option<Color>) -> f64 {
        let lum = |c: Option<Color>| {
            let Some(Color::Rgb(r, g, b)) = c else { panic!("{c:?}") };
            let f = |v: u8| {
                let v = f64::from(v) / 255.0;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn blood_contrast_low_is_the_original_and_normal_and_high_climb() {
        let blood = |level| {
            Theme::preset(ThemePreset::Blood, &CustomColors::default(), ColorMode::Full, false)
                .with_contrast(level)
                .palette
        };
        let original = Theme::preset(ThemePreset::Blood, &CustomColors::default(), ColorMode::Full, false).palette;
        assert_eq!(blood(Contrast::Low), original, "low is what it always was");
        let (low, normal, high) = (blood(Contrast::Low), blood(Contrast::Normal), blood(Contrast::High));
        let measure = |p: &Palette| {
            (
                ratio(p.surface_fg, p.surface_bg),
                ratio(p.surface_muted, p.surface_bg),
                ratio(p.accent, p.screen_bg),
                ratio(p.select_fg, p.select_bg),
            )
        };
        let (l, n, h) = (measure(&low), measure(&normal), measure(&high));
        // Every step up reads better, and normal and high pass the usual bars.
        assert!(l.1 < n.1 && n.1 < h.1, "muted text {l:?} {n:?} {h:?}");
        assert!(l.2 < n.2 && n.2 < h.2, "borders and titles {l:?} {n:?} {h:?}");
        assert!(n.1 >= 4.5 && h.1 >= 7.0, "muted text: normal {n:?}, high {h:?}");
        assert!(n.2 >= 3.0 && h.2 >= 4.5, "borders: normal {n:?}, high {h:?}");
        assert!(l.0 >= 7.0 && n.0 >= 7.0 && h.0 >= 7.0, "body text stays high everywhere");
        assert!(n.3 >= 4.5 && h.3 >= 4.5, "selected text stays readable: {n:?} {h:?}");
    }

    #[test]
    fn contrast_only_touches_blood_in_full_colour() {
        for preset in [ThemePreset::Default, ThemePreset::DarkBlue, ThemePreset::Custom] {
            let t = Theme::preset(preset, &CustomColors::default(), ColorMode::Full, false);
            assert_eq!(t.with_contrast(Contrast::High).palette, t.palette, "{preset:?}");
        }
        let t = Theme::preset(ThemePreset::Blood, &CustomColors::default(), ColorMode::Ansi16, false);
        assert_eq!(t.with_contrast(Contrast::High).palette, t.palette, "sixteen colours has no steps");
    }

    #[test]
    fn old_saved_prefs_load_with_low_contrast() {
        let ui: taskologic_core::prefs::UiPrefs = serde_json::from_str(r#"{"theme":"blood"}"#).unwrap();
        assert_eq!(ui.contrast, Contrast::Low);
        let ui: taskologic_core::prefs::UiPrefs = serde_json::from_str(r#"{"theme":"blood","contrast":"high"}"#).unwrap();
        assert_eq!(ui.contrast, Contrast::High);
    }

    #[test]
    fn the_dark_themes_differ_in_their_accent_and_nothing_else() {
        let blue = Theme::preset(ThemePreset::DarkBlue, &CustomColors::default(), ColorMode::Full, false).palette;
        for (preset, accent) in [
            (ThemePreset::DarkRed, Color::Indexed(196)),
            (ThemePreset::DarkOrange, Color::Indexed(214)),
            (ThemePreset::DarkYellow, Color::Indexed(226)),
            (ThemePreset::DarkGreen, Color::Indexed(46)),
        ] {
            let p = Theme::preset(preset, &CustomColors::default(), ColorMode::Full, false).palette;
            assert_eq!(p.accent, Some(accent), "{preset:?}");
            assert_eq!(p.outline, Some(accent), "{preset:?}");
            assert_eq!(p.select_bg, Some(accent), "{preset:?}");
            assert_eq!(p.screen_bg, blue.screen_bg, "{preset:?}");
            assert_eq!(p.surface_bg, blue.surface_bg, "{preset:?}");
            assert_eq!(p.card_bg, blue.card_bg, "{preset:?}");
            assert_eq!(p.button_bg, blue.button_bg, "{preset:?}");
        }
        // The darkness level moves the greys together, darker to lighter,
        // and only on a dark theme.
        let plain = Theme::preset(ThemePreset::DarkGreen, &CustomColors::default(), ColorMode::Full, false);
        assert_eq!(plain.palette.screen_bg, Some(Color::Indexed(233)), "default level");
        assert_eq!(plain.palette.surface_bg, Some(Color::Indexed(235)));
        let black = plain.with_darkness(Darkness::Black);
        assert_eq!(black.palette.screen_bg, Some(Color::Indexed(16)));
        assert_eq!(black.palette.surface_bg, Some(Color::Indexed(234)));
        assert_eq!(black.palette.button_bg, Some(Color::Indexed(237)));
        let darker = plain.with_darkness(Darkness::Darker);
        assert_eq!(darker.palette.screen_bg, Some(Color::Indexed(233)));
        assert_eq!(darker.palette.surface_bg, black.palette.surface_bg);
        let lighter = plain.with_darkness(Darkness::Lighter);
        assert_eq!(lighter.palette.surface_bg, Some(Color::Indexed(236)));
        assert_eq!(lighter.palette.card_bg, Some(Color::Indexed(239)));
        assert_eq!(black.palette.accent, plain.palette.accent, "the accent never moves");
        let light = Theme::preset(ThemePreset::Default, &CustomColors::default(), ColorMode::Full, false);
        assert_eq!(light.with_darkness(Darkness::Black).palette.screen_bg, light.palette.screen_bg);
        // A custom theme nobody has touched is dark blue.
        let custom = Theme::preset(ThemePreset::Custom, &CustomColors::default(), ColorMode::Full, false).palette;
        assert_eq!(custom.screen_bg, blue.screen_bg);
        assert_eq!(custom.accent, blue.accent);
        assert_eq!(custom.select_fg, blue.select_fg);
        // And the old name for dark blue still loads from saved preferences.
        assert_eq!(serde_json::from_str::<ThemePreset>("\"dark\"").unwrap(), ThemePreset::DarkBlue);
        assert_eq!(serde_json::to_string(&ThemePreset::DarkBlue).unwrap(), "\"dark_blue\"");
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
            Some(Color::Black),
            "fell back to dark blue"
        );
        assert_eq!(
            t.surface_title().fg,
            Some(Color::Green),
            "and kept what did parse"
        );
    }
}
