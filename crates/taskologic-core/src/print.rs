//! Semantic print jobs.
//!
//! The daemon builds one of these. It says what to print, not how. A client
//! renders it for whatever printer it actually has. The daemon never produces
//! printer bytes, that boundary is what makes remote clients possible.

use chrono::{DateTime, TimeDelta, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::barcode::{ScanAction, ScanPayload};
use crate::board::Board;
use crate::ids::{ShortId, TaskId, Uid};
use crate::offset::Offset;
use crate::prefs::{PrintMode, PrintPrefs};
use crate::task::{ChecklistItem, Task};
use crate::user::User;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Symbology {
    #[default]
    Code39,
    Code128,
    Qr,
    DataMatrix,
}

impl Symbology {
    pub const ALL: [Symbology; 4] = [
        Symbology::Code39,
        Symbology::Code128,
        Symbology::Qr,
        Symbology::DataMatrix,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Symbology::Code39 => "CODE39",
            Symbology::Code128 => "CODE128",
            Symbology::Qr => "QR",
            Symbology::DataMatrix => "DataMatrix",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Barcode {
    pub action: ScanAction,
    pub payload: String,
    pub symbology: Symbology,
    /// What the line above the code says, when the action's own wording
    /// would be a lie. A sample on a test slip does not start anything.
    #[serde(default)]
    pub label: Option<String>,
    /// Draw a 1D code at one dot per module whatever the profile says: the
    /// thin half of a test strip, to learn what a scanner still reads.
    #[serde(default)]
    pub narrow: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrintJobKind {
    Task,
    Reminder,
    /// One slip for a whole group of tasks a program made at once: each with
    /// its start code, and the root's code that finishes whatever is running.
    Sheet,
    /// A card of control codes, or one task code on its own: a heading and
    /// a list of labelled barcodes, laid out fixed rather than by the
    /// printer's slip layout. See `crate::control`.
    Codes,
}

/// When one of a task's own print rules fires.
///
/// A task that carries rules decides its own printing: for that task alone
/// they stand in for every user's autoprint mode and reminder lead times.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrintWhen {
    /// The moment the task is made.
    OnCreate,
    /// When work begins: the start date arriving or the task entering the
    /// started column, whichever comes first, and only once either way.
    OnStart,
    /// So long before the due date. Follows the due date if it moves.
    BeforeDue(Offset),
}

impl PrintWhen {
    /// Stable name for the row that remembers a slip went out. It shares a
    /// table with the reminder kinds, so it must not collide with them.
    pub fn name(self) -> &'static str {
        match self {
            PrintWhen::OnCreate => "rule_create",
            PrintWhen::OnStart => "rule_start",
            PrintWhen::BeforeDue(_) => "rule_due",
        }
    }
}

/// Which slip a rule prints. What a kind looks like is the printer's own
/// layout, one per kind, so a rule picks the paper and never its shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlipKind {
    /// The receipt you work from, laid out around the code that finishes.
    #[default]
    Task,
    /// The nudge that says a thing is coming up, laid out around the code
    /// that starts it.
    Reminder,
    /// One slip for every task a fan-out step made at once, each with its
    /// start code, plus the root's code that finishes whatever is running.
    /// Only a step that fans out can ask for one, and only when created.
    Sheet,
}

impl SlipKind {
    pub const ALL: [SlipKind; 3] = [SlipKind::Task, SlipKind::Reminder, SlipKind::Sheet];

    pub fn label(self) -> &'static str {
        match self {
            SlipKind::Task => "task slip",
            SlipKind::Reminder => "reminder slip",
            SlipKind::Sheet => "group sheet",
        }
    }

    /// The stable half of a sent key, next to the moment.
    fn name(self) -> &'static str {
        match self {
            SlipKind::Task => "task",
            SlipKind::Reminder => "reminder",
            SlipKind::Sheet => "sheet",
        }
    }

    pub fn job_kind(self) -> PrintJobKind {
        match self {
            SlipKind::Task => PrintJobKind::Task,
            SlipKind::Reminder => PrintJobKind::Reminder,
            SlipKind::Sheet => PrintJobKind::Sheet,
        }
    }
}

