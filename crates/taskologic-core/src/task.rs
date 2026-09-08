//! Tasks. The record is deliberately flat and easy to extend, the README
//! promises more fields later.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::board::Board;
use crate::ids::{BoardId, ColumnId, ShortId, TaskId, Uid};
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
    /// Date and time, not just a date, because reminders are in hours.
    pub due_at: Option<DateTime<Utc>>,
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
    pub due_at: Option<DateTime<Utc>>,
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
            due_at: None,
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
}
