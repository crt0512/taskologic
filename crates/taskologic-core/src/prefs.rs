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
    /// Off by default. Most tasks are read by when they are due, and every
    /// field switched on here makes every card on the board a line taller.
    pub start_date: bool,
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
            start_date: false,
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
        u16::from(self.start_date)
            + u16::from(self.due_date)
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
    /// Today's date in the top bar, right of the Taskologic label.
    pub show_date: bool,
    /// A clock with seconds, right of the date, or right of the label when
    /// the date is off.
    pub show_time: bool,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            show_board_tabs: true,
            theme: ThemePreset::default(),
            custom_colors: CustomColors::default(),
            card_fields: None,
            touchscreen: false,
            scanner_enabled: true,
            show_date: false,
            show_time: false,
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
#[serde(default, from = "PrintPrefsRepr")]
pub struct PrintPrefs {
    pub show_print_button: bool,
    /// Lead time before a task's start date. Minutes rather than hours so
    /// that "half an hour before" is exact. None is off.
    pub reminder_start_minutes: Option<u32>,
    /// The same, counted back from the due date. The two are independent:
    /// a task with both dates gets a slip before each.
    pub reminder_due_minutes: Option<u32>,
    pub mode: PrintMode,
    pub autoprint_filter: Option<AutoprintFilter>,
}

impl Default for PrintPrefs {
    fn default() -> Self {
        Self {
            show_print_button: true,
            reminder_start_minutes: None,
            reminder_due_minutes: None,
            mode: PrintMode::Manual,
            autoprint_filter: None,
        }
    }
}

/// The reminder setting has been spelled three ways. Before 0.1.10 it was
/// whole hours under `reminder_hours`; 0.1.10 made it minutes under
/// `reminder_minutes`; 0.1.11 split it in two, and the one that existed was
/// always counted back from the due date. Reading goes through here so an
/// upgrade keeps the user's setting instead of silently switching reminders
/// off. Writing never uses it, so nothing emits the old spellings again.
#[derive(Deserialize)]
#[serde(default)]
struct PrintPrefsRepr {
    show_print_button: bool,
    reminder_start_minutes: Option<u32>,
    reminder_due_minutes: Option<u32>,
    reminder_minutes: Option<u32>,
    reminder_hours: Option<u32>,
    mode: PrintMode,
    autoprint_filter: Option<AutoprintFilter>,
}

impl Default for PrintPrefsRepr {
    fn default() -> Self {
        let p = PrintPrefs::default();
        Self {
            show_print_button: p.show_print_button,
            reminder_start_minutes: p.reminder_start_minutes,
            reminder_due_minutes: p.reminder_due_minutes,
            reminder_minutes: None,
            reminder_hours: None,
            mode: p.mode,
            autoprint_filter: p.autoprint_filter,
        }
    }
}

impl From<PrintPrefsRepr> for PrintPrefs {
    fn from(r: PrintPrefsRepr) -> Self {
        Self {
            show_print_button: r.show_print_button,
            // Nothing inherits a start reminder: the old setting never meant
            // one, so it starts empty and the user opts in.
            reminder_start_minutes: r.reminder_start_minutes,
            reminder_due_minutes: r
                .reminder_due_minutes
                .or(r.reminder_minutes)
                .or(r.reminder_hours.map(|h| h.saturating_mul(60))),
            mode: r.mode,
            autoprint_filter: r.autoprint_filter,
        }
    }
}

/// The largest lead time a reminder may have, one year, so a typo cannot
/// park a reminder past the heat death of the universe.
pub const MAX_REMINDER_MINUTES: u32 = 365 * 24 * 60;

/// Hours as typed in a form to whole minutes. Fractions are allowed, so
/// "1.5" is ninety minutes. An empty string means no reminder, and zero
/// means at the due time.
pub fn parse_reminder_hours(text: &str) -> Result<Option<u32>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let hours = text
        .parse::<f64>()
        .ok()
        .filter(|h| h.is_finite())
        .ok_or("remind must be a number of hours, fractions allowed")?;
    if hours < 0.0 {
        return Err("a reminder cannot be a negative number of hours".into());
    }
    let minutes = (hours * 60.0).round();
    if minutes > f64::from(MAX_REMINDER_MINUTES) {
        return Err("a reminder cannot be more than a year early".into());
    }
    Ok(Some(minutes as u32))
}