/// Who a rule prints for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recipients {
    /// The task's assignees, or its creator while it has none.
    #[default]
    Assignees,
    Creator,
    /// These people, whether or not the task is theirs.
    Users(Vec<Uid>),
}

/// One print rule on a task: when, which slip, for whom.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrintRule {
    pub when: PrintWhen,
    #[serde(default)]
    pub slip: SlipKind,
    #[serde(default)]
    pub to: Recipients,
}

impl PrintRule {
    /// What a sent row for this rule is called, per person: the moment and
    /// the paper. Two rules that agree on both would hand over the same
    /// slip twice, so they share the name and the second finds it taken.
    pub fn sent_kind(&self) -> String {
        format!("{}_{}", self.when.name(), self.slip.name())
    }

    /// One readable line: "on start: reminder slip to assignees".
    pub fn summary(&self, name_of: &dyn Fn(Uid) -> String) -> String {
        let when = match self.when {
            PrintWhen::OnCreate => "when created".to_string(),
            PrintWhen::OnStart => "on start".to_string(),
            PrintWhen::BeforeDue(lead) => {
                format!("{} {} before due", lead.amount, lead.unit.label())
            }
        };
        let to = match &self.to {
            Recipients::Assignees => "assignees".to_string(),
            Recipients::Creator => "creator".to_string(),
            Recipients::Users(uids) => uids
                .iter()
                .map(|u| name_of(*u))
                .collect::<Vec<_>>()
                .join(", "),
        };
        format!("{when}: {} to {to}", self.slip.label())
    }
}

/// One dependency line on a slip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepLine {
    pub short_id: ShortId,
    pub title: String,
    pub done: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrintJob {
    pub kind: PrintJobKind,
    pub task_id: TaskId,
    pub short_id: ShortId,
    pub board_name: String,
    pub title: String,
    /// Everything the task has goes in the job, whether or not any given
    /// printer will show it. What reaches the paper is the printer's slip
    /// layout to decide, so that one panel answers for the shape of a slip
    /// and the daemon does not have to guess. Empty means the task has none.
    pub description: Option<String>,
    #[serde(default)]
    pub start_at: Option<DateTime<Utc>>,
    pub due_at: Option<DateTime<Utc>>,
    pub dependencies: Option<Vec<DepLine>>,
    pub created_by: Option<String>,
    pub assignees: Option<Vec<String>>,
    #[serde(default)]
    pub checklist: Vec<ChecklistItem>,
    pub barcodes: Vec<Barcode>,
    /// The tasks a group sheet lists, each with its own start code. Empty
    /// on every other kind of slip.
    #[serde(default)]
    pub sheet: Vec<SheetEntry>,
    /// The codes a codes card carries, in order. Empty on every other kind.
    #[serde(default)]
    pub codes: Vec<CodeLine>,
    /// For rendering timestamps. The job follows the user, so does the zone.
    pub timezone: Tz,
    pub created_at: DateTime<Utc>,
}

/// One line of a group sheet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SheetEntry {
    pub short_id: ShortId,
    pub title: String,
    pub barcode: Barcode,
}

/// One code on a codes card: what the line above it says, and the code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeLine {
    pub label: String,
    pub payload: String,
    pub symbology: Symbology,
    /// One dot per module, for the thin half of a test strip.
    #[serde(default)]
    pub narrow: bool,
}

impl CodeLine {
    /// The barcode the layout draws for this line. Codes cards carry no
    /// task action; the label is the whole story.
    pub fn barcode(&self) -> Barcode {
        Barcode {
            action: ScanAction::StartPause,
            payload: self.payload.clone(),
            symbology: self.symbology,
            label: Some(self.label.clone()),
            narrow: self.narrow,
        }
    }
}

/// A codes card: `heading` at the top, then every code with its label.
/// Built and printed by the client on its own, no task behind it.
pub fn build_codes_job(heading: &str, codes: Vec<CodeLine>, user: &User, now: DateTime<Utc>) -> PrintJob {
    PrintJob {
        kind: PrintJobKind::Codes,
        task_id: TaskId(0),
        short_id: ShortId::from_index(0),
        board_name: heading.to_string(),
        title: heading.to_string(),
        description: None,
        start_at: None,
        due_at: None,
        dependencies: None,
        created_by: None,
        assignees: None,
        checklist: Vec::new(),
        barcodes: Vec::new(),
        sheet: Vec::new(),
        codes,
        timezone: user.timezone,
        created_at: now,
    }
}

