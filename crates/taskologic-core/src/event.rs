//! The event log.
//!
//! Every column transition gets a row from day one, even though time tracking
//! is a future feature. Recording it now is nearly free, reconstructing it
//! later is not. It doubles as the audit log for "who deleted my task".

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::barcode::ScanAction;
use crate::ids::{BoardId, ColumnId, EventId, TaskId, TemplateId, Uid};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    TaskCreated,
    TaskEdited,
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
            EventKind::TaskCreated => "task_created",
            EventKind::TaskEdited => "task_edited",
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