/// Minutes back to the shortest hours string that round trips, so 90 shows
/// as "1.5" and 120 as "2".
pub fn reminder_hours_text(minutes: u32) -> String {
    if minutes.is_multiple_of(60) {
        return (minutes / 60).to_string();
    }
    let text = format!("{:.6}", f64::from(minutes) / 60.0);
    text.trim_end_matches('0').trim_end_matches('.').to_string()
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
        assert!(p.ui.scanner_enabled, "on unless the user turns it off");
        assert!(!p.ui.show_date && !p.ui.show_time);
        assert_eq!(p.print.reminder_start_minutes, None);
        assert_eq!(p.print.reminder_due_minutes, None);
        assert_eq!(p.print.mode, PrintMode::Manual);
        assert_eq!(p.scanner.format, Symbology::Code39);
        assert_eq!(p.scanner.magic, Magic::Dots);
        assert_eq!(p.ui.card_fields, None);
        assert_eq!(p.ui.theme, ThemePreset::Default);
        let c = CardFields::default();
        assert!(c.due_date && c.assignees && c.dependencies && !c.short_id);
        assert!(!c.start_date, "cards do not grow a line for everyone");
        assert_eq!(c.extra_lines(), 3);
    }

    #[test]
    fn hours_as_typed_round_trip_through_whole_minutes() {
        assert_eq!(parse_reminder_hours(""), Ok(None));
        assert_eq!(parse_reminder_hours("   "), Ok(None));
        assert_eq!(parse_reminder_hours("2"), Ok(Some(120)));
        assert_eq!(parse_reminder_hours(" 1.5 "), Ok(Some(90)));
        assert_eq!(parse_reminder_hours("0"), Ok(Some(0)), "at the due time");
        assert_eq!(reminder_hours_text(120), "2");
        assert_eq!(reminder_hours_text(90), "1.5");
        assert_eq!(reminder_hours_text(0), "0");
        for m in [1, 7, 45, 90, 600, MAX_REMINDER_MINUTES] {
            let text = reminder_hours_text(m);
            assert_eq!(parse_reminder_hours(&text), Ok(Some(m)), "{text}");
        }
    }

    #[test]
    fn a_reminder_that_is_not_a_sane_number_of_hours_is_refused() {
        let not_a_number = "remind must be a number of hours, fractions allowed";
        assert_eq!(parse_reminder_hours("soon"), Err(not_a_number.into()));
        assert_eq!(parse_reminder_hours("inf"), Err(not_a_number.into()));
        assert_eq!(
            parse_reminder_hours("-1"),
            Err("a reminder cannot be a negative number of hours".into())
        );
        assert_eq!(
            parse_reminder_hours("9000"),
            Err("a reminder cannot be more than a year early".into())
        );
    }

    #[test]
    fn prefs_written_before_the_switch_to_minutes_keep_their_reminder() {
        let old = r#"{"print":{"show_print_button":true,"reminder_hours":3,"mode":"manual"}}"#;
        let p: UserPrefs = serde_json::from_str(old).unwrap();
        assert_eq!(p.print.reminder_due_minutes, Some(180));

        let written = serde_json::to_string(&p.print).unwrap();
        assert!(
            written.contains(r#""reminder_due_minutes":180"#),
            "{written}"
        );
        for gone in ["reminder_hours", r#""reminder_minutes""#] {
            assert!(
                !written.contains(gone),
                "{gone} is read, never written again: {written}"
            );
        }

        // The newest spelling wins if a row somehow carries several.
        let all: PrintPrefs = serde_json::from_str(
            r#"{"reminder_due_minutes":45,"reminder_minutes":90,"reminder_hours":3}"#,
        )
        .unwrap();
        assert_eq!(all.reminder_due_minutes, Some(45));
        assert_eq!(
            serde_json::from_str::<PrintPrefs>("{}").unwrap(),
            PrintPrefs::default()
        );
    }

    #[test]
    fn the_reminder_that_predates_the_split_becomes_the_due_one() {
        // 0.1.10 wrote minutes and meant "before due". Nobody gains a start
        // reminder they never asked for.
        let p: PrintPrefs = serde_json::from_str(r#"{"reminder_minutes":90}"#).unwrap();
        assert_eq!(p.reminder_due_minutes, Some(90));
        assert_eq!(p.reminder_start_minutes, None);

        // Same for the whole hours that predate even that.
        let older: PrintPrefs = serde_json::from_str(r#"{"reminder_hours":2}"#).unwrap();
        assert_eq!(older.reminder_due_minutes, Some(120));
        assert_eq!(older.reminder_start_minutes, None);
    }
}
