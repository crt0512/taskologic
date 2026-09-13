//! Tasks. The record is deliberately flat and easy to extend, the README
//! promises more fields later.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::board::Board;
use crate::ids::{BoardId, ColumnId, ShortId, TaskId, TemplateId, Uid};
use crate::repeat::RepeatSpec;

pub const MAX_TITLE_CHARS: usize = 200;

/// One line of a task's checklist.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistItem {
    pub text: String,
    #[serde(default)]
    pub done: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub short_id: ShortId,
    pub board_id: BoardId,
    pub column_id: ColumnId,
    /// Manual order inside the column.
    pub position: i64,
    pub title: String,
    pub description: String,
    /// When work on this is meant to begin. Independent of `due_at`: a task
    /// can carry one, both or neither.
    #[serde(default)]
    pub start_at: Option<DateTime<Utc>>,
    /// Date and time, not just a date, because reminders are in hours.
    pub due_at: Option<DateTime<Utc>>,
    /// Overrides the user's default lead time for the start reminder, for
    /// this task only. Minutes before `start_at`; None means the default.
    #[serde(default)]
    pub reminder_start_minutes: Option<u32>,
    /// The same for the reminder before `due_at`.
    #[serde(default)]
    pub reminder_due_minutes: Option<u32>,
    pub created_by: Uid,
    pub created_at: DateTime<Utc>,
    /// Set when the task enters the finished column, cleared when it leaves.
    /// The archive clock runs from here.
    pub finished_at: Option<DateTime<Utc>>,
    /// Bumped on every write. A stale version on save is rejected so two
    /// people editing at once do not clobber each other.
    pub version: u64,
    pub archived_at: Option<DateTime<Utc>>,
    pub archived_from_col: Option<ColumnId>,
    /// Set when the task was deleted rather than finished. Deleted tasks sit
    /// in the archive too and are purged after the board's retention period.
    pub deleted_at: Option<DateTime<Utc>>,
    pub assignees: Vec<Uid>,
    pub depends_on: Vec<TaskId>,
    /// Small todo items inside the task, ticked off from the task viewer.
    #[serde(default)]
    pub checklist: Vec<ChecklistItem>,
    pub repeat: Option<RepeatSpec>,
    /// The template this task was stamped out of, when it was one. Analytics
    /// groups sibling tasks by it. Tasks made before 0.1.11 carry none, so
    /// per template history starts there.
    #[serde(default)]
    pub template_id: Option<TemplateId>,
    /// Kept out of the analytics averages. One task that ran long for a
    /// reason of its own should not drag the estimate for all the others.
    #[serde(default)]
    pub exclude_from_stats: bool,
}

impl Task {
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }

    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }

    pub fn is_assignee(&self, uid: Uid) -> bool {
        self.assignees.contains(&uid)
    }

    pub fn is_creator(&self, uid: Uid) -> bool {
        self.created_by == uid
    }

    /// In the finished column, or archived. This is what "done" means for
    /// dependency checks and for repetition.
    pub fn is_done(&self, board: &Board) -> bool {
        self.is_archived() || self.column_id == board.finished_col
    }
}

