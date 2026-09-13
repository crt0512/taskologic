//! The event log.
//!
//! Every column transition gets a row from day one, even though time tracking
//! is a future feature. Recording it now is nearly free, reconstructing it
//! later is not. It doubles as the audit log for "who deleted my task".

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::barcode::ScanAction;
use crate::ids::{BoardId, ColumnId, EventId, TaskId, TemplateId, Uid};
use crate::task::{Task, TaskDraft};

/// One field an edit touched. What the analytics detail view reads back, so
/// "who changed this and to what" has an answer months later.
///
/// Dates and the title carry their old and new values because they are short
/// and because they are the ones people argue about. The rest say only that
/// they changed: the log is a history, not a backup, and a description or a
/// checklist can be long enough to bloat every row that touches it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "field", rename_all = "snake_case")]
pub enum FieldChange {
    Title {
        from: String,
        to: String,
    },
    StartAt {
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
    },
    DueAt {
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
    },
    Description,
    Checklist,
    Assignees,
    Dependencies,
    Reminders,
    Repeat,
}

impl FieldChange {
    /// What the detail view calls this line.
    pub fn label(&self) -> &'static str {
        match self {
            FieldChange::Title { .. } => "title",
            FieldChange::StartAt { .. } => "start date",
            FieldChange::DueAt { .. } => "due date",
            FieldChange::Description => "description",
            FieldChange::Checklist => "checklist",
            FieldChange::Assignees => "assignees",
            FieldChange::Dependencies => "dependencies",
            FieldChange::Reminders => "reminder",
            FieldChange::Repeat => "repetition",
        }
    }
}

/// Which fields a save actually changes. An edit that touched nothing but
/// the cursor records nothing, so the history stays worth reading.
pub fn diff(task: &Task, draft: &TaskDraft) -> Vec<FieldChange> {
    let mut out = Vec::new();
    if task.title != draft.title.trim() {
        out.push(FieldChange::Title {
            from: task.title.clone(),
            to: draft.title.trim().to_string(),
        });
    }
    if task.start_at != draft.start_at {
        out.push(FieldChange::StartAt {
            from: task.start_at,
            to: draft.start_at,
        });
    }
    if task.due_at != draft.due_at {
        out.push(FieldChange::DueAt {
            from: task.due_at,
            to: draft.due_at,
        });
    }
    if task.description != draft.description {
        out.push(FieldChange::Description);
    }
    if task.checklist != draft.checklist {
        out.push(FieldChange::Checklist);
    }
    if task.assignees != draft.assignees {
        out.push(FieldChange::Assignees);
    }
    if task.depends_on != draft.depends_on {
        out.push(FieldChange::Dependencies);
    }
    if task.reminder_start_minutes != draft.reminder_start_minutes
        || task.reminder_due_minutes != draft.reminder_due_minutes
    {
        out.push(FieldChange::Reminders);
    }
    if task.repeat != draft.repeat {
        out.push(FieldChange::Repeat);
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    /// `column` is where it landed, which is the first thing a time tracking
    /// trail needs and which rows written before 0.1.11 do not carry.
    TaskCreated {
        #[serde(default)]
        column: Option<ColumnId>,
    },
    /// `changed` is empty for edits recorded before 0.1.11, which said only
    /// that somebody saved the task.
    TaskEdited {
        #[serde(default)]
        changed: Vec<FieldChange>,
    },
    /// Ticking one item off, kept apart from a general edit because it is
    /// the thing people do most and the least interesting to read as one.
    /// The text is copied in so the line still reads after a rename.
    ChecklistToggled {
        index: usize,
        text: String,
        done: bool,
    },
    TaskMoved {
        from: ColumnId,
        to: ColumnId,
    },
    TaskReordered,
    /// Moved to the archive by a delete. The row stays until purged.
    TaskDeleted,
    /// The row is gone for good: owner, admin or the retention scheduler.
    TaskPurged,
    TaskArchived {
        from: ColumnId,
    },
    TaskRestored {
        to: ColumnId,
    },
    /// Somebody finished a task with open dependencies on purpose.
    DependencyOverridden {
        open: Vec<TaskId>,
    },
    ScanApplied {
        action: ScanAction,
    },
    RepeatSpawned {
        from_task: TaskId,
    },
    /// A member turned the repetition off from the repeating tasks list.
    RepeatStopped,
    BoardCreated,
    BoardDeleted,
    BoardSettingsChanged,
    ColumnAdded {
        column: ColumnId,
    },
    ColumnRemoved {
        column: ColumnId,
        tasks_moved_to: Option<ColumnId>,
    },
    ColumnRenamed {
        column: ColumnId,
    },
    MemberAdded {
        uid: Uid,
    },
    MemberRemoved {
        uid: Uid,
        unassigned: Vec<TaskId>,
    },
    PrintJobDropped {
        reason: String,
    },
    TemplateCreated {
        template: TemplateId,
    },
    TemplateChanged {
        template: TemplateId,
    },
    TemplateDeleted {
        template: TemplateId,
    },
}

impl EventKind {
    /// Stable name for the `kind` column, independent of the JSON detail.
    pub fn name(&self) -> &'static str {
        match self {
            EventKind::TaskCreated { .. } => "task_created",
            EventKind::TaskEdited { .. } => "task_edited",
            EventKind::ChecklistToggled { .. } => "checklist_toggled",
            EventKind::TaskMoved { .. } => "task_moved",
            EventKind::TaskReordered => "task_reordered",
            EventKind::TaskDeleted => "task_deleted",
            EventKind::TaskPurged => "task_purged",
            EventKind::TaskArchived { .. } => "task_archived",
            EventKind::TaskRestored { .. } => "task_restored",
            EventKind::DependencyOverridden { .. } => "dependency_overridden",
            EventKind::ScanApplied { .. } => "scan_applied",
            EventKind::RepeatSpawned { .. } => "repeat_spawned",
            EventKind::RepeatStopped => "repeat_stopped",
            EventKind::BoardCreated => "board_created",
            EventKind::BoardDeleted => "board_deleted",
            EventKind::BoardSettingsChanged => "board_settings_changed",
            EventKind::ColumnAdded { .. } => "column_added",
            EventKind::ColumnRemoved { .. } => "column_removed",
            EventKind::ColumnRenamed { .. } => "column_renamed",
            EventKind::MemberAdded { .. } => "member_added",
            EventKind::MemberRemoved { .. } => "member_removed",
            EventKind::PrintJobDropped { .. } => "print_job_dropped",
            EventKind::TemplateCreated { .. } => "template_created",
            EventKind::TemplateChanged { .. } => "template_changed",
            EventKind::TemplateDeleted { .. } => "template_deleted",
        }
    }

    /// Source and destination column, for the indexed `from_col` and `to_col`
    /// columns that time tracking will query.
    pub fn columns(&self) -> (Option<ColumnId>, Option<ColumnId>) {
        match self {
            EventKind::TaskCreated { column } => (None, *column),
            EventKind::TaskMoved { from, to } => (Some(*from), Some(*to)),
            EventKind::TaskArchived { from } => (Some(*from), None),
            EventKind::TaskRestored { to } => (None, Some(*to)),
            _ => (None, None),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: EventId,
    pub board_id: BoardId,
    pub task_id: Option<TaskId>,
    /// None when the scheduler did it.
    pub actor_uid: Option<Uid>,
    pub kind: EventKind,
    pub at: DateTime<Utc>,
}