fn barcode(user: &User, task: &Task, action: ScanAction) -> Barcode {
    let payload = ScanPayload {
        action,
        short_id: task.short_id,
    }
    .encode();
    Barcode {
        action,
        payload,
        symbology: user.prefs.scanner.format,
        label: None,
        narrow: false,
    }
}

/// A barcode that stands for nothing, to prove a printer can draw one.
/// Test slips carry one whatever the scanner prefs say, because the point of
/// a test print is the printer, not the prefs.
pub fn sample_barcode(symbology: Symbology, action: ScanAction) -> Barcode {
    Barcode {
        // Nothing scans this into anything; the label says as much.
        action,
        payload: SAMPLE_BARCODE_PAYLOAD.to_string(),
        symbology,
        label: Some("sample barcode".to_string()),
        narrow: false,
    }
}

/// The payload on a test slip's barcode. Digits only, so every symbology can
/// carry it, and recognisable enough to check against what a scanner reads.
pub const SAMPLE_BARCODE_PAYLOAD: &str = "123456789";

/// The codes a slip for this task carries: one to start it, and either one
/// to finish it or, when a program gave the task a question, one per answer
/// in the plain finish code's place. Answering finishes the task too.
fn barcodes_for(user: &User, task: &Task) -> Vec<Barcode> {
    let mut codes = vec![barcode(user, task, ScanAction::StartPause)];
    match task.program.as_ref().and_then(|p| p.question.as_ref()) {
        Some(q) => {
            for (i, answer) in q.answers().iter().enumerate() {
                let mut code = barcode(user, task, q.barcode_action(i));
                code.label = Some(format!("scan to finish: {answer}"));
                codes.push(code);
            }
        }
        None => codes.push(barcode(user, task, ScanAction::Finish)),
    }
    codes
}

/// A full task slip. `names` resolves uids to display names, `deps` is the
/// task's dependency list with their current state.
pub fn build_task_job(
    task: &Task,
    board: &Board,
    deps: &[DepLine],
    names: &dyn Fn(Uid) -> String,
    user: &User,
    now: DateTime<Utc>,
) -> PrintJob {
    let barcodes = barcodes_for(user, task);
    PrintJob {
        kind: PrintJobKind::Task,
        task_id: task.id,
        short_id: task.short_id,
        board_name: board.name.clone(),
        title: task.title.clone(),
        description: (!task.description.trim().is_empty()).then(|| task.description.clone()),
        start_at: task.start_at,
        due_at: task.due_at,
        dependencies: (!deps.is_empty()).then(|| deps.to_vec()),
        created_by: Some(names(task.created_by)),
        assignees: (!task.assignees.is_empty())
            .then(|| task.assignees.iter().map(|u| names(*u)).collect()),
        checklist: task.checklist.clone(),
        barcodes,
        sheet: Vec::new(),
        codes: Vec::new(),
        timezone: user.timezone,
        created_at: now,
    }
}

/// A group sheet: the run's root with, under it, every task in `group`
/// and its start code, and the root's code that finishes whatever of the
/// run is running. One piece of paper for a fan-out step.
pub fn build_sheet_job(
    root: &Task,
    group: &[Task],
    board: &Board,
    user: &User,
    now: DateTime<Utc>,
) -> PrintJob {
    let mut stop = barcode(user, root, ScanAction::FinishChildren);
    stop.label = Some("scan to finish what is running".to_string());
    PrintJob {
        kind: PrintJobKind::Sheet,
        task_id: root.id,
        short_id: root.short_id,
        board_name: board.name.clone(),
        title: root.title.clone(),
        description: None,
        start_at: None,
        due_at: None,
        dependencies: None,
        created_by: None,
        assignees: None,
        checklist: Vec::new(),
        barcodes: vec![stop],
        sheet: group
            .iter()
            .map(|t| {
                let mut code = barcode(user, t, ScanAction::StartPause);
                code.label = Some(format!("scan to start: {}", t.title));
                SheetEntry {
                    short_id: t.short_id,
                    title: t.title.clone(),
                    barcode: code,
                }
            })
            .collect(),
        codes: Vec::new(),
        timezone: user.timezone,
        created_at: now,
    }
}

