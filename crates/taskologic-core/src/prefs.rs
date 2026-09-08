//! Per user preferences. Stored as JSON on the user row and sent to every
//! client the user logs in from. Everything has a default and every struct is
//! `serde(default)` so old rows keep loading when fields are added.

use serde::{Deserialize, Serialize};

use crate::barcode::Magic;
use crate::print::Symbology;

/// Which colour scheme the client draws with.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreset {
    /// Blue desktop, light dialogs, red titles.
    #[default]
    Default,
    /// Dark surfaces, cyan accents.
    Dark,
    /// Whatever the user put in [`CustomColors`].
    Custom,
}

impl ThemePreset {
    pub const ALL: [ThemePreset; 3] =
        [ThemePreset::Default, ThemePreset::Dark, ThemePreset::Custom];

    pub fn label(self) -> &'static str {
        match self {
            ThemePreset::Default => "default",
            ThemePreset::Dark => "dark",
            ThemePreset::Custom => "custom",
        }
    }
}

/// Colours for [`ThemePreset::Custom`]. Each is a colour name (`blue`,
/// `lightcyan`), a `#rrggbb` value, or a 0-255 palette index. Anything that
/// does not parse falls back to the default theme's colour, so a typo costs
/// one wrong colour rather than an unreadable screen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomColors {
    /// Behind everything.
    pub screen: String,
    /// The bar across the top.
    pub bar: String,
    /// Popups, forms and dialogs.
    pub surface: String,
    /// Task and board cards.
    pub card: String,
    pub text: String,
    /// Secondary text.
    pub muted: String,
    /// Borders and titles.
    pub accent: String,
    /// Selected row background.
    pub select: String,
    pub button: String,
    pub warn: String,
    pub danger: String,
    pub ok: String,
}

impl Default for CustomColors {
    fn default() -> Self {
        Self {
            screen: "blue".into(),
            bar: "lightblue".into(),
            surface: "white".into(),
            card: "gray".into(),
            text: "black".into(),
            muted: "darkgray".into(),
            accent: "red".into(),
            select: "red".into(),
            button: "gray".into(),
            warn: "yellow".into(),
            danger: "red".into(),
            ok: "green".into(),
        }
    }
}

/// What a task card shows on a board. A board carries the default, a user
/// can override it for themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CardFields {
    pub due_date: bool,
    pub assignees: bool,
    pub dependencies: bool,
    pub description: bool,
    /// The barcode short id. Off by default, it is internal plumbing.
    pub short_id: bool,
}

impl Default for CardFields {
    fn default() -> Self {
        Self {
            due_date: true,
            assignees: true,
            dependencies: true,
            description: false,
            short_id: false,
        }
    }
}

impl CardFields {
    /// How many lines a card needs below its title.
    pub fn extra_lines(self) -> u16 {
        u16::from(self.due_date)
            + u16::from(self.assignees)
            + u16::from(self.dependencies)
            + u16::from(self.description)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserPrefs {
    pub ui: UiPrefs,
    pub print: PrintPrefs,
    pub scanner: ScannerPrefs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiPrefs {
    /// Show other boards as tabs while inside a board.
    pub show_board_tabs: bool,
    pub theme: ThemePreset,
    pub custom_colors: CustomColors,
    /// Overrides the board's own card fields when set.
    pub card_fields: Option<CardFields>,
    /// Bigger buttons.
    pub touchscreen: bool,
    /// Barcode scanner integration. Needs printing to be useful.
    pub scanner_enabled: bool,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            show_board_tabs: true,
            theme: ThemePreset::default(),
            custom_colors: CustomColors::default(),
            card_fields: None,
            touchscreen: false,
            scanner_enabled: false,
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrintMode {
    /// Print when a task is added to a board.
    OnAdd,
    /// Print when a task moves to the started column.
    OnStart,
    #[default]
    Manual,
}

/// Only used with an automatic [`PrintMode`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoprintFilter {
    pub assigned_to_me: bool,
    pub created_by_me: bool,
    /// Narrows `created_by_me` to tasks that are also assigned to me.
    pub unless_not_assigned_to_me: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrintPrefs {
    pub show_print_button: bool,
    /// Print a reminder this many hours before the due date. None is off.
    pub reminder_hours: Option<u32>,
    pub mode: PrintMode,
    pub autoprint_filter: Option<AutoprintFilter>,
}

impl Default for PrintPrefs {
    fn default() -> Self {
        Self {
            show_print_button: true,
            reminder_hours: None,
            mode: PrintMode::Manual,
            autoprint_filter: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScannerPrefs {
    /// The scanner sends Enter after the barcode contents.
    pub presses_enter: bool,
    /// Only listen after the full width scan button is pressed.
    pub manual_only: bool,
    /// Symbology for printed barcodes. Pure preference, the content is the same.
    pub format: Symbology,
    /// Characters the scanner is programmed to emit before the contents.
    pub prefix: String,
    pub magic: Magic,
}

impl Default for ScannerPrefs {
    fn default() -> Self {
        Self {
            presses_enter: false,
            manual_only: false,
            format: Symbology::Code39,
            prefix: String::new(),
            magic: Magic::Dots,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sensible() {
        let p = UserPrefs::default();
        assert!(p.ui.show_board_tabs);
        assert!(!p.ui.scanner_enabled);
        assert_eq!(p.print.mode, PrintMode::Manual);
        assert_eq!(p.scanner.format, Symbology::Code39);
        assert_eq!(p.scanner.magic, Magic::Dots);
        assert_eq!(p.ui.card_fields, None);
        assert_eq!(p.ui.theme, ThemePreset::Default);
        let c = CardFields::default();
        assert!(c.due_date && c.assignees && c.dependencies && !c.short_id);
        assert_eq!(c.extra_lines(), 3);
    }
}
