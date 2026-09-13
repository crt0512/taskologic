//! What goes on a slip, in what order, and how each part is set.
//!
//! This is the whole answer to "why did that print like that". It lives with
//! the printer, not with the user's preferences, so that one panel decides
//! the shape of a slip and nothing else quietly overrules it.

use serde::{Deserialize, Deserializer, Serialize};
use taskologic_core::print::PrintJobKind;

/// One part of a slip.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlipSection {
    /// A fixed line of your own, a shop name or a machine number.
    Custom,
    BoardTitle,
    /// The title, with the task's short id under it.
    TaskTitle,
    BarcodeStart,
    Description,
    Checklist,
    StartDate,
    DueDate,
    Creator,
    Assignee,
    Dependencies,
    BarcodeFinish,
    /// When the slip was printed.
    Timestamp,
}

impl SlipSection {
    /// Every section, in the order a new profile starts with.
    pub const ALL: [SlipSection; 13] = [
        SlipSection::Custom,
        SlipSection::BoardTitle,
        SlipSection::TaskTitle,
        SlipSection::BarcodeStart,
        SlipSection::Description,
        SlipSection::Checklist,
        SlipSection::StartDate,
        SlipSection::DueDate,
        SlipSection::Creator,
        SlipSection::Assignee,
        SlipSection::Dependencies,
        SlipSection::BarcodeFinish,
        SlipSection::Timestamp,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SlipSection::Custom => "Custom line",
            SlipSection::BoardTitle => "Board title",
            SlipSection::TaskTitle => "Task title",
            SlipSection::BarcodeStart => "Barcode: start",
            SlipSection::Description => "Description",
            SlipSection::Checklist => "Checklist",
            SlipSection::StartDate => "Start date",
            SlipSection::DueDate => "Due date",
            SlipSection::Creator => "Creator",
            SlipSection::Assignee => "Assignees",
            SlipSection::Dependencies => "Dependencies",
            SlipSection::BarcodeFinish => "Barcode: finish",
            SlipSection::Timestamp => "Date and time",
        }
    }

    /// Whether this section draws a barcode, which text output cannot do.
    pub fn is_barcode(self) -> bool {
        matches!(self, SlipSection::BarcodeStart | SlipSection::BarcodeFinish)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlipRow {
    pub section: SlipSection,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub large: bool,
    #[serde(default)]
    pub centered: bool,
    /// The words for [`SlipSection::Custom`]. Ignored by every other section.
    #[serde(default)]
    pub text: String,
}

impl SlipRow {
    fn new(section: SlipSection, enabled: bool, bold: bool, large: bool, centered: bool) -> Self {
        Self { section, enabled, bold, large, centered, text: String::new() }
    }
}

/// The sections of a slip, in the order they print.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SlipLayout {
    pub rows: Vec<SlipRow>,
}

impl Default for SlipLayout {
    fn default() -> Self {
        Self::task()
    }
}

impl SlipLayout {
    /// What a task receipt looks like out of the box.
    pub fn task() -> Self {
        use SlipSection as S;
        let mut custom = SlipRow::new(S::Custom, false, false, false, true);
        custom.text = "Taskologic".to_string();
        Self {
            rows: vec![
                custom,
                SlipRow::new(S::BoardTitle, true, true, false, true),
                SlipRow::new(S::TaskTitle, true, true, true, false),
                // A receipt is for the task you are starting, so the code that
                // finishes it is the one worth having on the paper.
                SlipRow::new(S::BarcodeStart, false, false, false, true),
                SlipRow::new(S::Description, true, false, false, false),
                SlipRow::new(S::Checklist, true, false, false, false),
                SlipRow::new(S::StartDate, false, false, false, true),
                SlipRow::new(S::DueDate, true, false, true, true),
                SlipRow::new(S::Creator, false, false, false, false),
                SlipRow::new(S::Assignee, false, false, false, false),
                SlipRow::new(S::Dependencies, true, false, true, true),
                SlipRow::new(S::BarcodeFinish, true, false, false, true),
                SlipRow::new(S::Timestamp, true, true, false, true),
            ],
        }
    }

    /// What a reminder looks like out of the box. The same parts, shaped for
    /// a slip whose job is to say a thing is due: nothing has been started
    /// yet, so it carries the code that starts it rather than the one that
    /// finishes it.
    pub fn reminder() -> Self {
        use SlipSection as S;
        let mut l = Self::task();
        for row in &mut l.rows {
            match row.section {
                S::BarcodeStart => row.enabled = true,
                S::BarcodeFinish => row.enabled = false,
                // A reminder is often about the start date, so it says when
                // that is; a receipt is handed over as work begins and does
                // not need telling.
                S::StartDate => row.enabled = true,
                _ => {}
            }
        }
        l
    }

    /// Drop repeats and add anything missing, keeping the order that was
    /// given. A config written by an older version knows nothing about a
    /// section added since; it should gain it rather than lose the slip.
    pub fn normalised(mut self, defaults: &Self) -> Self {
        let mut seen = Vec::new();
        self.rows.retain(|r| {
            let first = !seen.contains(&r.section);
            seen.push(r.section);
            first
        });
        for d in &defaults.rows {
            if !self.rows.iter().any(|r| r.section == d.section) {
                self.rows.push(d.clone());
            }
        }
        self
    }

    pub fn get(&self, section: SlipSection) -> Option<&SlipRow> {
        self.rows.iter().find(|r| r.section == section)
    }

    /// Move the row at `i` one place towards the top of the slip.
    pub fn move_up(&mut self, i: usize) -> bool {
        if i == 0 || i >= self.rows.len() {
            return false;
        }
        self.rows.swap(i - 1, i);
        true
    }

