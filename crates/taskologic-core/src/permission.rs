//! The permission matrix, in one place.
//!
//! Every check the daemon makes goes through [`check_board`] or
//! [`check_task`]. Nothing else in the codebase is allowed to decide who may
//! do what. The client uses the same functions to hide buttons it knows will
//! fail, but the daemon is the authority.
//!
//! Non members of a private board get [`Denial::BoardNotVisible`] for every
//! action and the daemon answers as if the board does not exist. Admins are
//! the exception: they know a private board exists, may unlock and delete
//! it, and get [`Denial::AdminNotMember`] for anything that would show them
//! its content.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::board::Board;
use crate::ids::Uid;
use crate::task::Task;

/// Who is asking.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub uid: Uid,
    /// The app level admin flag. Not unix root.
    pub is_admin: bool,
}

/// Actions that only need the board.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoardAction {
    View,
    AddTask,
    ManageColumns,
    EditSettings,
    ManageMembers,
    DeleteBoard,
    ToggleLock,
    TogglePrivate,
    /// Permanently delete tasks from the archive.
    PurgeTasks,
}

/// Actions on one task. The task's board is checked first.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAction {
    Edit,
    Move,
    /// Move to the archive marked as deleted. Reversible.
    Delete,
    Restore,
    /// Remove the row for good. Only from the archive.
    Purge,
}

/// Why something was refused. Every variant except `BoardNotVisible` renders
/// as a sentence that says who could have done it instead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "denial", rename_all = "snake_case")]
pub enum Denial {
    /// Private board, not a member. Deliberately says nothing more.
    BoardNotVisible,
    /// An admin outside a private board asked for its content.
    AdminNotMember,
    /// Reserved for the owner and admins. Carries the action as a phrase.
    OwnerOrAdminOnly {
        what: String,
    },
    LockedBoard,
    NotTaskEditor,
    NotTaskDeleter,
    NotTemplateManager,
}

impl Denial {
    pub fn reason(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for Denial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Denial::BoardNotVisible => write!(f, "board not found"),
            Denial::AdminNotMember => write!(
                f,
                "you are not a member of this private board; admins can unlock or delete it but not open it"
            ),
            Denial::OwnerOrAdminOnly { what } => {
                write!(f, "only the board owner or an admin can {what}")
            }
            Denial::LockedBoard => write!(f, "this board is locked, unlock it before deleting it"),
            Denial::NotTaskEditor => write!(
                f,
                "only the task creator, an assignee, the board owner or an admin can edit this task"
            ),
            Denial::NotTaskDeleter => write!(
                f,
                "only the task creator, the board owner or an admin can delete this task"
            ),
            Denial::NotTemplateManager => write!(
                f,
                "only the template creator, the board owner or an admin can change or delete this template"
            ),
        }
    }
}

impl std::error::Error for Denial {}

fn owner_or_admin(ok: bool, what: &str) -> Result<(), Denial> {
    if ok {
        Ok(())
    } else {
        Err(Denial::OwnerOrAdminOnly {
            what: what.to_string(),
        })
    }
}

/// True if the actor can see the board's content.
pub fn can_view(actor: &Actor, board: &Board) -> bool {
    board.is_member(actor.uid)
}

pub fn check_board(action: BoardAction, actor: &Actor, board: &Board) -> Result<(), Denial> {
    if !board.is_member(actor.uid) {
        if !actor.is_admin {
            return Err(Denial::BoardNotVisible);
        }
        if !matches!(action, BoardAction::DeleteBoard | BoardAction::ToggleLock) {
            return Err(Denial::AdminNotMember);
        }
    }
    let privileged = actor.is_admin || board.owner_uid == actor.uid;
    match action {
        BoardAction::View | BoardAction::AddTask => Ok(()),
        BoardAction::ManageColumns => owner_or_admin(privileged, "add, edit or remove columns"),
        BoardAction::EditSettings => owner_or_admin(privileged, "edit board settings"),
        BoardAction::ManageMembers => owner_or_admin(privileged, "manage members"),
        BoardAction::ToggleLock => owner_or_admin(privileged, "lock or unlock the board"),
        BoardAction::TogglePrivate => {
            owner_or_admin(privileged, "change whether the board is private")
        }
        BoardAction::PurgeTasks => owner_or_admin(privileged, "permanently delete archived tasks"),
        BoardAction::DeleteBoard => {
            owner_or_admin(privileged, "delete the board")?;
            if board.is_locked {
                Err(Denial::LockedBoard)
            } else {
                Ok(())
            }
        }
    }
}