/// A reminder slip. It carries the same fields a task slip does, for the
/// same reason: the printer's layout decides what a reminder shows, and it
/// cannot show what was never sent.
pub fn build_reminder_job(task: &Task, board: &Board, user: &User, now: DateTime<Utc>) -> PrintJob {
    let barcodes = barcodes_for(user, task);
    PrintJob {
        kind: PrintJobKind::Reminder,
        task_id: task.id,
        short_id: task.short_id,
        board_name: board.name.clone(),
        title: task.title.clone(),
        description: (!task.description.trim().is_empty()).then(|| task.description.clone()),
        start_at: task.start_at,
        due_at: task.due_at,
        dependencies: None,
        created_by: None,
        assignees: None,
        checklist: task.checklist.clone(),
        barcodes,
        sheet: Vec::new(),
        codes: Vec::new(),
        timezone: user.timezone,
        created_at: now,
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AutoprintTrigger {
    AddedToBoard,
    MovedToStarted,
}

/// Whether this user's prefs ask for an automatic print on this trigger.
pub fn wants_autoprint(
    prefs: &PrintPrefs,
    trigger: AutoprintTrigger,
    task: &Task,
    uid: Uid,
) -> bool {
    let mode_matches = matches!(
        (prefs.mode, trigger),
        (PrintMode::OnAdd, AutoprintTrigger::AddedToBoard)
            | (PrintMode::OnStart, AutoprintTrigger::MovedToStarted)
    );
    if !mode_matches {
        return false;
    }
    let Some(filter) = &prefs.autoprint_filter else {
        return true;
    };
    let assigned = task.is_assignee(uid);
    let created = task.is_creator(uid);
    let by_assignment = filter.assigned_to_me && assigned;
    let by_creation =
        filter.created_by_me && created && (!filter.unless_not_assigned_to_me || assigned);
    by_assignment || by_creation
}

/// Which of a task's two dates a reminder counts back from.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReminderKind {
    Start,
    Due,
}

impl ReminderKind {
    pub const ALL: [ReminderKind; 2] = [ReminderKind::Start, ReminderKind::Due];

    /// Stable name for the column that remembers a slip was sent.
    pub fn name(self) -> &'static str {
        match self {
            ReminderKind::Start => "start",
            ReminderKind::Due => "due",
        }
    }
}

/// When a reminder should print for this task. A task can override the
/// user's default lead time, and an override works even when the user has no
/// default, because ticking it on one task is a deliberate act.
pub fn reminder_at(task: &Task, prefs: &PrintPrefs, kind: ReminderKind) -> Option<DateTime<Utc>> {
    let (minutes, anchor) = match kind {
        ReminderKind::Start => (
            task.reminder_start_minutes.or(prefs.reminder_start_minutes),
            task.start_at,
        ),
        ReminderKind::Due => (
            task.reminder_due_minutes.or(prefs.reminder_due_minutes),
            task.due_at,
        ),
    };
    anchor?.checked_sub_signed(TimeDelta::minutes(i64::from(minutes?)))
}

/// One slip a task owes somebody: when it prints, which date it counts back
/// from, and which kind of slip it is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PlannedReminder {
    pub kind: ReminderKind,
    pub at: DateTime<Utc>,
    pub job: PrintJobKind,
}

/// Everything this task owes this user, whether or not the time has come.
/// The scheduler filters by the clock, by what it already sent and by
/// whether the task was ever started; the shape of the answer is decided
/// here, where it can be tested without a database.
///
/// A task with neither date owes nothing, which is why a task nobody dated
/// stays quiet until somebody starts it. With one date it owes a reminder
/// before that date. With both it owes a reminder before the start and then
/// the task slip before the due date, because by then the point is not
/// "this is coming up" but "here is the paper you finish with".
pub fn plan_reminders(task: &Task, prefs: &PrintPrefs) -> Vec<PlannedReminder> {
    let both_dates = task.start_at.is_some() && task.due_at.is_some();
    let mut out = Vec::new();
    if let Some(at) = reminder_at(task, prefs, ReminderKind::Start) {
        out.push(PlannedReminder {
            kind: ReminderKind::Start,
            at,
            job: PrintJobKind::Reminder,
        });
    }
    if let Some(at) = reminder_at(task, prefs, ReminderKind::Due) {
        out.push(PlannedReminder {
            kind: ReminderKind::Due,
            at,
            job: if both_dates {
                PrintJobKind::Task
            } else {
                PrintJobKind::Reminder
            },
        });
    }
    out
}