    /// Move the row at `i` one place towards the bottom.
    pub fn move_down(&mut self, i: usize) -> bool {
        if i + 1 >= self.rows.len() {
            return false;
        }
        self.rows.swap(i, i + 1);
        true
    }
}

impl<'de> Deserialize<'de> for SlipLayout {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let rows = Vec::<SlipRow>::deserialize(d)?;
        // Filled against the task defaults; a reminder layout missing a
        // section gains it switched on or off to taste afterwards, which is
        // better than the section vanishing.
        Ok(SlipLayout { rows }.normalised(&SlipLayout::task()))
    }
}

/// A slip layout each for the two kinds of slip.
///
/// They are kept apart rather than folded into one list with a "which kind"
/// column, because the two want different *orders*, not only different
/// sections: a reminder leads with what is due, a receipt with what to do.
/// One shared order cannot say both.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SlipSet {
    pub task: SlipLayout,
    pub reminder: SlipLayout,
}

impl Default for SlipSet {
    fn default() -> Self {
        Self { task: SlipLayout::task(), reminder: SlipLayout::reminder() }
    }
}

impl SlipSet {
    pub fn for_kind(&self, kind: PrintJobKind) -> &SlipLayout {
        match kind {
            PrintJobKind::Task => &self.task,
            PrintJobKind::Reminder => &self.reminder,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_receipt_starts_from_the_documented_defaults() {
        let l = SlipLayout::task();
        assert_eq!(
            l.rows.iter().map(|r| r.section).collect::<Vec<_>>(),
            SlipSection::ALL.to_vec(),
            "every section, in the documented order"
        );
        let row = |s| l.get(s).unwrap();
        let custom = row(SlipSection::Custom);
        assert!(!custom.enabled);
        assert_eq!(custom.text, "Taskologic");

        let board = row(SlipSection::BoardTitle);
        assert!(board.enabled && board.bold && board.centered && !board.large);
        let title = row(SlipSection::TaskTitle);
        assert!(title.enabled && title.bold && title.large && !title.centered);
        let due = row(SlipSection::DueDate);
        assert!(due.enabled && due.large && due.centered);
        assert!(
            !row(SlipSection::StartDate).enabled,
            "a receipt is handed over as the work starts"
        );
        assert!(
            SlipLayout::reminder()
                .get(SlipSection::StartDate)
                .unwrap()
                .enabled,
            "a reminder may well be about the start date"
        );
        let deps = row(SlipSection::Dependencies);
        assert!(deps.enabled && deps.large && deps.centered);
        let stamp = row(SlipSection::Timestamp);
        assert!(stamp.enabled && stamp.bold && stamp.centered);

        assert!(row(SlipSection::Description).enabled);
        assert!(row(SlipSection::Checklist).enabled);
        assert!(!row(SlipSection::Creator).enabled);
        assert!(!row(SlipSection::Assignee).enabled);
    }

    #[test]
    fn the_two_kinds_carry_the_barcode_that_suits_them() {
        let task = SlipLayout::task();
        let rem = SlipLayout::reminder();
        // Nothing has been started when a reminder prints, so it offers the
        // code that starts it; a receipt offers the one that finishes.
        assert!(!task.get(SlipSection::BarcodeStart).unwrap().enabled);
        assert!(task.get(SlipSection::BarcodeFinish).unwrap().enabled);
        assert!(rem.get(SlipSection::BarcodeStart).unwrap().enabled);
        assert!(!rem.get(SlipSection::BarcodeFinish).unwrap().enabled);
    }

    #[test]
    fn the_two_layouts_are_edited_apart() {
        let mut set = SlipSet::default();
        set.reminder.move_up(3);
        set.reminder.rows[0].bold = true;
        assert_ne!(set.task, set.reminder, "one does not follow the other");
        assert_eq!(set.task, SlipLayout::task(), "the receipt is untouched");
        assert_eq!(set.for_kind(PrintJobKind::Task), &set.task);
        assert_eq!(set.for_kind(PrintJobKind::Reminder), &set.reminder);
    }

    #[test]
    fn a_layout_from_an_older_config_gains_what_it_never_heard_of() {
        // Someone's config with two sections in it, one of them twice.
        let partial = SlipLayout {
            rows: vec![
                SlipRow::new(SlipSection::TaskTitle, true, false, false, false),
                SlipRow::new(SlipSection::Timestamp, false, false, false, false),
                SlipRow::new(SlipSection::TaskTitle, false, true, true, true),
            ],
        }
        .normalised(&SlipLayout::task());
        assert_eq!(partial.rows.len(), SlipSection::ALL.len(), "nothing missing");
        assert_eq!(partial.rows[0].section, SlipSection::TaskTitle, "their order is kept");
        assert_eq!(partial.rows[1].section, SlipSection::Timestamp);
        assert!(
            partial.get(SlipSection::TaskTitle).unwrap().enabled,
            "the first spelling wins, not the repeat"
        );
        assert!(partial.get(SlipSection::BoardTitle).unwrap().enabled, "the rest arrive as defaults");
    }

    #[test]
    fn rows_move_up_and_down_and_stop_at_the_ends() {
        let mut l = SlipLayout::task();
        assert!(l.move_up(1));
        assert_eq!(l.rows[0].section, SlipSection::BoardTitle);
        assert_eq!(l.rows[1].section, SlipSection::Custom);
        assert!(l.move_down(0));
        assert_eq!(l.rows[0].section, SlipSection::Custom);
        assert!(!l.move_up(0), "the top row has nowhere to go");
        let last = l.rows.len() - 1;
        assert!(!l.move_down(last), "nor the bottom one");
        assert_eq!(l, SlipLayout::task(), "and nothing moved");
    }
}
