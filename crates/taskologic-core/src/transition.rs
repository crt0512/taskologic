//! Column transitions: what moving a task means, what a scan does, and when
//! the archive clock fires. All decisions, no side effects. The daemon
//! applies the plan and records the events.

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crate::barcode::ScanAction;
use crate::board::Board;
use crate::ids::{ColumnId, TaskId};
use crate::task::Task;

/// Everything the daemon needs to apply a column move.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovePlan {
    pub from: ColumnId,
    pub to: ColumnId,
    /// New value for `Task::finished_at`.
    pub finished_at: Option<DateTime<Utc>>,
    /// Dependencies that were still open and got overridden. Non empty means
    /// a `DependencyOverridden` event must be recorded.
    pub overrode_deps: Vec<TaskId>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum MoveError {
    #[error("column {0} is not on this board")]
    UnknownColumn(ColumnId),
    #[error("archived tasks cannot be moved, restore it first")]
    TaskArchived,
    /// The client shows these and offers an override.
    #[error("{} dependencies are still open", .open.len())]
    BlockedByDependencies { open: Vec<TaskId> },
}

/// Decide a move. `open_deps` is the subset of the task's dependencies that
/// are not done, which the daemon computes because it has the other tasks.
pub fn plan_move(
    board: &Board,
    task: &Task,
    to: ColumnId,
    open_deps: &[TaskId],
    override_deps: bool,
    now: DateTime<Utc>,
) -> Result<MovePlan, MoveError> {
    if task.is_archived() {
        return Err(MoveError::TaskArchived);
    }
    if !board.has_column(to) {
        return Err(MoveError::UnknownColumn(to));
    }
    let from = task.column_id;
    let entering_finished = to == board.finished_col && from != board.finished_col;
    let leaving_finished = from == board.finished_col && to != board.finished_col;

    let mut overrode_deps = Vec::new();
    if entering_finished && !open_deps.is_empty() {
        if !override_deps {
            return Err(MoveError::BlockedByDependencies {
                open: open_deps.to_vec(),
            });
        }
        overrode_deps = open_deps.to_vec();
    }

    let finished_at = if entering_finished {
        Some(now)
    } else if leaving_finished {
        None
    } else {
        task.finished_at
    };

    Ok(MovePlan {
        from,
        to,
        finished_at,
        overrode_deps,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "refusal", rename_all = "snake_case")]
pub enum ScanRefusal {
    #[error("task is already finished")]
    AlreadyFinished,
    #[error("task is archived")]
    Archived,
}

/// Which column a scan wants the task in. The move itself still goes through
/// [`plan_move`] so finishing by barcode runs the dependency check too.
pub fn scan_target(
    action: ScanAction,
    board: &Board,
    task: &Task,
) -> Result<ColumnId, ScanRefusal> {
    if task.is_archived() {
        return Err(ScanRefusal::Archived);
    }
    if task.column_id == board.finished_col {
        return Err(ScanRefusal::AlreadyFinished);
    }
    Ok(match action {
        ScanAction::StartPause if task.column_id == board.started_col => board.paused_col,
        ScanAction::StartPause => board.started_col,
        ScanAction::Finish => board.finished_col,
    })
}

/// When the task should be archived, if the clock is running.
pub fn archive_due_at(board: &Board, task: &Task) -> Option<DateTime<Utc>> {
    if task.is_archived() || task.column_id != board.finished_col {
        return None;
    }
    let finished = task.finished_at?;
    finished.checked_add_signed(TimeDelta::seconds(board.archive_after_secs))
}

/// When a deleted task is removed for good, if it was deleted.
pub fn purge_due_at(board: &Board, task: &Task) -> Option<DateTime<Utc>> {
    let deleted = task.deleted_at?;
    deleted.checked_add_signed(TimeDelta::seconds(board.purge_deleted_after_secs))
}

/// Where a restored task goes: back where it was, or the first column if
/// that column is gone.
pub fn restore_column(board: &Board, task: &Task) -> Option<ColumnId> {
    match task.archived_from_col {
        Some(c) if board.has_column(c) => Some(c),
        _ => board.first_column().map(|c| c.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::test_support::board_with_members;
    use crate::task::test_support::task_on;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    #[test]
    fn finishing_starts_the_archive_clock_and_leaving_stops_it() {
        let board = board_with_members(1, &[1]);
        let mut task = task_on(&board, 1);
        let plan = plan_move(&board, &task, board.finished_col, &[], false, now()).unwrap();
        assert_eq!(plan.finished_at, Some(now()));
        assert!(plan.overrode_deps.is_empty());

        task.column_id = board.finished_col;
        task.finished_at = Some(now());
        assert_eq!(
            archive_due_at(&board, &task),
            Some(now() + TimeDelta::seconds(board.archive_after_secs))
        );

        let back = plan_move(&board, &task, board.started_col, &[], false, now()).unwrap();
        assert_eq!(back.finished_at, None);

        // Reordering inside the finished column keeps the clock as it was.
        let same = plan_move(&board, &task, board.finished_col, &[], false, now()).unwrap();
        assert_eq!(same.finished_at, Some(now()));
    }

    #[test]
    fn open_dependencies_block_unless_overridden() {
        let board = board_with_members(1, &[1]);
        let task = task_on(&board, 1);
        let open = vec![TaskId(7), TaskId(8)];
        assert_eq!(
            plan_move(&board, &task, board.finished_col, &open, false, now()),
            Err(MoveError::BlockedByDependencies { open: open.clone() })
        );
        let plan = plan_move(&board, &task, board.finished_col, &open, true, now()).unwrap();
        assert_eq!(plan.overrode_deps, open);
        // Dependencies do not matter for any other column.
        let plan = plan_move(&board, &task, board.started_col, &open, false, now()).unwrap();
        assert!(plan.overrode_deps.is_empty());
    }

    #[test]
    fn archived_and_unknown_columns_are_refused() {
        let board = board_with_members(1, &[1]);
        let mut task = task_on(&board, 1);
        assert_eq!(
            plan_move(&board, &task, ColumnId(99), &[], false, now()),
            Err(MoveError::UnknownColumn(ColumnId(99)))
        );
        task.archived_at = Some(now());
        assert_eq!(
            plan_move(&board, &task, board.started_col, &[], false, now()),
            Err(MoveError::TaskArchived)
        );
    }

    #[test]
    fn scan_toggles_and_finishes() {
        let board = board_with_members(1, &[1]);
        let mut task = task_on(&board, 1);
        assert_eq!(
            scan_target(ScanAction::StartPause, &board, &task),
            Ok(board.started_col)
        );
        task.column_id = board.started_col;
        assert_eq!(
            scan_target(ScanAction::StartPause, &board, &task),
            Ok(board.paused_col)
        );
        task.column_id = board.paused_col;
        assert_eq!(
            scan_target(ScanAction::StartPause, &board, &task),
            Ok(board.started_col)
        );
        assert_eq!(
            scan_target(ScanAction::Finish, &board, &task),
            Ok(board.finished_col)
        );
        task.column_id = board.finished_col;
        assert_eq!(
            scan_target(ScanAction::StartPause, &board, &task),
            Err(ScanRefusal::AlreadyFinished)
        );
        assert_eq!(
            scan_target(ScanAction::Finish, &board, &task),
            Err(ScanRefusal::AlreadyFinished)
        );
        task.archived_at = Some(now());
        assert_eq!(
            scan_target(ScanAction::Finish, &board, &task),
            Err(ScanRefusal::Archived)
        );
    }

    #[test]
    fn purge_clock_runs_only_for_deleted_tasks() {
        let board = board_with_members(1, &[1]);
        let mut task = task_on(&board, 1);
        task.archived_at = Some(now());
        assert_eq!(
            purge_due_at(&board, &task),
            None,
            "archived by finishing, kept forever"
        );
        task.deleted_at = Some(now());
        assert_eq!(
            purge_due_at(&board, &task),
            Some(now() + TimeDelta::seconds(board.purge_deleted_after_secs))
        );
    }

    #[test]
    fn restore_falls_back_to_first_column() {
        let board = board_with_members(1, &[1]);
        let mut task = task_on(&board, 1);
        task.archived_from_col = Some(board.paused_col);
        assert_eq!(restore_column(&board, &task), Some(board.paused_col));
        task.archived_from_col = Some(ColumnId(99));
        assert_eq!(restore_column(&board, &task), Some(ColumnId(1)));
        task.archived_from_col = None;
        assert_eq!(restore_column(&board, &task), Some(ColumnId(1)));
    }
}