/// Who a rule's slip goes to, among the board's members, each once.
/// Assignees fall back to the creator when there are none, and anyone who
/// has left the board since the rule was written is dropped rather than
/// printed for.
pub fn recipients_of(rule: &PrintRule, task: &Task, board: &Board) -> Vec<Uid> {
    let mut uids: Vec<Uid> = match &rule.to {
        Recipients::Assignees if task.assignees.is_empty() => vec![task.created_by],
        Recipients::Assignees => task.assignees.clone(),
        Recipients::Creator => vec![task.created_by],
        Recipients::Users(uids) => uids.clone(),
    };
    uids.retain(|u| board.is_member(*u));
    uids.sort_unstable();
    uids.dedup();
    uids
}

/// One of a task's rules whose moment is a date rather than an event: which
/// rule, when it prints, the date it counts to, and the key it is remembered
/// under once sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedRule {
    pub index: usize,
    pub at: DateTime<Utc>,
    /// The date past which the slip is not worth printing, when there is
    /// one. A "before due" slip is not, once the due date has gone by. A
    /// start slip is the paper you work from and waits as long as the task
    /// does, so it has none.
    pub anchor: Option<DateTime<Utc>>,
    /// What a sent row is keyed on. The due date for "before due", so moving
    /// the date earns a fresh slip; the creation time for "on start", so the
    /// date path and the move into the started column agree on one slip.
    pub sent_anchor: DateTime<Utc>,
}