/// What a client sends to create a task. Everything the daemon fills in
/// itself (ids, creator, timestamps, version) is absent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskDraft {
    pub title: String,
    pub description: String,
    pub start_at: Option<DateTime<Utc>>,
    pub due_at: Option<DateTime<Utc>>,
    /// Overrides the user's default lead time for the start reminder, for
    /// this task only. Minutes before `start_at`; None means the default.
    pub reminder_start_minutes: Option<u32>,
    /// The same for the reminder before `due_at`.
    pub reminder_due_minutes: Option<u32>,
    pub assignees: Vec<Uid>,
    pub depends_on: Vec<TaskId>,
    pub checklist: Vec<ChecklistItem>,
    pub repeat: Option<RepeatSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskError {
    #[error("title cannot be empty")]
    EmptyTitle,
    #[error("title is too long, {MAX_TITLE_CHARS} characters at most")]
    TitleTooLong,
    #[error("uid {0} is not a member of this board and cannot be assigned")]
    AssigneeNotMember(Uid),
    #[error("a reminder cannot be more than a year before the date it counts back from")]
    ReminderTooEarly,
    #[error("the start date cannot be after the due date")]
    StartAfterDue,
    #[error("{0}")]
    Repeat(#[from] crate::repeat::RepeatError),
}

pub fn validate_title(title: &str) -> Result<(), TaskError> {
    let t = title.trim();
    if t.is_empty() {
        return Err(TaskError::EmptyTitle);
    }
    if t.chars().count() > MAX_TITLE_CHARS {
        return Err(TaskError::TitleTooLong);
    }
    Ok(())
}

/// Checks everything that can be checked without the database. Dependency
/// existence and cycles are the daemon's job, see [`crate::deps`].
pub fn validate_draft(draft: &TaskDraft, board: &Board) -> Result<(), TaskError> {
    validate_title(&draft.title)?;
    for uid in &draft.assignees {
        if !board.is_member(*uid) {
            return Err(TaskError::AssigneeNotMember(*uid));
        }
    }
    let too_early = |m: &Option<u32>| m.is_some_and(|m| m > crate::prefs::MAX_REMINDER_MINUTES);
    if too_early(&draft.reminder_due_minutes) || too_early(&draft.reminder_start_minutes) {
        return Err(TaskError::ReminderTooEarly);
    }
    // A task that is due before it starts is a typo, not a plan. Either date
    // on its own is fine.
    if let (Some(start), Some(due)) = (draft.start_at, draft.due_at)
        && start > due
    {
        return Err(TaskError::StartAfterDue);
    }
    if let Some(r) = &draft.repeat {
        r.validate()?;
    }
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use super::*;

    /// A task in the board's first column, created by `creator`.
    pub fn task_on(board: &Board, creator: Uid) -> Task {
        Task {
            id: TaskId(100),
            short_id: ShortId::from_index(100),
            board_id: board.id,
            column_id: board.first_column().map(|c| c.id).unwrap_or(ColumnId(1)),
            position: 0,
            title: "Test task".into(),
            description: String::new(),
            start_at: None,
            due_at: None,
            reminder_start_minutes: None,
            reminder_due_minutes: None,
            created_by: creator,
            created_at: board.created_at,
            finished_at: None,
            version: 1,
            archived_at: None,
            archived_from_col: None,
            deleted_at: None,
            assignees: vec![],
            depends_on: vec![],
            checklist: vec![],
            repeat: None,
            template_id: None,
            exclude_from_stats: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::test_support::board_with_members;

    #[test]
    fn draft_validation() {
        let board = board_with_members(1, &[1, 2]);
        let mut d = TaskDraft {
            title: "Water plants".into(),
            ..Default::default()
        };
        assert_eq!(validate_draft(&d, &board), Ok(()));
        d.assignees = vec![2, 9];
        assert_eq!(
            validate_draft(&d, &board),
            Err(TaskError::AssigneeNotMember(9))
        );
        d.assignees.clear();
        d.title = " ".into();
        assert_eq!(validate_draft(&d, &board), Err(TaskError::EmptyTitle));
        d.title = "x".repeat(MAX_TITLE_CHARS + 1);
        assert_eq!(validate_draft(&d, &board), Err(TaskError::TitleTooLong));
    }

    #[test]
    fn a_reminder_override_is_capped_at_the_same_year_the_prefs_are() {
        let board = board_with_members(1, &[1]);
        let mut d = TaskDraft {
            title: "Water plants".into(),
            reminder_due_minutes: Some(crate::prefs::MAX_REMINDER_MINUTES),
            ..Default::default()
        };
        assert_eq!(validate_draft(&d, &board), Ok(()));
        d.reminder_due_minutes = Some(crate::prefs::MAX_REMINDER_MINUTES + 1);
        assert_eq!(validate_draft(&d, &board), Err(TaskError::ReminderTooEarly));

        // Both lead times answer to the same cap.
        d.reminder_due_minutes = None;
        d.reminder_start_minutes = Some(crate::prefs::MAX_REMINDER_MINUTES + 1);
        assert_eq!(validate_draft(&d, &board), Err(TaskError::ReminderTooEarly));
    }

    #[test]
    fn a_task_cannot_be_due_before_it_starts() {
        let board = board_with_members(1, &[1]);
        let early = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let late = DateTime::from_timestamp(1_800_003_600, 0).unwrap();
        let mut d = TaskDraft {
            title: "Water plants".into(),
            start_at: Some(early),
            due_at: Some(late),
            ..Default::default()
        };
        assert_eq!(validate_draft(&d, &board), Ok(()));
        // Starting exactly when it is due is pointless but not wrong.
        d.start_at = Some(late);
        assert_eq!(validate_draft(&d, &board), Ok(()));
        d.start_at = Some(late + chrono::TimeDelta::seconds(1));
        assert_eq!(validate_draft(&d, &board), Err(TaskError::StartAfterDue));
        // Either date alone is fine whatever the other would have been.
        d.due_at = None;
        assert_eq!(validate_draft(&d, &board), Ok(()));
        d.start_at = None;
        d.due_at = Some(early);
        assert_eq!(validate_draft(&d, &board), Ok(()));
    }
}
