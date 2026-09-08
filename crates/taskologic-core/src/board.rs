//! Boards and columns.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{BoardId, ColumnId, Uid};
use crate::prefs::CardFields;

/// Default archive delay for finished tasks, one week.
pub const DEFAULT_ARCHIVE_AFTER_SECS: i64 = 7 * 24 * 60 * 60;
/// Default time a deleted task stays in the archive before it is purged, 30 days.
pub const DEFAULT_PURGE_DELETED_AFTER_SECS: i64 = 30 * 24 * 60 * 60;
pub const MAX_NAME_CHARS: usize = 120;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub id: ColumnId,
    pub board_id: BoardId,
    pub name: String,
    /// Manual order. Gaps are fine, only relative order matters.
    pub position: i64,
    /// Show tasks by due date instead of their manual order.
    pub sort_by_due: bool,
}

/// The three columns every board designates. They drive the barcode actions,
/// the archive clock and, later, time tracking.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnRole {
    Started,
    Paused,
    Finished,
}

impl ColumnRole {
    pub const ALL: [ColumnRole; 3] = [
        ColumnRole::Started,
        ColumnRole::Paused,
        ColumnRole::Finished,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ColumnRole::Started => "started",
            ColumnRole::Paused => "paused/deferred",
            ColumnRole::Finished => "finished",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    pub id: BoardId,
    pub name: String,
    /// What the board is for. Shown on its card and in its settings.
    pub description: String,
    pub owner_uid: Uid,
    /// Only the owner and admins may delete a locked board.
    pub is_locked: bool,
    /// Only members can see a private board. Public boards are visible to
    /// every Taskologic user and the member list is ignored.
    pub is_private: bool,
    pub archive_after_secs: i64,
    /// How long a deleted task stays in the archive before it is purged.
    pub purge_deleted_after_secs: i64,
    /// Default for what task cards show here. Users may override it.
    pub card_fields: CardFields,
    pub started_col: ColumnId,
    pub paused_col: ColumnId,
    pub finished_col: ColumnId,
    pub created_at: DateTime<Utc>,
    /// Sorted by position.
    pub columns: Vec<Column>,
    /// Explicit members. The owner is always in here. Unused while public.
    pub members: Vec<Uid>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BoardError {
    #[error("board name cannot be empty")]
    EmptyName,
    #[error("board name is too long, {MAX_NAME_CHARS} characters at most")]
    NameTooLong,
    #[error("a board needs at least one column")]
    NoColumns,
    #[error("column names cannot be empty")]
    EmptyColumnName,
    #[error("column name {0:?} is used twice")]
    DuplicateColumnName(String),
    #[error("the {} column is not one of the board's columns", .0.label())]
    DesignatedColumnMissing(ColumnRole),
    #[error("archive delay cannot be negative")]
    NegativeArchiveDelay,
    #[error("deleted task retention cannot be negative")]
    NegativePurgeDelay,
    #[error("column {0} is not on this board")]
    ColumnNotOnBoard(ColumnId),
    #[error("the column still has tasks, pick where they should go")]
    TasksNeedDestination,
    #[error("this is the {} column, pick a replacement before removing it", .0.label())]
    DesignatedNeedsReplacement(ColumnRole),
    #[error("tasks and roles cannot be moved to the column being removed")]
    DestinationIsRemovedColumn,
    #[error("the owner cannot be removed from the member list")]
    OwnerIsAlwaysMember,
}

pub fn validate_name(name: &str) -> Result<(), BoardError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(BoardError::EmptyName);
    }
    if trimmed.chars().count() > MAX_NAME_CHARS {
        return Err(BoardError::NameTooLong);
    }
    Ok(())
}

pub fn validate_column_names<S: AsRef<str>>(names: &[S]) -> Result<(), BoardError> {
    if names.is_empty() {
        return Err(BoardError::NoColumns);
    }
    let mut seen: Vec<String> = Vec::new();
    for n in names {
        let n = n.as_ref().trim();
        if n.is_empty() {
            return Err(BoardError::EmptyColumnName);
        }
        let key = n.to_lowercase();
        if seen.contains(&key) {
            return Err(BoardError::DuplicateColumnName(n.to_string()));
        }
        seen.push(key);
    }
    Ok(())
}