/// The dated rules of a task: "on start" once it has a start date, "before
/// due" once it has a due date. The event driven moments, creation and
/// entering the started column, are fired where those things happen.
pub fn plan_rule_prints(task: &Task) -> Vec<PlannedRule> {
    task.print_rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| {
            let (at, anchor, sent_anchor) = match rule.when {
                PrintWhen::OnCreate => return None,
                PrintWhen::OnStart => {
                    let start = task.start_at?;
                    (start, None, task.created_at)
                }
                PrintWhen::BeforeDue(lead) => {
                    let due = task.due_at?;
                    (due.checked_sub_signed(lead.delta()?)?, Some(due), due)
                }
            };
            Some(PlannedRule {
                index,
                at,
                anchor,
                sent_anchor,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::test_support::board_with_members;
    use crate::prefs::{AutoprintFilter, UserPrefs};
    use crate::task::test_support::task_on;

    fn user(uid: Uid, prefs: UserPrefs) -> User {
        User {
            uid,
            username: format!("user{uid}"),
            is_admin: false,
            timezone: chrono_tz::UTC,
            prefs,
            has_pin: false,
            created_at: DateTime::from_timestamp(0, 0).unwrap(),
        }
    }

    #[test]
    fn task_job_honours_prefs_and_scanner_state() {
        let board = board_with_members(1, &[1, 2]);
        let mut task = task_on(&board, 1);
        task.description = "Two lines\nof text".into();
        task.assignees = vec![2];
        let names = |u: Uid| format!("user{u}");
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();

        // Everything the task has goes in the job. What a printer shows is
        // its own slip layout's business, so nothing here is left out on a
        // preference's say so.
        let prefs = UserPrefs::default();
        let job = build_task_job(&task, &board, &[], &names, &user(1, prefs.clone()), now);
        assert_eq!(job.description.as_deref(), Some("Two lines\nof text"));
        assert_eq!(job.assignees, Some(vec!["user2".to_string()]));
        assert_eq!(
            job.created_by.as_deref(),
            Some("user1"),
            "sent even though few slips show it"
        );
        assert_eq!(
            job.dependencies, None,
            "a task with no dependencies has none to send"
        );
        assert_eq!(job.checklist, task.checklist);

        // Barcodes ride along whatever the scanner prefs say; the layout
        // decides whether either of them is printed.
        assert_eq!(job.barcodes.len(), 2, "one to start, one to finish");
        assert_eq!(job.barcodes[0].action, ScanAction::StartPause);
        assert_eq!(job.barcodes[1].action, ScanAction::Finish);
        assert!(job.barcodes[0].payload.starts_with("..1S"));
    }

    #[test]
    fn a_reminder_carries_the_same_fields_a_task_slip_does() {
        let board = board_with_members(1, &[1]);
        let task = task_on(&board, 1);
        let mut prefs = UserPrefs::default();
        prefs.ui.scanner_enabled = true;
        let job = build_reminder_job(
            &task,
            &board,
            &user(1, prefs),
            DateTime::from_timestamp(0, 0).unwrap(),
        );
        assert_eq!(job.kind, PrintJobKind::Reminder);
        // A reminder is not a different document, it is the same one with a
        // different layout in front of it, so it carries the same fields.
        assert_eq!(
            job.barcodes.len(),
            2,
            "start and finish, as a task slip has"
        );
        assert!(job.barcodes[0].payload.starts_with("..1S"));
        assert_eq!(job.due_at, task.due_at);
        assert_eq!(job.checklist, task.checklist);
    }

    #[test]
    fn autoprint_filtering() {
        let board = board_with_members(1, &[1, 2, 3]);
        let mut task = task_on(&board, 1);
        task.assignees = vec![2];
        let mut prefs = PrintPrefs {
            mode: PrintMode::OnAdd,
            ..Default::default()
        };

        assert!(wants_autoprint(
            &prefs,
            AutoprintTrigger::AddedToBoard,
            &task,
            3
        ));
        assert!(!wants_autoprint(
            &prefs,
            AutoprintTrigger::MovedToStarted,
            &task,
            3
        ));

        prefs.autoprint_filter = Some(AutoprintFilter {
            assigned_to_me: true,
            created_by_me: false,
            unless_not_assigned_to_me: false,
        });
        assert!(wants_autoprint(
            &prefs,
            AutoprintTrigger::AddedToBoard,
            &task,
            2
        ));
        assert!(!wants_autoprint(
            &prefs,
            AutoprintTrigger::AddedToBoard,
            &task,
            1
        ));

        prefs.autoprint_filter = Some(AutoprintFilter {
            assigned_to_me: false,
            created_by_me: true,
            unless_not_assigned_to_me: false,
        });
        assert!(wants_autoprint(
            &prefs,
            AutoprintTrigger::AddedToBoard,
            &task,
            1
        ));
        assert!(!wants_autoprint(
            &prefs,
            AutoprintTrigger::AddedToBoard,
            &task,
            2
        ));

        prefs.autoprint_filter = Some(AutoprintFilter {
            assigned_to_me: false,
            created_by_me: true,
            unless_not_assigned_to_me: true,
        });
        assert!(
            !wants_autoprint(&prefs, AutoprintTrigger::AddedToBoard, &task, 1),
            "created by me but not assigned to me"
        );
        task.assignees.push(1);
        assert!(wants_autoprint(
            &prefs,
            AutoprintTrigger::AddedToBoard,
            &task,
            1
        ));

        prefs.mode = PrintMode::Manual;
        assert!(!wants_autoprint(
            &prefs,
            AutoprintTrigger::AddedToBoard,
            &task,
            1
        ));
    }

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn reminder_time() {
        let board = board_with_members(1, &[1]);
        let mut task = task_on(&board, 1);
        let prefs = PrintPrefs {
            reminder_due_minutes: Some(120),
            ..Default::default()
        };
        assert_eq!(reminder_at(&task, &prefs, ReminderKind::Due), None);
        task.due_at = Some(at(10_000));
        assert_eq!(
            reminder_at(&task, &prefs, ReminderKind::Due),
            Some(at(10_000 - 7200))
        );
        assert_eq!(
            reminder_at(&task, &PrintPrefs::default(), ReminderKind::Due),
            None
        );
    }

    #[test]
    fn the_two_lead_times_count_back_from_their_own_date() {
        let board = board_with_members(1, &[1]);
        let mut task = task_on(&board, 1);
        task.start_at = Some(at(10_000));
        task.due_at = Some(at(20_000));
        let prefs = PrintPrefs {
            reminder_start_minutes: Some(30),
            reminder_due_minutes: Some(120),
            ..Default::default()
        };
        assert_eq!(
            reminder_at(&task, &prefs, ReminderKind::Start),
            Some(at(10_000 - 1800))
        );
        assert_eq!(
            reminder_at(&task, &prefs, ReminderKind::Due),
            Some(at(20_000 - 7200))
        );

        // One lead time set and not the other leaves that date silent.
        let only_start = PrintPrefs {
            reminder_start_minutes: Some(30),
            ..Default::default()
        };
        assert!(reminder_at(&task, &only_start, ReminderKind::Start).is_some());
        assert_eq!(reminder_at(&task, &only_start, ReminderKind::Due), None);
    }

    #[test]
    fn a_tasks_own_lead_time_beats_the_default_and_stands_without_one() {
        let board = board_with_members(1, &[1]);
        let mut task = task_on(&board, 1);
        task.due_at = Some(at(10_000));
        task.reminder_due_minutes = Some(30);
        let prefs = PrintPrefs {
            reminder_due_minutes: Some(120),
            ..Default::default()
        };
        assert_eq!(
            reminder_at(&task, &prefs, ReminderKind::Due),
            Some(at(10_000 - 1800))
        );
        assert_eq!(
            reminder_at(&task, &PrintPrefs::default(), ReminderKind::Due),
            Some(at(10_000 - 1800)),
            "ticking it on one task is a deliberate act, default or not"
        );

        // The start override is its own switch and does not borrow the due one.
        task.start_at = Some(at(5_000));
        assert_eq!(
            reminder_at(&task, &PrintPrefs::default(), ReminderKind::Start),
            None
        );
        task.reminder_start_minutes = Some(15);
        assert_eq!(
            reminder_at(&task, &PrintPrefs::default(), ReminderKind::Start),
            Some(at(5_000 - 900))
        );
    }

    #[test]
    fn a_task_with_no_dates_owes_nothing() {
        let board = board_with_members(1, &[1]);
        let task = task_on(&board, 1);
        let prefs = PrintPrefs {
            reminder_start_minutes: Some(30),
            reminder_due_minutes: Some(6),
            ..Default::default()
        };
        assert_eq!(plan_reminders(&task, &prefs), vec![]);
    }

    #[test]
    fn one_date_prints_a_reminder_and_two_print_the_slip_you_finish_with() {
        let board = board_with_members(1, &[1]);
        let prefs = PrintPrefs {
            reminder_start_minutes: Some(30),
            reminder_due_minutes: Some(6),
            ..Default::default()
        };

        // Only a start date: one reminder, before the start.
        let mut task = task_on(&board, 1);
        task.start_at = Some(at(10_000));
        assert_eq!(
            plan_reminders(&task, &prefs),
            vec![PlannedReminder {
                kind: ReminderKind::Start,
                at: at(10_000 - 1800),
                job: PrintJobKind::Reminder,
            }]
        );

        // Only a due date: one reminder, before the due date.
        let mut task = task_on(&board, 1);
        task.due_at = Some(at(20_000));
        assert_eq!(
            plan_reminders(&task, &prefs),
            vec![PlannedReminder {
                kind: ReminderKind::Due,
                at: at(20_000 - 360),
                job: PrintJobKind::Reminder,
            }]
        );

        // Both: a reminder to start, then the task slip to work from.
        task.start_at = Some(at(10_000));
        assert_eq!(
            plan_reminders(&task, &prefs),
            vec![
                PlannedReminder {
                    kind: ReminderKind::Start,
                    at: at(10_000 - 1800),
                    job: PrintJobKind::Reminder,
                },
                PlannedReminder {
                    kind: ReminderKind::Due,
                    at: at(20_000 - 360),
                    job: PrintJobKind::Task,
                },
            ]
        );
    }

    fn minutes(n: u32) -> Offset {
        Offset {
            amount: n,
            unit: crate::offset::OffsetUnit::Minutes,
        }
    }

    #[test]
    fn a_rules_recipients_are_members_each_once_and_assignees_fall_back_to_the_creator() {
        let board = board_with_members(1, &[1, 2, 3]);
        let mut task = task_on(&board, 1);
        let rule = |to: Recipients| PrintRule {
            when: PrintWhen::OnCreate,
            slip: SlipKind::Task,
            to,
        };
        // Nobody assigned: the creator gets it.
        assert_eq!(recipients_of(&rule(Recipients::Assignees), &task, &board), vec![1]);
        task.assignees = vec![3, 2, 3];
        assert_eq!(recipients_of(&rule(Recipients::Assignees), &task, &board), vec![2, 3]);
        assert_eq!(recipients_of(&rule(Recipients::Creator), &task, &board), vec![1]);
        // Somebody who is not on the board any more is left out, not printed for.
        assert_eq!(
            recipients_of(&rule(Recipients::Users(vec![9, 2, 2])), &task, &board),
            vec![2]
        );
    }

    #[test]
    fn dated_rules_are_planned_from_the_dates_the_task_has() {
        let board = board_with_members(1, &[1]);
        let mut task = task_on(&board, 1);
        task.print_rules = vec![
            PrintRule {
                when: PrintWhen::OnCreate,
                slip: SlipKind::Task,
                to: Recipients::Assignees,
            },
            PrintRule {
                when: PrintWhen::OnStart,
                slip: SlipKind::Reminder,
                to: Recipients::Assignees,
            },
            PrintRule {
                when: PrintWhen::BeforeDue(minutes(10)),
                slip: SlipKind::Task,
                to: Recipients::Creator,
            },
        ];
        // No dates: nothing the clock can decide, and creation is not its job.
        assert_eq!(plan_rule_prints(&task), vec![]);

        task.due_at = Some(at(20_000));
        assert_eq!(
            plan_rule_prints(&task),
            vec![PlannedRule {
                index: 2,
                at: at(20_000 - 600),
                anchor: Some(at(20_000)),
                sent_anchor: at(20_000),
            }]
        );

        // The start rule is keyed on the creation time, not the start date,
        // so a slip printed by moving the task counts for this one too.
        task.start_at = Some(at(10_000));
        let planned = plan_rule_prints(&task);
        assert_eq!(planned.len(), 2);
        assert_eq!(
            planned[0],
            PlannedRule {
                index: 1,
                at: at(10_000),
                anchor: None,
                sent_anchor: task.created_at,
            }
        );
    }

    #[test]
    fn rules_read_back_as_a_sentence_and_share_a_sent_key_when_they_agree() {
        let names = |u: Uid| format!("user{u}");
        let a = PrintRule {
            when: PrintWhen::BeforeDue(minutes(10)),
            slip: SlipKind::Task,
            to: Recipients::Users(vec![2, 3]),
        };
        assert_eq!(
            a.summary(&names),
            "10 minutes before due: task slip to user2, user3"
        );
        assert_eq!(a.sent_kind(), "rule_due_task");
        let b = PrintRule {
            when: PrintWhen::BeforeDue(minutes(30)),
            slip: SlipKind::Task,
            to: Recipients::Assignees,
        };
        assert_eq!(b.sent_kind(), a.sent_kind(), "same moment, same paper");
        let c = PrintRule {
            when: PrintWhen::OnStart,
            slip: SlipKind::Reminder,
            to: Recipients::Assignees,
        };
        assert_eq!(c.summary(&names), "on start: reminder slip to assignees");
        assert_eq!(c.sent_kind(), "rule_start_reminder");
    }

    #[test]
    fn a_rule_on_the_wire_is_short_and_old_rows_without_the_extras_still_load() {
        let rule = PrintRule {
            when: PrintWhen::BeforeDue(minutes(10)),
            slip: SlipKind::Reminder,
            to: Recipients::Users(vec![3]),
        };
        let json = serde_json::to_string(&rule).unwrap();
        assert_eq!(
            json,
            r#"{"when":{"before_due":{"amount":10,"unit":"minutes"}},"slip":"reminder","to":{"users":[3]}}"#
        );
        let back: PrintRule = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rule);
        // Only the moment is required; the rest has a sensible default.
        let bare: PrintRule = serde_json::from_str(r#"{"when":"on_start"}"#).unwrap();
        assert_eq!(bare.when, PrintWhen::OnStart);
        assert_eq!(bare.slip, SlipKind::Task);
        assert_eq!(bare.to, Recipients::Assignees);
    }
}