pub fn check_task(
    action: TaskAction,
    actor: &Actor,
    board: &Board,
    task: &Task,
) -> Result<(), Denial> {
    debug_assert_eq!(
        task.board_id, board.id,
        "task checked against the wrong board"
    );
    if !board.is_member(actor.uid) {
        return Err(if actor.is_admin {
            Denial::AdminNotMember
        } else {
            Denial::BoardNotVisible
        });
    }
    let privileged = actor.is_admin || board.owner_uid == actor.uid;
    let creator = task.created_by == actor.uid;
    let assignee = task.is_assignee(actor.uid);
    match action {
        // Moving is open to every member, that is the point of a shared board.
        // So is restoring, a delete is meant to be cheap to undo.
        TaskAction::Move | TaskAction::Restore => Ok(()),
        TaskAction::Edit => {
            if privileged || creator || assignee {
                Ok(())
            } else {
                Err(Denial::NotTaskEditor)
            }
        }
        TaskAction::Delete => {
            if privileged || creator {
                Ok(())
            } else {
                Err(Denial::NotTaskDeleter)
            }
        }
        TaskAction::Purge => owner_or_admin(privileged, "permanently delete tasks"),
    }
}

/// Templates follow the task rules: any member may create and use them,
/// changing or deleting one is for its creator, the board owner or an admin.
/// `owner_uid` is whoever saved the template.
pub fn check_template_manage(actor: &Actor, board: &Board, owner_uid: Uid) -> Result<(), Denial> {
    if !board.is_member(actor.uid) {
        return Err(if actor.is_admin {
            Denial::AdminNotMember
        } else {
            Denial::BoardNotVisible
        });
    }
    if actor.is_admin || board.owner_uid == actor.uid || owner_uid == actor.uid {
        Ok(())
    } else {
        Err(Denial::NotTemplateManager)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::test_support::board_with_members;
    use crate::task::test_support::task_on;

    const OWNER: Uid = 1;
    const ADMIN: Uid = 2;
    const CREATOR: Uid = 3;
    const ASSIGNEE: Uid = 4;
    const OTHER: Uid = 5;
    const OUTSIDER: Uid = 6;
    const OUTSIDE_ADMIN: Uid = 7;

    fn actor(uid: Uid) -> Actor {
        Actor {
            uid,
            is_admin: uid == ADMIN || uid == OUTSIDE_ADMIN,
        }
    }

    fn fixture(private: bool, locked: bool) -> (Board, Task) {
        let mut board = board_with_members(OWNER, &[OWNER, ADMIN, CREATOR, ASSIGNEE, OTHER]);
        board.is_private = private;
        board.is_locked = locked;
        let mut task = task_on(&board, CREATOR);
        task.assignees = vec![ASSIGNEE];
        (board, task)
    }

    /// The README table, row by row. Columns: owner, admin, creator, assignee, other.
    const ROWS: &[(&str, [bool; 5])] = &[
        ("view", [true, true, true, true, true]),
        ("add_task", [true, true, true, true, true]),
        ("edit_task", [true, true, true, true, false]),
        ("move_task", [true, true, true, true, true]),
        ("delete_task", [true, true, true, false, false]),
        ("purge_task", [true, true, false, false, false]),
        ("columns", [true, true, false, false, false]),
        ("settings", [true, true, false, false, false]),
        ("members", [true, true, false, false, false]),
        ("delete_unlocked", [true, true, false, false, false]),
        ("delete_locked", [false, false, false, false, false]),
        ("restore", [true, true, true, true, true]),
        ("toggle_lock", [true, true, false, false, false]),
        ("toggle_private", [true, true, false, false, false]),
        // CREATOR stands in as the template owner here.
        ("manage_template", [true, true, true, false, false]),
    ];

    fn run(row: &str, uid: Uid) -> Result<(), Denial> {
        let locked = row == "delete_locked";
        let (board, task) = fixture(true, locked);
        let a = actor(uid);
        match row {
            "view" => check_board(BoardAction::View, &a, &board),
            "add_task" => check_board(BoardAction::AddTask, &a, &board),
            "edit_task" => check_task(TaskAction::Edit, &a, &board, &task),
            "move_task" => check_task(TaskAction::Move, &a, &board, &task),
            "delete_task" => check_task(TaskAction::Delete, &a, &board, &task),
            "purge_task" => check_task(TaskAction::Purge, &a, &board, &task),
            "columns" => check_board(BoardAction::ManageColumns, &a, &board),
            "settings" => check_board(BoardAction::EditSettings, &a, &board),
            "members" => check_board(BoardAction::ManageMembers, &a, &board),
            "delete_unlocked" | "delete_locked" => {
                check_board(BoardAction::DeleteBoard, &a, &board)
            }
            "restore" => check_task(TaskAction::Restore, &a, &board, &task),
            "toggle_lock" => check_board(BoardAction::ToggleLock, &a, &board),
            "toggle_private" => check_board(BoardAction::TogglePrivate, &a, &board),
            "manage_template" => check_template_manage(&a, &board, CREATOR),
            _ => unreachable!(),
        }
    }

    #[test]
    fn matrix_matches_the_readme() {
        for (row, expected) in ROWS {
            for (i, uid) in [OWNER, ADMIN, CREATOR, ASSIGNEE, OTHER]
                .into_iter()
                .enumerate()
            {
                let got = run(row, uid);
                assert_eq!(got.is_ok(), expected[i], "row {row}, uid {uid}: {got:?}");
                if let Err(d) = got {
                    assert_ne!(d, Denial::BoardNotVisible, "members must get a real reason");
                    assert!(!d.reason().is_empty());
                }
            }
        }
    }

    #[test]
    fn locked_boards_need_unlocking_first_even_for_the_owner() {
        let (board, _) = fixture(false, true);
        assert_eq!(
            check_board(BoardAction::DeleteBoard, &actor(OWNER), &board),
            Err(Denial::LockedBoard)
        );
        assert_eq!(
            check_board(BoardAction::DeleteBoard, &actor(ADMIN), &board),
            Err(Denial::LockedBoard)
        );
        assert_eq!(
            check_board(BoardAction::ToggleLock, &actor(OWNER), &board),
            Ok(())
        );
    }

    #[test]
    fn outsiders_get_nothing_on_private_boards() {
        for (row, _) in ROWS {
            assert_eq!(
                run(row, OUTSIDER),
                Err(Denial::BoardNotVisible),
                "row {row}"
            );
        }
    }

    #[test]
    fn admins_outside_a_private_board_can_delete_or_unlock_but_not_look() {
        let (board, task) = fixture(true, false);
        let a = actor(OUTSIDE_ADMIN);
        assert_eq!(check_board(BoardAction::DeleteBoard, &a, &board), Ok(()));
        assert_eq!(check_board(BoardAction::ToggleLock, &a, &board), Ok(()));
        for action in [
            BoardAction::View,
            BoardAction::AddTask,
            BoardAction::ManageColumns,
            BoardAction::EditSettings,
            BoardAction::ManageMembers,
            BoardAction::TogglePrivate,
            BoardAction::PurgeTasks,
        ] {
            assert_eq!(
                check_board(action, &a, &board),
                Err(Denial::AdminNotMember),
                "{action:?}"
            );
        }
        for action in [
            TaskAction::Edit,
            TaskAction::Move,
            TaskAction::Delete,
            TaskAction::Restore,
            TaskAction::Purge,
        ] {
            assert_eq!(
                check_task(action, &a, &board, &task),
                Err(Denial::AdminNotMember),
                "{action:?}"
            );
        }
        let (locked, _) = fixture(true, true);
        assert_eq!(
            check_board(BoardAction::DeleteBoard, &a, &locked),
            Err(Denial::LockedBoard)
        );
    }

    #[test]
    fn everyone_is_a_member_of_a_public_board() {
        let (board, task) = fixture(false, false);
        let a = actor(OUTSIDER);
        assert!(check_board(BoardAction::View, &a, &board).is_ok());
        assert!(check_task(TaskAction::Move, &a, &board, &task).is_ok());
        assert_eq!(
            check_board(BoardAction::ManageMembers, &a, &board),
            Err(Denial::OwnerOrAdminOnly {
                what: "manage members".into()
            })
        );
        assert_eq!(
            check_board(BoardAction::DeleteBoard, &a, &board),
            Err(Denial::OwnerOrAdminOnly {
                what: "delete the board".into()
            })
        );
    }

    #[test]
    fn denials_explain_themselves() {
        assert_eq!(
            Denial::OwnerOrAdminOnly {
                what: "manage members".into()
            }
            .reason(),
            "only the board owner or an admin can manage members"
        );
        assert!(Denial::LockedBoard.reason().contains("unlock"));
        assert!(Denial::AdminNotMember.reason().contains("admins"));
    }
}
