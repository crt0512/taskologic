//! Task templates. A template is a saved [`TaskDraft`] on a board, used to
//! stamp out recurring kinds of work. Templates deliberately carry no due
//! date and no dependencies: both are properties of one concrete instance,
//! not of the kind of task.

use serde::{Deserialize, Serialize};

use crate::ids::{BoardId, TemplateId, Uid};
use crate::task::TaskDraft;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    pub id: TemplateId,
    pub board_id: BoardId,
    /// Whoever saved it. Managing the template follows the task rules:
    /// this user, the board owner or an admin.
    pub owner_uid: Uid,
    pub name: String,
    pub draft: TaskDraft,
}