impl Board {
    /// Owner always, everyone on a public board, the member list otherwise.
    /// The daemon has already made sure the uid is a Taskologic user.
    pub fn is_member(&self, uid: Uid) -> bool {
        uid == self.owner_uid || !self.is_private || self.members.contains(&uid)
    }

    pub fn column(&self, id: ColumnId) -> Option<&Column> {
        self.columns.iter().find(|c| c.id == id)
    }

    pub fn has_column(&self, id: ColumnId) -> bool {
        self.column(id).is_some()
    }

    /// The leftmost column, where new and repeated tasks land.
    pub fn first_column(&self) -> Option<&Column> {
        self.columns.iter().min_by_key(|c| c.position)
    }

    pub fn column_for(&self, role: ColumnRole) -> ColumnId {
        match role {
            ColumnRole::Started => self.started_col,
            ColumnRole::Paused => self.paused_col,
            ColumnRole::Finished => self.finished_col,
        }
    }

    /// Every role a column holds. A column may hold more than one, the spec
    /// does not forbid it, but the barcode toggle behaves oddly if started
    /// and paused are the same column so the UI should discourage that.
    pub fn roles_of(&self, col: ColumnId) -> Vec<ColumnRole> {
        ColumnRole::ALL
            .into_iter()
            .filter(|r| self.column_for(*r) == col)
            .collect()
    }

    pub fn validate(&self) -> Result<(), BoardError> {
        validate_name(&self.name)?;
        let names: Vec<&str> = self.columns.iter().map(|c| c.name.as_str()).collect();
        validate_column_names(&names)?;
        for role in ColumnRole::ALL {
            if !self.has_column(self.column_for(role)) {
                return Err(BoardError::DesignatedColumnMissing(role));
            }
        }
        if self.archive_after_secs < 0 {
            return Err(BoardError::NegativeArchiveDelay);
        }
        if self.purge_deleted_after_secs < 0 {
            return Err(BoardError::NegativePurgeDelay);
        }
        if !self.members.contains(&self.owner_uid) {
            return Err(BoardError::OwnerIsAlwaysMember);
        }
        Ok(())
    }

