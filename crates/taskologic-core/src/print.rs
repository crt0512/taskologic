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
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrintJobKind {
    Task,
    Reminder,
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
    /// For rendering timestamps. The job follows the user, so does the zone.
    pub timezone: Tz,
    pub created_at: DateTime<Utc>,
}

fn barcode(user: &User, task: &Task, action: ScanAction) -> Barcode {
    let payload = ScanPayload {
        action,
        short_id: task.short_id,
    }
    .encode(user.prefs.scanner.magic);
    Barcode {
        action,
        payload,
        symbology: user.prefs.scanner.format,
        label: None,
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
    }
}

/// The payload on a test slip's barcode. Digits only, so every symbology can
/// carry it, and recognisable enough to check against what a scanner reads.
pub const SAMPLE_BARCODE_PAYLOAD: &str = "123456789";

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
    let barcodes = vec![
        barcode(user, task, ScanAction::StartPause),
        barcode(user, task, ScanAction::Finish),
    ];
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
        timezone: user.timezone,
        created_at: now,
    }
}

/// A reminder slip. It carries the same fields a task slip does, for the
/// same reason: the printer's layout decides what a reminder shows, and it
/// cannot show what was never sent.
pub fn build_reminder_job(task: &Task, board: &Board, user: &User, now: DateTime<Utc>) -> PrintJob {
    let barcodes = vec![
        barcode(user, task, ScanAction::StartPause),
        barcode(user, task, ScanAction::Finish),
    ];
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
        prefs.scanner.magic = crate::barcode::Magic::Dashes;
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
        assert!(job.barcodes[0].payload.starts_with("--1S"));
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
}