    /// Uids that currently hold a task on the board but would lose access if
    /// it went private. The caller supplies the assignee set because tasks
    /// are not loaded with the board.
    pub fn would_lose_access(&self, assignees_on_board: &[Uid]) -> Vec<Uid> {
        let mut out: Vec<Uid> = assignees_on_board
            .iter()
            .copied()
            .filter(|uid| *uid != self.owner_uid && !self.members.contains(uid))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Result of [`plan_column_removal`], everything the daemon has to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnRemoval {
    pub removed: ColumnId,
    pub move_tasks_to: Option<ColumnId>,
    pub reassign_roles: Vec<(ColumnRole, ColumnId)>,
}

/// Removing a column never silently deletes tasks and never leaves a role
/// dangling. `replacements` must cover every role the column holds.
pub fn plan_column_removal(
    board: &Board,
    col: ColumnId,
    has_tasks: bool,
    move_tasks_to: Option<ColumnId>,
    replacements: &[(ColumnRole, ColumnId)],
) -> Result<ColumnRemoval, BoardError> {
    if !board.has_column(col) {
        return Err(BoardError::ColumnNotOnBoard(col));
    }
    if board.columns.len() <= 1 {
        return Err(BoardError::NoColumns);
    }
    if has_tasks && move_tasks_to.is_none() {
        return Err(BoardError::TasksNeedDestination);
    }
    if let Some(dest) = move_tasks_to {
        if dest == col {
            return Err(BoardError::DestinationIsRemovedColumn);
        }
        if !board.has_column(dest) {
            return Err(BoardError::ColumnNotOnBoard(dest));
        }
    }
    let mut reassign_roles = Vec::new();
    for role in board.roles_of(col) {
        let (_, replacement) = replacements
            .iter()
            .find(|(r, _)| *r == role)
            .ok_or(BoardError::DesignatedNeedsReplacement(role))?;
        if *replacement == col {
            return Err(BoardError::DestinationIsRemovedColumn);
        }
        if !board.has_column(*replacement) {
            return Err(BoardError::ColumnNotOnBoard(*replacement));
        }
        reassign_roles.push((role, *replacement));
    }
    Ok(ColumnRemoval {
        removed: col,
        move_tasks_to: if has_tasks { move_tasks_to } else { None },
        reassign_roles,
    })
}

#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use super::*;

    /// Four column board: Todo, Doing, Waiting, Done, with the obvious roles.
    pub fn board_with_members(owner: Uid, members: &[Uid]) -> Board {
        let id = BoardId(1);
        let names = ["Todo", "Doing", "Waiting", "Done"];
        let columns = names
            .iter()
            .enumerate()
            .map(|(i, n)| Column {
                id: ColumnId(i as i64 + 1),
                board_id: id,
                name: n.to_string(),
                position: i as i64 * 10,
                sort_by_due: false,
            })
            .collect();
        let mut members = members.to_vec();
        if !members.contains(&owner) {
            members.insert(0, owner);
        }
        Board {
            id,
            name: "Test board".into(),
            description: String::new(),
            owner_uid: owner,
            is_locked: false,
            is_private: true,
            archive_after_secs: DEFAULT_ARCHIVE_AFTER_SECS,
            purge_deleted_after_secs: DEFAULT_PURGE_DELETED_AFTER_SECS,
            card_fields: CardFields::default(),
            started_col: ColumnId(2),
            paused_col: ColumnId(3),
            finished_col: ColumnId(4),
            created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            columns,
            members,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::board_with_members;
    use super::*;

    #[test]
    fn validation() {
        let mut b = board_with_members(1, &[1, 2]);
        assert_eq!(b.validate(), Ok(()));
        b.name = "  ".into();
        assert_eq!(b.validate(), Err(BoardError::EmptyName));
        b.name = "ok".into();
        b.finished_col = ColumnId(99);
        assert_eq!(
            b.validate(),
            Err(BoardError::DesignatedColumnMissing(ColumnRole::Finished))
        );
        b.finished_col = ColumnId(4);
        b.columns[1].name = "todo".into();
        assert_eq!(
            b.validate(),
            Err(BoardError::DuplicateColumnName("todo".into()))
        );
        b.columns[1].name = "Doing".into();
        b.members = vec![2];
        assert_eq!(b.validate(), Err(BoardError::OwnerIsAlwaysMember));
    }

    #[test]
    fn membership() {
        let mut b = board_with_members(1, &[1, 2]);
        assert!(b.is_member(1));
        assert!(b.is_member(2));
        assert!(!b.is_member(3));
        b.is_private = false;
        assert!(b.is_member(3));
        b.is_private = true;
        b.members = vec![];
        assert!(
            b.is_member(1),
            "owner is a member even if the list is broken"
        );
    }

    #[test]
    fn going_private_reports_who_would_be_cut_off() {
        let b = board_with_members(1, &[1, 2]);
        assert_eq!(b.would_lose_access(&[1, 2, 3, 3, 4]), vec![3, 4]);
    }

    #[test]
    fn column_removal_rules() {
        let b = board_with_members(1, &[1]);
        let todo = ColumnId(1);
        let doing = ColumnId(2);
        let done = ColumnId(4);

        assert_eq!(
            plan_column_removal(&b, todo, true, None, &[]),
            Err(BoardError::TasksNeedDestination)
        );
        assert_eq!(
            plan_column_removal(&b, todo, true, Some(todo), &[]),
            Err(BoardError::DestinationIsRemovedColumn)
        );
        assert_eq!(
            plan_column_removal(&b, todo, false, None, &[]),
            Ok(ColumnRemoval {
                removed: todo,
                move_tasks_to: None,
                reassign_roles: vec![]
            })
        );
        assert_eq!(
            plan_column_removal(&b, done, false, None, &[]),
            Err(BoardError::DesignatedNeedsReplacement(ColumnRole::Finished))
        );
        assert_eq!(
            plan_column_removal(
                &b,
                done,
                true,
                Some(doing),
                &[(ColumnRole::Finished, doing)]
            ),
            Ok(ColumnRemoval {
                removed: done,
                move_tasks_to: Some(doing),
                reassign_roles: vec![(ColumnRole::Finished, doing)]
            })
        );
        assert_eq!(
            plan_column_removal(&b, ColumnId(42), false, None, &[]),
            Err(BoardError::ColumnNotOnBoard(ColumnId(42)))
        );
    }
}
