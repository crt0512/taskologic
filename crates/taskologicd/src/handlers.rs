//! One function per request. Permission checks all go through
//! `taskologic_core::permission`, nothing in here decides on its own.

use std::sync::Arc;

use chrono::Utc;
use taskologic_core::barcode::{ScanParseError, ScanPayload};
use taskologic_core::board::{Board, plan_column_removal};
use taskologic_core::event::EventKind;
use taskologic_core::ids::{ColumnId, TaskId, Uid};
use taskologic_core::permission::{
    Actor, BoardAction, TaskAction, check_board, check_task, check_template_manage,
};
use taskologic_core::print::{AutoprintTrigger, build_task_job, wants_autoprint};
use taskologic_core::task::{Task, TaskDraft};
use taskologic_core::template::Template;
use taskologic_core::transition::{self, plan_move};
use taskologic_core::user::UserSummary;
use taskologic_proto::{
    BoardChange, BoardDetail, Event, RepeatEntry, Request, Response, ScanOutcome, Severity,
    TaskChange,
};

use crate::auth;
use crate::db::repo::{self, PrintAck};
use crate::error::AppError;
use crate::state::AppState;

/// The authenticated connection a request came in on.
#[derive(Clone, Debug)]
pub struct Session {
    pub uid: Uid,
    pub is_admin: bool,
}

impl Session {
    fn actor(&self) -> Actor {
        Actor {
            uid: self.uid,
            is_admin: self.is_admin,
        }
    }
}

type R = Result<Response, AppError>;

pub fn handle(state: &Arc<AppState>, s: &Session, req: Request) -> R {
    match req {
        Request::Hello(_) => Err(AppError::Protocol("hello already sent".into())),
        Request::ListBoards => {
            let boards = state
                .db
                .with(|c| repo::list_boards_visible(c, s.uid, s.is_admin))?;
            Ok(Response::Boards { boards })
        }
        Request::GetBoard { board_id } => {
            let (board, tasks) = state.db.with(|c| {
                let board = repo::require_board(c, board_id)?;
                check_board(BoardAction::View, &s.actor(), &board)?;
                let tasks = repo::list_tasks(c, board_id, false)?;
                Ok((board, tasks))
            })?;
            Ok(Response::Board(BoardDetail { board, tasks }))
        }
        Request::CreateBoard(req) => {
            let board = state
                .db
                .tx(|c| repo::create_board(c, s.uid, &req, Utc::now()))?;
            state.publish_board_meta(
                &board,
                Event::BoardChanged {
                    board_id: board.id,
                    change: BoardChange::Created,
                },
            );
            let tasks = Vec::new();
            Ok(Response::Board(BoardDetail { board, tasks }))
        }
        Request::UpdateBoard(req) => {
            // Not load_board: an admin outside a private board may unlock it,
            // so the checks are per field and the content is withheld below.
            let before = state.db.with(|c| repo::require_board(c, req.board_id))?;
            let actor = s.actor();
            let settings = req.name.is_some()
                || req.archive_after_secs.is_some()
                || req.purge_deleted_after_secs.is_some()
                || req.card_fields.is_some()
                || !req.roles.is_empty();
            if req.is_locked.is_some() {
                check_board(BoardAction::ToggleLock, &actor, &before)?;
            }
            if req.is_private.is_some() {
                check_board(BoardAction::TogglePrivate, &actor, &before)?;
            }
            if settings || (req.is_locked.is_none() && req.is_private.is_none()) {
                check_board(BoardAction::EditSettings, &actor, &before)?;
            }
            let after = state.db.tx(|c| {
                let after = repo::update_board(c, &before, &req)?;
                // Going private must not cut people off from work that is
                // already theirs: everyone with a task assigned carries over
                // as a member. The owner can remove them afterwards.
                if !before.is_private && after.is_private {
                    let assigned = repo::assignee_uids_on_board(c, after.id)?;
                    for uid in after.would_lose_access(&assigned) {
                        repo::add_member(c, after.id, uid, Utc::now())?;
                        repo::record_event(
                            c,
                            after.id,
                            None,
                            Some(s.uid),
                            &EventKind::MemberAdded { uid },
                            Utc::now(),
                        )?;
                    }
                }
                repo::record_event(
                    c,
                    after.id,
                    None,
                    Some(s.uid),
                    &EventKind::BoardSettingsChanged,
                    Utc::now(),
                )?;
                repo::require_board(c, after.id)
            })?;
            // Going private: whoever could see it before gets told it changed,
            // using the wider audience so non members learn it disappeared.
            state.publish_board_meta(
                &before,
                Event::BoardChanged {
                    board_id: after.id,
                    change: BoardChange::Settings,
                },
            );
            if !before.is_private && after.is_private {
                state.publish_board(
                    &before,
                    Event::BoardChanged {
                        board_id: after.id,
                        change: BoardChange::AccessRevoked,
                    },
                );
            }
            let tasks = if after.is_member(s.uid) {
                state.db.with(|c| repo::list_tasks(c, after.id, false))?
            } else {
                Vec::new()
            };
            Ok(Response::Board(BoardDetail {
                board: after,
                tasks,
            }))
        }
        Request::DeleteBoard { board_id } => {
            let board = state.db.with(|c| repo::require_board(c, board_id))?;
            check_board(BoardAction::DeleteBoard, &s.actor(), &board)?;
            state.db.tx(|c| {
                repo::record_event(
                    c,
                    board.id,
                    None,
                    Some(s.uid),
                    &EventKind::BoardDeleted,
                    Utc::now(),
                )?;
                repo::delete_board(c, board.id)
            })?;
            state.publish_board_meta(
                &board,
                Event::BoardChanged {
                    board_id,
                    change: BoardChange::Deleted,
                },
            );
            Ok(Response::Done)
        }
        Request::AddColumn { board_id, name } => {
            let board = load_board(state, s, board_id)?;
            check_board(BoardAction::ManageColumns, &s.actor(), &board)?;
            state.db.tx(|c| {
                let col = repo::add_column(c, &board, &name)?;
                repo::record_event(
                    c,
                    board.id,
                    None,
                    Some(s.uid),
                    &EventKind::ColumnAdded { column: col.id },
                    Utc::now(),
                )
            })?;
            columns_changed(state, s, board_id)
        }
        Request::RenameColumn { column_id, name } => {
            let board = board_of_column(state, s, column_id)?;
            check_board(BoardAction::ManageColumns, &s.actor(), &board)?;
            state.db.tx(|c| {
                repo::rename_column(c, &board, column_id, &name)?;
                repo::record_event(
                    c,
                    board.id,
                    None,
                    Some(s.uid),
                    &EventKind::ColumnRenamed { column: column_id },
                    Utc::now(),
                )
            })?;
            columns_changed(state, s, board.id)
        }
        Request::ReorderColumns { board_id, order } => {
            let board = load_board(state, s, board_id)?;
            check_board(BoardAction::ManageColumns, &s.actor(), &board)?;
            state.db.tx(|c| repo::reorder_columns(c, &board, &order))?;
            columns_changed(state, s, board_id)
        }
        Request::SetColumnSort {
            column_id,
            sort_by_due,
        } => {
            let board = board_of_column(state, s, column_id)?;
            check_board(BoardAction::ManageColumns, &s.actor(), &board)?;
            state
                .db
                .with(|c| repo::set_column_sort(c, column_id, sort_by_due))?;
            columns_changed(state, s, board.id)
        }
        Request::RemoveColumn {
            column_id,
            move_tasks_to,
            replacements,
        } => {
            let board = board_of_column(state, s, column_id)?;
            check_board(BoardAction::ManageColumns, &s.actor(), &board)?;
            let has_tasks = state
                .db
                .with(|c| repo::task_count_in_column(c, column_id))?
                > 0;
            let plan =
                plan_column_removal(&board, column_id, has_tasks, move_tasks_to, &replacements)?;
            state.db.tx(|c| {
                repo::remove_column(c, &board, &plan)?;
                repo::record_event(
                    c,
                    board.id,
                    None,
                    Some(s.uid),
                    &EventKind::ColumnRemoved {
                        column: column_id,
                        tasks_moved_to: plan.move_tasks_to,
                    },
                    Utc::now(),
                )
            })?;
            columns_changed(state, s, board.id)
        }
        Request::ListMembers { board_id } => {
            let board = load_board(state, s, board_id)?;
            let members = state.db.with(|c| repo::list_members(c, &board))?;
            Ok(Response::Members { members })
        }
        Request::AddMember { board_id, uid } => {
            let board = load_board(state, s, board_id)?;
            check_board(BoardAction::ManageMembers, &s.actor(), &board)?;
            state.db.tx(|c| {
                repo::require_user(c, uid).map_err(|_| {
                    AppError::bad(format!("uid {uid} has never logged in to Taskologic"))
                })?;
                repo::add_member(c, board.id, uid, Utc::now())?;
                repo::record_event(
                    c,
                    board.id,
                    None,
                    Some(s.uid),
                    &EventKind::MemberAdded { uid },
                    Utc::now(),
                )
            })?;
            let board = load_board(state, s, board_id)?;
            state.publish_board_meta(
                &board,
                Event::BoardChanged {
                    board_id,
                    change: BoardChange::Members,
                },
            );
            Ok(Response::Done)
        }
        Request::RemoveMember {
            board_id,
            uid,
            unassign,
        } => {
            let board = load_board(state, s, board_id)?;
            check_board(BoardAction::ManageMembers, &s.actor(), &board)?;
            if uid == board.owner_uid {
                return Err(AppError::bad(
                    "the owner cannot be removed from their own board",
                ));
            }
            let unassigned = state.db.tx(|c| {
                let assigned = repo::tasks_assigned_on_board(c, board.id, uid)?;
                if !assigned.is_empty() && !unassign {
                    return Err(AppError::MemberHasTasks(assigned));
                }
                let unassigned = repo::unassign_on_board(c, board.id, uid)?;
                repo::remove_member(c, board.id, uid)?;
                repo::record_event(
                    c,
                    board.id,
                    None,
                    Some(s.uid),
                    &EventKind::MemberRemoved {
                        uid,
                        unassigned: unassigned.clone(),
                    },
                    Utc::now(),
                )?;
                Ok(unassigned)
            })?;
            // Tell the removed user first, with the old audience, then the rest.
            if board.is_private {
                state.publish_uid(
                    uid,
                    Event::BoardChanged {
                        board_id,
                        change: BoardChange::AccessRevoked,
                    },
                );
            }
            let after = load_board(state, s, board_id)?;
            state.publish_board_meta(
                &after,
                Event::BoardChanged {
                    board_id,
                    change: BoardChange::Members,
                },
            );
            for t in unassigned {
                if let Some(task) = state.db.with(|c| repo::get_task(c, t))? {
                    state.publish_board(
                        &after,
                        Event::TaskChanged {
                            task,
                            change: TaskChange::Edited,
                            actor: Some(s.uid),
                        },
                    );
                }
            }
            Ok(Response::Done)
        }
        Request::ListUsers => {
            let mut users: Vec<UserSummary> = state
                .db
                .with(repo::list_users)?
                .iter()
                .map(UserSummary::from)
                .collect();
            // Group members who have never logged in are still Taskologic users
            // and belong in the picker. Failures here only lose that extra.
            match auth::group_members(&state.cfg.group) {
                Ok(members) => {
                    for m in members {
                        if !users.iter().any(|u| u.uid == m.uid) {
                            users.push(UserSummary {
                                uid: m.uid,
                                username: m.username,
                                is_admin: false,
                            });
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "could not list group members"),
            }
            users.sort_by(|a, b| a.username.cmp(&b.username).then(a.uid.cmp(&b.uid)));
            Ok(Response::Users { users })
        }
        Request::SetAdmin { uid, is_admin } => {
            if !s.is_admin {
                return Err(AppError::Denied(
                    taskologic_core::permission::Denial::OwnerOrAdminOnly {
                        what: "grant or revoke admin".into(),
                    },
                ));
            }
            if uid == s.uid && !is_admin {
                return Err(AppError::bad("you cannot revoke your own admin flag"));
            }
            state.db.with(|c| {
                repo::require_user(c, uid)?;
                repo::set_admin(c, uid, is_admin)
            })?;
            Ok(Response::Done)
        }
        Request::CreateTask {
            board_id,
            column_id,
            draft,
        } => {
            let board = load_board(state, s, board_id)?;
            check_board(BoardAction::AddTask, &s.actor(), &board)?;
            let column = match column_id {
                Some(c) if board.has_column(c) => c,
                Some(c) => return Err(AppError::bad(format!("column {c} is not on this board"))),
                None => board
                    .first_column()
                    .map(|c| c.id)
                    .ok_or_else(|| AppError::bad("board has no columns"))?,
            };
            let task = state.db.tx(|c| {
                check_deps(c, s, TaskId(0), &draft)?;
                repo::create_task(c, &board, column, &draft, s.uid, Utc::now())
            })?;
            state.publish_board(
                &board,
                Event::TaskChanged {
                    task: task.clone(),
                    change: TaskChange::Created,
                    actor: Some(s.uid),
                },
            );
            autoprint(state, &board, &task, AutoprintTrigger::AddedToBoard);
            Ok(Response::Task { task })
        }
        Request::UpdateTask {
            task_id,
            version,
            draft,
        } => {
            let (board, task) = load_task(state, s, task_id)?;
            check_task(TaskAction::Edit, &s.actor(), &board, &task)?;
            if task.version != version {
                return Err(AppError::Conflict {
                    current: Box::new(task),
                });
            }
            let task = state.db.tx(|c| {
                taskologic_core::task::validate_draft(&draft, &board)?;
                check_deps(c, s, task.id, &draft)?;
                let t = repo::update_task(c, &task, &draft, Utc::now())?;
                repo::record_event(
                    c,
                    board.id,
                    Some(t.id),
                    Some(s.uid),
                    &EventKind::TaskEdited,
                    Utc::now(),
                )?;
                Ok(t)
            })?;
            state.publish_board(
                &board,
                Event::TaskChanged {
                    task: task.clone(),
                    change: TaskChange::Edited,
                    actor: Some(s.uid),
                },
            );
            Ok(Response::Task { task })
        }
        Request::MoveTask {
            task_id,
            to_column,
            position,
            override_deps,
        } => {
            let (board, task) = load_task(state, s, task_id)?;
            check_task(TaskAction::Move, &s.actor(), &board, &task)?;
            let moved = move_task(state, s, &board, &task, to_column, position, override_deps)?;
            Ok(Response::Task { task: moved })
        }
        Request::SetChecklistItem {
            task_id,
            index,
            done,
        } => {
            let (board, task) = load_task(state, s, task_id)?;
            // Ticking an item is working the task, the same as moving it.
            check_task(TaskAction::Move, &s.actor(), &board, &task)?;
            let task = state.db.tx(|c| {
                let t = repo::set_checklist_item(c, &task, index, done)?;
                repo::record_event(
                    c,
                    board.id,
                    Some(t.id),
                    Some(s.uid),
                    &EventKind::TaskEdited,
                    Utc::now(),
                )?;
                Ok(t)
            })?;
            state.publish_board(
                &board,
                Event::TaskChanged {
                    task: task.clone(),
                    change: TaskChange::Edited,
                    actor: Some(s.uid),
                },
            );
            Ok(Response::Task { task })
        }
        Request::DeleteTask { task_id } => {
            let (board, task) = load_task(state, s, task_id)?;
            check_task(TaskAction::Delete, &s.actor(), &board, &task)?;
            if task.is_archived() {
                return Err(AppError::bad("task is already in the archive"));
            }
            let task = state.db.tx(|c| {
                let t = repo::soft_delete_task(c, &task, Utc::now())?;
                repo::record_event(
                    c,
                    board.id,
                    Some(t.id),
                    Some(s.uid),
                    &EventKind::TaskDeleted,
                    Utc::now(),
                )?;
                Ok(t)
            })?;
            state.publish_board(
                &board,
                Event::TaskChanged {
                    task: task.clone(),
                    change: TaskChange::Deleted,
                    actor: Some(s.uid),
                },
            );
            Ok(Response::Task { task })
        }
        Request::PurgeTask { task_id } => {
            let (board, task) = load_task(state, s, task_id)?;
            check_task(TaskAction::Purge, &s.actor(), &board, &task)?;
            if !task.is_archived() {
                return Err(AppError::bad(
                    "only archived tasks can be deleted for good, delete it first",
                ));
            }
            state.db.tx(|c| {
                repo::record_event(
                    c,
                    board.id,
                    Some(task.id),
                    Some(s.uid),
                    &EventKind::TaskPurged,
                    Utc::now(),
                )?;
                repo::purge_task(c, task.id)
            })?;
            state.publish_board(
                &board,
                Event::TaskChanged {
                    task,
                    change: TaskChange::Purged,
                    actor: Some(s.uid),
                },
            );
            Ok(Response::Done)
        }
        Request::RestoreTask { task_id } => {
            let (board, task) = load_task(state, s, task_id)?;
            check_task(TaskAction::Restore, &s.actor(), &board, &task)?;
            if !task.is_archived() {
                return Err(AppError::bad("task is not archived"));
            }
            let to = transition::restore_column(&board, &task)
                .ok_or_else(|| AppError::bad("board has no columns"))?;
            let task = state.db.tx(|c| {
                let t = repo::restore_task(c, &task, to, Utc::now())?;
                repo::record_event(
                    c,
                    board.id,
                    Some(t.id),
                    Some(s.uid),
                    &EventKind::TaskRestored { to },
                    Utc::now(),
                )?;
                Ok(t)
            })?;
            state.publish_board(
                &board,
                Event::TaskChanged {
                    task: task.clone(),
                    change: TaskChange::Restored,
                    actor: Some(s.uid),
                },
            );
            Ok(Response::Task { task })
        }
        Request::ListArchived { board_id } => {
            let board = load_board(state, s, board_id)?;
            let tasks = state.db.with(|c| repo::list_tasks(c, board.id, true))?;
            Ok(Response::Tasks { tasks })
        }
        Request::ListTemplates { board_id } => {
            let board = load_board(state, s, board_id)?;
            let templates = state.db.with(|c| repo::list_templates(c, board.id))?;
            Ok(Response::Templates { templates })
        }
        Request::CreateTemplate {
            board_id,
            name,
            draft,
        } => {
            let board = load_board(state, s, board_id)?;
            check_board(BoardAction::AddTask, &s.actor(), &board)?;
            let (name, draft) = template_input(&board, name, draft)?;
            let template = state.db.tx(|c| {
                let t = repo::create_template(c, board.id, s.uid, &name, &draft)?;
                repo::record_event(
                    c,
                    board.id,
                    None,
                    Some(s.uid),
                    &EventKind::TemplateCreated { template: t.id },
                    Utc::now(),
                )?;
                Ok(t)
            })?;
            Ok(Response::Template { template })
        }
        Request::UpdateTemplate {
            template_id,
            name,
            draft,
        } => {
            let (board, tpl) = load_template(state, s, template_id)?;
            check_template_manage(&s.actor(), &board, tpl.owner_uid)?;
            let (name, draft) = template_input(&board, name, draft)?;
            let template = state.db.tx(|c| {
                let t = repo::update_template(c, tpl.id, &name, &draft)?;
                repo::record_event(
                    c,
                    board.id,
                    None,
                    Some(s.uid),
                    &EventKind::TemplateChanged { template: t.id },
                    Utc::now(),
                )?;
                Ok(t)
            })?;
            Ok(Response::Template { template })
        }
        Request::DeleteTemplate { template_id } => {
            let (board, tpl) = load_template(state, s, template_id)?;
            check_template_manage(&s.actor(), &board, tpl.owner_uid)?;
            state.db.tx(|c| {
                repo::record_event(
                    c,
                    board.id,
                    None,
                    Some(s.uid),
                    &EventKind::TemplateDeleted { template: tpl.id },
                    Utc::now(),
                )?;
                repo::delete_template(c, tpl.id)
            })?;
            Ok(Response::Done)
        }
        Request::ListRepeats { board_id } => {
            let board = load_board(state, s, board_id)?;
            let rows = state.db.with(|c| repo::list_repeats(c, board.id))?;
            let entries = rows
                .into_iter()
                .map(|(task, next_fire_at)| RepeatEntry { task, next_fire_at })
                .collect();
            Ok(Response::Repeats { entries })
        }
        Request::StopRepeat { task_id } => {
            // Any member: load_task already requires board view. Stopping a
            // repeat is working the board, like moving a task.
            let (board, task) = load_task(state, s, task_id)?;
            if task.repeat.is_none() {
                return Err(AppError::bad("this task has no repetition"));
            }
            let task = state.db.tx(|c| {
                let t = repo::stop_repeat(c, &task)?;
                repo::record_event(
                    c,
                    board.id,
                    Some(t.id),
                    Some(s.uid),
                    &EventKind::RepeatStopped,
                    Utc::now(),
                )?;
                Ok(t)
            })?;
            state.publish_board(
                &board,
                Event::TaskChanged {
                    task: task.clone(),
                    change: TaskChange::Edited,
                    actor: Some(s.uid),
                },
            );
            Ok(Response::Task { task })
        }
        Request::Scan { payload } => Ok(Response::Scan(scan(state, s, &payload)?)),
        Request::Search {
            query,
            include_archived,
        } => {
            let hits = state
                .db
                .with(|c| repo::search(c, s.uid, &query, include_archived))?;
            Ok(Response::SearchResults { hits })
        }
        Request::PrintTask { task_id } => {
            let (board, task) = load_task(state, s, task_id)?;
            let job = state.db.with(|c| {
                let user = repo::require_user(c, s.uid)?;
                let names = repo::username_map(c)?;
                let deps = repo::dep_lines(c, &task)?;
                Ok(build_task_job(
                    &task,
                    &board,
                    &deps,
                    &|u| name_of(&names, u),
                    &user,
                    Utc::now(),
                ))
            })?;
            state.enqueue_print(s.uid, &job)?;
            Ok(Response::Done)
        }
        Request::AckPrintJob { job_id, error } => {
            match error {
                None => state.db.with(|c| repo::ack_print_ok(c, job_id))?,
                Some(err) => {
                    let ack = state
                        .db
                        .with(|c| repo::ack_print_err(c, job_id, &err, Utc::now()))?;
                    match ack {
                        PrintAck::Requeued { attempts } => state.notify(
                            s.uid,
                            Severity::Warning,
                            format!(
                                "print failed ({err}), will retry, attempt {attempts} of {}",
                                repo::MAX_PRINT_ATTEMPTS
                            ),
                        ),
                        PrintAck::Dropped => state.notify(
                            s.uid,
                            Severity::Error,
                            format!("print job dropped after repeated failures: {err}"),
                        ),
                    }
                }
            }
            Ok(Response::Done)
        }
        Request::SetTimezone { timezone } => {
            let tz: chrono_tz::Tz = timezone.parse().map_err(|_| {
                AppError::bad(format!(
                    "{timezone:?} is not an IANA timezone name like Europe/Berlin"
                ))
            })?;
            state.db.with(|c| repo::update_timezone(c, s.uid, tz))?;
            Ok(Response::Done)
        }
        Request::UpdatePrefs { prefs } => {
            state.db.with(|c| repo::update_prefs(c, s.uid, &prefs))?;
            state.publish_uid(s.uid, Event::PrefsChanged { prefs });
            Ok(Response::Done)
        }
    }
}

fn name_of(names: &std::collections::HashMap<Uid, String>, uid: Uid) -> String {
    names
        .get(&uid)
        .cloned()
        .unwrap_or_else(|| format!("uid {uid}"))
}

/// Load a board the session may see. Anything else is "not found".
fn load_board(
    state: &AppState,
    s: &Session,
    id: taskologic_core::ids::BoardId,
) -> Result<Board, AppError> {
    let board = state.db.with(|c| repo::require_board(c, id))?;
    check_board(BoardAction::View, &s.actor(), &board)?;
    Ok(board)
}

fn board_of_column(state: &AppState, s: &Session, column_id: ColumnId) -> Result<Board, AppError> {
    let board_id = state
        .db
        .with(|c| repo::column_board(c, column_id))?
        .ok_or_else(|| AppError::NotFound("column not found".into()))?;
    load_board(state, s, board_id)
}

fn load_task(state: &AppState, s: &Session, id: TaskId) -> Result<(Board, Task), AppError> {
    let task = state.db.with(|c| repo::require_task(c, id))?;
    let board = state.db.with(|c| repo::require_board(c, task.board_id))?;
    // Task on an invisible board reads as "task not found", same rule as boards.
    check_board(BoardAction::View, &s.actor(), &board)
        .map_err(|_| AppError::NotFound("task not found".into()))?;
    Ok((board, task))
}

/// Template on an invisible board reads as "template not found".
fn load_template(
    state: &AppState,
    s: &Session,
    id: taskologic_core::ids::TemplateId,
) -> Result<(Board, Template), AppError> {
    let tpl = state.db.with(|c| repo::require_template(c, id))?;
    let board = state.db.with(|c| repo::require_board(c, tpl.board_id))?;
    check_board(BoardAction::View, &s.actor(), &board)
        .map_err(|_| AppError::NotFound("template not found".into()))?;
    Ok((board, tpl))
}

/// Validates a template and strips what templates do not carry: a due date
/// and dependencies belong to one instance, not to the kind of task.
fn template_input(
    board: &Board,
    name: String,
    mut draft: TaskDraft,
) -> Result<(String, TaskDraft), AppError> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::bad("template name cannot be empty"));
    }
    draft.due_at = None;
    draft.depends_on.clear();
    taskologic_core::task::validate_draft(&draft, board)?;
    Ok((name, draft))
}

fn columns_changed(state: &AppState, s: &Session, board_id: taskologic_core::ids::BoardId) -> R {
    let board = load_board(state, s, board_id)?;
    state.publish_board_meta(
        &board,
        Event::BoardChanged {
            board_id,
            change: BoardChange::Columns,
        },
    );
    let tasks = state.db.with(|c| repo::list_tasks(c, board_id, false))?;
    Ok(Response::Board(BoardDetail { board, tasks }))
}

/// Dependencies must exist, be visible to the caller, and not form a cycle.
fn check_deps(
    c: &rusqlite::Connection,
    s: &Session,
    task_id: TaskId,
    draft: &TaskDraft,
) -> Result<(), AppError> {
    if draft.depends_on.is_empty() {
        return Ok(());
    }
    for dep in &draft.depends_on {
        let t = repo::get_task(c, *dep)?
            .ok_or_else(|| AppError::bad(format!("dependency {dep} does not exist")))?;
        let b = repo::require_board(c, t.board_id)?;
        if !b.is_member(s.uid) {
            return Err(AppError::bad(format!("dependency {dep} does not exist")));
        }
    }
    let graph = repo::deps_graph(c)?;
    if let Some(bad) = taskologic_core::deps::first_cycle(&graph, task_id, &draft.depends_on) {
        return Err(AppError::bad(format!(
            "depending on task {bad} would create a cycle"
        )));
    }
    Ok(())
}

fn move_task(
    state: &Arc<AppState>,
    s: &Session,
    board: &Board,
    task: &Task,
    to: ColumnId,
    position: Option<i64>,
    override_deps: bool,
) -> Result<Task, AppError> {
    let now = Utc::now();
    let moved = state.db.tx(|c| {
        let open = if to == board.finished_col { repo::open_deps(c, task)? } else { Vec::new() };
        let plan = plan_move(board, task, to, &open, override_deps, now)?;
        let moved = repo::apply_move(c, task, &plan, position, now)?;
        if plan.from == plan.to {
            repo::record_event(c, board.id, Some(task.id), Some(s.uid), &EventKind::TaskReordered, now)?;
        } else {
            // apply_move records the transition without an actor; fix that up
            // by recording who did it alongside.
            c.execute(
                "UPDATE events SET actor_uid = ?2 WHERE id = (SELECT max(id) FROM events WHERE task_id = ?1 AND kind = 'task_moved')",
                rusqlite::params![task.id.0, i64::from(s.uid)],
            )?;
        }
        if !plan.overrode_deps.is_empty() {
            repo::record_event(
                c,
                board.id,
                Some(task.id),
                Some(s.uid),
                &EventKind::DependencyOverridden { open: plan.overrode_deps.clone() },
                now,
            )?;
        }
        Ok((moved, plan))
    })?;
    let (moved, plan) = moved;
    let change = if plan.from == plan.to {
        TaskChange::Reordered
    } else {
        TaskChange::Moved {
            from: plan.from,
            to: plan.to,
        }
    };
    state.publish_board(
        board,
        Event::TaskChanged {
            task: moved.clone(),
            change,
            actor: Some(s.uid),
        },
    );
    if plan.from != plan.to && to == board.started_col {
        autoprint(state, board, &moved, AutoprintTrigger::MovedToStarted);
    }
    Ok(moved)
}

fn scan(state: &Arc<AppState>, s: &Session, payload: &str) -> Result<ScanOutcome, AppError> {
    let parsed = match ScanPayload::parse(payload) {
        Ok(p) => p,
        Err(ScanParseError::NoMagic) => {
            return Ok(ScanOutcome::BadScan {
                reason: "not a Taskologic barcode".into(),
            });
        }
        Err(e) => {
            return Ok(ScanOutcome::BadScan {
                reason: e.to_string(),
            });
        }
    };
    let Some(task) = state
        .db
        .with(|c| repo::task_by_short_id(c, parsed.short_id))?
    else {
        return Ok(ScanOutcome::UnknownTask);
    };
    let board = state.db.with(|c| repo::require_board(c, task.board_id))?;
    if check_task(TaskAction::Move, &s.actor(), &board, &task).is_err() {
        return Ok(ScanOutcome::NoPermission);
    }
    let target = match transition::scan_target(parsed.action, &board, &task) {
        Ok(t) => t,
        Err(r) => {
            return Ok(ScanOutcome::Refused {
                reason: r.to_string(),
            });
        }
    };
    match move_task(state, s, &board, &task, target, None, false) {
        Ok(moved) => {
            state.db.with(|c| {
                repo::record_event(
                    c,
                    board.id,
                    Some(task.id),
                    Some(s.uid),
                    &EventKind::ScanApplied {
                        action: parsed.action,
                    },
                    Utc::now(),
                )
            })?;
            let column_name = board
                .column(target)
                .map(|c| c.name.clone())
                .unwrap_or_default();
            Ok(ScanOutcome::Applied {
                task: Box::new(moved),
                action: parsed.action,
                moved_to: target,
                column_name,
            })
        }
        Err(AppError::Blocked { open }) => Ok(ScanOutcome::BlockedByDependencies {
            task_id: task.id,
            open,
        }),
        Err(e) => Err(e),
    }
}

/// Queue automatic prints for every user whose prefs ask for one. Failures
/// are logged, never surfaced to the user who triggered them.
fn autoprint(state: &Arc<AppState>, board: &Board, task: &Task, trigger: AutoprintTrigger) {
    let result = state.db.with(|c| {
        let names = repo::username_map(c)?;
        let deps = repo::dep_lines(c, task)?;
        // Whoever has had this task already is not asked twice, however many
        // times it passes through the started column.
        let already = repo::autoprinted_uids(c, task.id)?;
        let now = Utc::now();
        let mut jobs = Vec::new();
        for user in repo::list_users(c)? {
            if !board.is_member(user.uid)
                || already.contains(&user.uid)
                || !wants_autoprint(&user.prefs.print, trigger, task, user.uid)
            {
                continue;
            }
            repo::mark_autoprinted(c, task.id, user.uid, now)?;
            jobs.push((
                user.uid,
                build_task_job(task, board, &deps, &|u| name_of(&names, u), &user, now),
            ));
        }
        Ok(jobs)
    });
    match result {
        Ok(jobs) => {
            for (uid, job) in jobs {
                if let Err(e) = state.enqueue_print(uid, &job) {
                    tracing::warn!(uid, error = %e, "could not queue automatic print");
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "autoprint lookup failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::Db;
    use taskologic_core::board::{DEFAULT_ARCHIVE_AFTER_SECS, DEFAULT_PURGE_DELETED_AFTER_SECS};
    use taskologic_proto::{CreateBoard, ErrorBody, ErrorCode};

    fn state() -> Arc<AppState> {
        let st = AppState::new(Config::default(), Db::open_in_memory().unwrap());
        for (uid, name) in [(1, "alice"), (2, "bob"), (3, "carol")] {
            st.db
                .with(|c| {
                    repo::upsert_user_on_login(c, uid, name, chrono_tz::UTC, false, Utc::now())
                })
                .unwrap();
        }
        st
    }

    fn session(uid: Uid) -> Session {
        Session {
            uid,
            is_admin: false,
        }
    }

    fn admin(uid: Uid) -> Session {
        Session {
            uid,
            is_admin: true,
        }
    }

    fn make_board(st: &Arc<AppState>, owner: Uid, private: bool, members: &[Uid]) -> Board {
        let req = CreateBoard {
            name: "Kitchen".into(),
            description: String::new(),
            columns: vec![
                "Todo".into(),
                "Doing".into(),
                "Waiting".into(),
                "Done".into(),
            ],
            started_col: 1,
            paused_col: 2,
            finished_col: 3,
            archive_after_secs: DEFAULT_ARCHIVE_AFTER_SECS,
            purge_deleted_after_secs: DEFAULT_PURGE_DELETED_AFTER_SECS,
            card_fields: Default::default(),
            is_private: private,
            is_locked: false,
            members: members.to_vec(),
        };
        match handle(st, &session(owner), Request::CreateBoard(req)).unwrap() {
            Response::Board(d) => d.board,
            other => panic!("{other:?}"),
        }
    }

    fn make_task(st: &Arc<AppState>, uid: Uid, board: &Board, title: &str) -> Task {
        let draft = TaskDraft {
            title: title.into(),
            ..Default::default()
        };
        match handle(
            st,
            &session(uid),
            Request::CreateTask {
                board_id: board.id,
                column_id: None,
                draft,
            },
        )
        .unwrap()
        {
            Response::Task { task } => task,
            other => panic!("{other:?}"),
        }
    }

    fn code(e: AppError) -> ErrorCode {
        ErrorBody::from(e).code
    }

    /// Turn on "print when a task is started" for this user.
    fn wants_prints_on_start(st: &Arc<AppState>, uid: Uid) {
        st.db
            .with(|c| {
                let mut user = repo::list_users(c)?.into_iter().find(|u| u.uid == uid).unwrap();
                user.prefs.print.mode = taskologic_core::prefs::PrintMode::OnStart;
                repo::update_prefs(c, uid, &user.prefs)?;
                Ok(())
            })
            .unwrap();
    }

    fn queued_slips(st: &Arc<AppState>, uid: Uid) -> usize {
        st.db.with(|c| repo::pending_print_jobs(c, uid, Utc::now())).unwrap().len()
    }

    fn move_to(st: &Arc<AppState>, uid: Uid, task: TaskId, to_column: ColumnId) {
        handle(
            st,
            &session(uid),
            Request::MoveTask { task_id: task, to_column, position: None, override_deps: false },
        )
        .unwrap();
    }

    #[test]
    fn a_task_prints_once_however_often_it_is_started() {
        let st = state();
        let board = make_board(&st, 1, false, &[]);
        wants_prints_on_start(&st, 1);
        let task = make_task(&st, 1, &board, "Dishes");

        move_to(&st, 1, task.id, board.started_col);
        assert_eq!(queued_slips(&st, 1), 1, "starting it prints it");

        // Pause and carry on: the same slip must not come out again.
        move_to(&st, 1, task.id, board.paused_col);
        move_to(&st, 1, task.id, board.started_col);
        assert_eq!(queued_slips(&st, 1), 1, "unpausing reprinted it");

        // Nor when it goes back to the to-do column and is started afresh.
        move_to(&st, 1, task.id, board.columns[0].id);
        move_to(&st, 1, task.id, board.started_col);
        assert_eq!(queued_slips(&st, 1), 1, "restarting reprinted it");
    }

    #[test]
    fn printing_once_is_remembered_per_person_not_per_task() {
        let st = state();
        let board = make_board(&st, 1, false, &[2]);
        wants_prints_on_start(&st, 1);
        let task = make_task(&st, 1, &board, "Dishes");

        move_to(&st, 1, task.id, board.started_col);
        assert_eq!(queued_slips(&st, 1), 1);
        assert_eq!(queued_slips(&st, 2), 0, "bob has not asked for prints");

        // Bob asks for them afterwards, and the next start is his first.
        wants_prints_on_start(&st, 2);
        move_to(&st, 1, task.id, board.paused_col);
        move_to(&st, 1, task.id, board.started_col);
        assert_eq!(queued_slips(&st, 1), 1, "alice already had hers");
        assert_eq!(queued_slips(&st, 2), 1, "bob gets his first");
    }

    #[test]
    fn templates_follow_the_task_rules() {
        let st = state();
        let board = make_board(&st, 1, false, &[]);
        // Any member creates one; the due date and dependencies are stripped.
        let draft = TaskDraft {
            title: "Weekly clean".into(),
            due_at: Some(Utc::now()),
            depends_on: vec![TaskId(99)],
            ..Default::default()
        };
        let tpl = match handle(
            &st,
            &session(2),
            Request::CreateTemplate {
                board_id: board.id,
                name: "Weekly clean".into(),
                draft,
            },
        )
        .unwrap()
        {
            Response::Template { template } => template,
            other => panic!("{other:?}"),
        };
        assert_eq!(tpl.owner_uid, 2);
        assert_eq!(tpl.draft.due_at, None);
        assert!(tpl.draft.depends_on.is_empty());
        // Another plain member cannot change or delete it, says why.
        let draft = TaskDraft {
            title: "Weekly clean".into(),
            ..Default::default()
        };
        let err = handle(
            &st,
            &session(3),
            Request::UpdateTemplate {
                template_id: tpl.id,
                name: "Mine now".into(),
                draft: draft.clone(),
            },
        )
        .unwrap_err();
        assert_eq!(code(err), ErrorCode::PermissionDenied);
        let err = handle(
            &st,
            &session(3),
            Request::DeleteTemplate {
                template_id: tpl.id,
            },
        )
        .unwrap_err();
        assert_eq!(code(err), ErrorCode::PermissionDenied);
        // The board owner can, and the list reflects it.
        handle(
            &st,
            &session(1),
            Request::UpdateTemplate {
                template_id: tpl.id,
                name: "Deep clean".into(),
                draft,
            },
        )
        .unwrap();
        let listed = match handle(
            &st,
            &session(3),
            Request::ListTemplates { board_id: board.id },
        )
        .unwrap()
        {
            Response::Templates { templates } => templates,
            other => panic!("{other:?}"),
        };
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Deep clean");
        handle(
            &st,
            &session(2),
            Request::DeleteTemplate {
                template_id: tpl.id,
            },
        )
        .unwrap();
        match handle(
            &st,
            &session(2),
            Request::ListTemplates { board_id: board.id },
        )
        .unwrap()
        {
            Response::Templates { templates } => assert!(templates.is_empty()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn templates_on_private_boards_read_as_not_found_for_outsiders() {
        let st = state();
        let board = make_board(&st, 1, true, &[1]);
        let draft = TaskDraft {
            title: "Secret".into(),
            ..Default::default()
        };
        let tpl = match handle(
            &st,
            &session(1),
            Request::CreateTemplate {
                board_id: board.id,
                name: "Secret".into(),
                draft,
            },
        )
        .unwrap()
        {
            Response::Template { template } => template,
            other => panic!("{other:?}"),
        };
        let err = handle(
            &st,
            &session(2),
            Request::ListTemplates { board_id: board.id },
        )
        .unwrap_err();
        assert_eq!(code(err), ErrorCode::NotFound);
        let err = handle(
            &st,
            &session(2),
            Request::DeleteTemplate {
                template_id: tpl.id,
            },
        )
        .unwrap_err();
        assert_eq!(code(err), ErrorCode::NotFound);
    }

    #[test]
    fn the_repeat_list_shows_next_fire_and_any_member_can_stop_it() {
        use taskologic_core::repeat::{Repeat, RepeatSpec};
        let st = state();
        let board = make_board(&st, 1, false, &[]);
        let draft = TaskDraft {
            title: "Water plants".into(),
            repeat: Some(RepeatSpec {
                rule: Repeat::EveryDays {
                    every: 2,
                    from: Utc::now().date_naive(),
                },
                at: chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                tz: chrono_tz::UTC,
            }),
            ..Default::default()
        };
        let task = match handle(
            &st,
            &session(1),
            Request::CreateTask {
                board_id: board.id,
                column_id: None,
                draft,
            },
        )
        .unwrap()
        {
            Response::Task { task } => task,
            other => panic!("{other:?}"),
        };
        let entries = match handle(
            &st,
            &session(2),
            Request::ListRepeats { board_id: board.id },
        )
        .unwrap()
        {
            Response::Repeats { entries } => entries,
            other => panic!("{other:?}"),
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].task.id, task.id);
        assert!(
            entries[0].next_fire_at.is_some(),
            "a fresh repeat has a next fire time"
        );
        // Any member stops it, the version bumps so editors notice.
        let stopped =
            match handle(&st, &session(2), Request::StopRepeat { task_id: task.id }).unwrap() {
                Response::Task { task } => task,
                other => panic!("{other:?}"),
            };
        assert_eq!(stopped.repeat, None);
        assert_eq!(stopped.version, task.version + 1);
        match handle(
            &st,
            &session(1),
            Request::ListRepeats { board_id: board.id },
        )
        .unwrap()
        {
            Response::Repeats { entries } => assert!(entries.is_empty()),
            other => panic!("{other:?}"),
        }
        let err = handle(&st, &session(1), Request::StopRepeat { task_id: task.id }).unwrap_err();
        assert_eq!(code(err), ErrorCode::BadRequest);
    }

    #[test]
    fn checklist_survives_the_round_trip_and_items_toggle() {
        use taskologic_core::task::ChecklistItem;
        let st = state();
        let board = make_board(&st, 1, false, &[]);
        let draft = TaskDraft {
            title: "Party".into(),
            checklist: vec![
                ChecklistItem {
                    text: "balloons".into(),
                    done: false,
                },
                ChecklistItem {
                    text: "cake".into(),
                    done: true,
                },
            ],
            ..Default::default()
        };
        let task = match handle(
            &st,
            &session(1),
            Request::CreateTask {
                board_id: board.id,
                column_id: None,
                draft,
            },
        )
        .unwrap()
        {
            Response::Task { task } => task,
            other => panic!("{other:?}"),
        };
        assert_eq!(task.checklist.len(), 2);
        assert!(task.checklist[1].done);
        // Any member can tick, no version needed, the version still bumps.
        let ticked = match handle(
            &st,
            &session(2),
            Request::SetChecklistItem {
                task_id: task.id,
                index: 0,
                done: true,
            },
        )
        .unwrap()
        {
            Response::Task { task } => task,
            other => panic!("{other:?}"),
        };
        assert!(ticked.checklist[0].done);
        assert_eq!(ticked.version, task.version + 1);
        let err = handle(
            &st,
            &session(2),
            Request::SetChecklistItem {
                task_id: task.id,
                index: 9,
                done: true,
            },
        )
        .unwrap_err();
        assert_eq!(code(err), ErrorCode::BadRequest);
    }

    #[test]
    fn first_user_is_admin_and_later_ones_are_not() {
        let st = state();
        let (a, b) = st
            .db
            .with(|c| Ok((repo::require_user(c, 1)?, repo::require_user(c, 2)?)))
            .unwrap();
        assert!(a.is_admin);
        assert!(!b.is_admin);
    }

    #[test]
    fn private_boards_are_invisible_to_non_members() {
        let st = state();
        let board = make_board(&st, 1, true, &[2]);
        assert!(handle(&st, &session(2), Request::GetBoard { board_id: board.id }).is_ok());
        let err = handle(&st, &session(3), Request::GetBoard { board_id: board.id }).unwrap_err();
        assert_eq!(code(err), ErrorCode::NotFound);
        let listed = match handle(&st, &session(3), Request::ListBoards).unwrap() {
            Response::Boards { boards } => boards,
            _ => unreachable!(),
        };
        assert!(listed.is_empty());
        // Tasks on it are equally invisible.
        let task = make_task(&st, 1, &board, "Secret");
        let err = handle(
            &st,
            &session(3),
            Request::MoveTask {
                task_id: task.id,
                to_column: board.started_col,
                position: None,
                override_deps: false,
            },
        )
        .unwrap_err();
        assert_eq!(code(err), ErrorCode::NotFound);
    }

    #[test]
    fn move_records_events_and_starts_the_archive_clock() {
        let st = state();
        let board = make_board(&st, 1, false, &[]);
        let task = make_task(&st, 2, &board, "Dishes");
        let moved = match handle(
            &st,
            &session(3),
            Request::MoveTask {
                task_id: task.id,
                to_column: board.finished_col,
                position: None,
                override_deps: false,
            },
        )
        .unwrap()
        {
            Response::Task { task } => task,
            _ => unreachable!(),
        };
        assert_eq!(moved.column_id, board.finished_col);
        assert!(moved.finished_at.is_some());
        assert_eq!(moved.version, task.version + 1);
        let n = st
            .db
            .with(|c| repo::event_count(c, task.id, "task_moved"))
            .unwrap();
        assert_eq!(n, 1);
        let actor: Option<i64> = st
            .db
            .with(|c| {
                Ok(c.query_row(
                    "SELECT actor_uid FROM events WHERE task_id = ?1 AND kind = 'task_moved'",
                    [task.id.0],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(actor, Some(3), "any member may move, and the log says who");
    }

    #[test]
    fn delete_is_an_archive_move_and_purge_is_for_owners() {
        let st = state();
        let board = make_board(&st, 1, false, &[]);
        let task = make_task(&st, 2, &board, "Dishes");
        let err = handle(&st, &session(3), Request::DeleteTask { task_id: task.id }).unwrap_err();
        assert_eq!(code(err), ErrorCode::PermissionDenied);
        let deleted =
            match handle(&st, &session(2), Request::DeleteTask { task_id: task.id }).unwrap() {
                Response::Task { task } => task,
                other => panic!("{other:?}"),
            };
        assert!(deleted.is_archived() && deleted.is_deleted());
        assert_eq!(deleted.archived_from_col, Some(board.columns[0].id));
        assert_eq!(
            st.db
                .with(|c| repo::event_count(c, task.id, "task_deleted"))
                .unwrap(),
            1
        );
        let archived = match handle(
            &st,
            &session(3),
            Request::ListArchived { board_id: board.id },
        )
        .unwrap()
        {
            Response::Tasks { tasks } => tasks,
            _ => unreachable!(),
        };
        assert_eq!(archived.len(), 1);

        // Anyone on the board can restore, only owner or admin can purge.
        let err = handle(&st, &session(2), Request::PurgeTask { task_id: task.id }).unwrap_err();
        assert_eq!(code(err), ErrorCode::PermissionDenied);
        let restored =
            match handle(&st, &session(3), Request::RestoreTask { task_id: task.id }).unwrap() {
                Response::Task { task } => task,
                _ => unreachable!(),
            };
        assert!(!restored.is_archived() && !restored.is_deleted());
        let err = handle(&st, &session(1), Request::PurgeTask { task_id: task.id }).unwrap_err();
        assert_eq!(
            code(err),
            ErrorCode::BadRequest,
            "live tasks cannot be purged"
        );
        handle(&st, &session(1), Request::DeleteTask { task_id: task.id }).unwrap();
        handle(&st, &session(1), Request::PurgeTask { task_id: task.id }).unwrap();
        assert_eq!(
            code(handle(&st, &session(1), Request::RestoreTask { task_id: task.id }).unwrap_err()),
            ErrorCode::NotFound
        );
        assert_eq!(
            st.db
                .with(|c| repo::event_count(c, task.id, "task_purged"))
                .unwrap(),
            1,
            "audit log survives the row"
        );
    }

    #[test]
    fn deleting_a_repeating_task_pauses_the_repeat_until_restored() {
        let st = state();
        let board = make_board(&st, 1, false, &[]);
        let repeat = taskologic_core::repeat::RepeatSpec {
            rule: taskologic_core::repeat::Repeat::EveryDays {
                every: 1,
                from: chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap(),
            },
            at: chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            tz: chrono_tz::UTC,
        };
        let draft = TaskDraft {
            title: "Water".into(),
            repeat: Some(repeat),
            ..Default::default()
        };
        let task = match handle(
            &st,
            &session(1),
            Request::CreateTask {
                board_id: board.id,
                column_id: None,
                draft,
            },
        )
        .unwrap()
        {
            Response::Task { task } => task,
            _ => unreachable!(),
        };
        handle(&st, &session(1), Request::DeleteTask { task_id: task.id }).unwrap();
        let active: i64 = st
            .db
            .with(|c| {
                Ok(c.query_row(
                    "SELECT active FROM repeats WHERE task_id = ?1",
                    [task.id.0],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(active, 0);
        let restored =
            match handle(&st, &session(1), Request::RestoreTask { task_id: task.id }).unwrap() {
                Response::Task { task } => task,
                _ => unreachable!(),
            };
        assert!(restored.repeat.is_some());
    }

    #[test]
    fn only_owner_or_admin_delete_boards_and_locked_ones_need_unlocking() {
        let st = state();
        let board = make_board(&st, 1, false, &[]);
        assert_eq!(
            code(
                handle(
                    &st,
                    &session(3),
                    Request::DeleteBoard { board_id: board.id }
                )
                .unwrap_err()
            ),
            ErrorCode::PermissionDenied
        );
        let lock = taskologic_proto::UpdateBoard {
            board_id: board.id,
            name: None,
            description: None,
            is_locked: Some(true),
            is_private: None,
            archive_after_secs: None,
            purge_deleted_after_secs: None,
            card_fields: None,
            roles: vec![],
        };
        handle(&st, &session(1), Request::UpdateBoard(lock)).unwrap();
        let err = handle(
            &st,
            &session(1),
            Request::DeleteBoard { board_id: board.id },
        )
        .unwrap_err();
        assert_eq!(code(err), ErrorCode::PermissionDenied);
        let unlock = taskologic_proto::UpdateBoard {
            board_id: board.id,
            name: None,
            description: None,
            is_locked: Some(false),
            is_private: None,
            archive_after_secs: None,
            purge_deleted_after_secs: None,
            card_fields: None,
            roles: vec![],
        };
        handle(&st, &session(1), Request::UpdateBoard(unlock)).unwrap();
        handle(
            &st,
            &session(1),
            Request::DeleteBoard { board_id: board.id },
        )
        .unwrap();
    }

    #[test]
    fn admins_can_delete_private_boards_they_cannot_open() {
        let st = state();
        let board = make_board(&st, 2, true, &[2]);
        let err = handle(&st, &admin(1), Request::GetBoard { board_id: board.id }).unwrap_err();
        assert_eq!(
            code(err),
            ErrorCode::PermissionDenied,
            "admins learn it exists, not what is in it"
        );
        let listed = match handle(&st, &admin(1), Request::ListBoards).unwrap() {
            Response::Boards { boards } => boards,
            _ => unreachable!(),
        };
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].is_member);
        let lock = taskologic_proto::UpdateBoard {
            board_id: board.id,
            name: None,
            description: None,
            is_locked: Some(true),
            is_private: None,
            archive_after_secs: None,
            purge_deleted_after_secs: None,
            card_fields: None,
            roles: vec![],
        };
        let locked = match handle(&st, &admin(1), Request::UpdateBoard(lock)).unwrap() {
            Response::Board(d) => d,
            _ => unreachable!(),
        };
        assert!(locked.board.is_locked);
        assert!(locked.tasks.is_empty(), "content withheld");
        let unlock = taskologic_proto::UpdateBoard {
            board_id: board.id,
            name: None,
            description: None,
            is_locked: Some(false),
            is_private: None,
            archive_after_secs: None,
            purge_deleted_after_secs: None,
            card_fields: None,
            roles: vec![],
        };
        handle(&st, &admin(1), Request::UpdateBoard(unlock)).unwrap();
        handle(&st, &admin(1), Request::DeleteBoard { board_id: board.id }).unwrap();
        // A plain user still sees nothing of it.
        let board = make_board(&st, 2, true, &[2]);
        assert_eq!(
            code(
                handle(
                    &st,
                    &session(3),
                    Request::DeleteBoard { board_id: board.id }
                )
                .unwrap_err()
            ),
            ErrorCode::NotFound
        );
    }

    #[test]
    fn open_dependencies_block_finishing_unless_overridden() {
        let st = state();
        let board = make_board(&st, 1, false, &[]);
        let dep = make_task(&st, 1, &board, "Buy soap");
        let draft = TaskDraft {
            title: "Dishes".into(),
            depends_on: vec![dep.id],
            ..Default::default()
        };
        let task = match handle(
            &st,
            &session(1),
            Request::CreateTask {
                board_id: board.id,
                column_id: None,
                draft,
            },
        )
        .unwrap()
        {
            Response::Task { task } => task,
            _ => unreachable!(),
        };
        let err = handle(
            &st,
            &session(1),
            Request::MoveTask {
                task_id: task.id,
                to_column: board.finished_col,
                position: None,
                override_deps: false,
            },
        )
        .unwrap_err();
        assert!(matches!(err, AppError::Blocked { ref open } if *open == vec![dep.id]));
        handle(
            &st,
            &session(1),
            Request::MoveTask {
                task_id: task.id,
                to_column: board.finished_col,
                position: None,
                override_deps: true,
            },
        )
        .unwrap();
        assert_eq!(
            st.db
                .with(|c| repo::event_count(c, task.id, "dependency_overridden"))
                .unwrap(),
            1
        );
        // A cycle is refused up front.
        let draft = TaskDraft {
            title: "Buy soap".into(),
            depends_on: vec![task.id],
            ..Default::default()
        };
        let err = handle(
            &st,
            &session(1),
            Request::UpdateTask {
                task_id: dep.id,
                version: dep.version,
                draft,
            },
        )
        .unwrap_err();
        assert_eq!(code(err), ErrorCode::BadRequest);
    }

    #[test]
    fn stale_version_is_a_conflict_with_the_current_task() {
        let st = state();
        let board = make_board(&st, 1, false, &[]);
        let task = make_task(&st, 1, &board, "Dishes");
        let draft = TaskDraft {
            title: "Dishes, properly".into(),
            ..Default::default()
        };
        handle(
            &st,
            &session(1),
            Request::UpdateTask {
                task_id: task.id,
                version: task.version,
                draft: draft.clone(),
            },
        )
        .unwrap();
        let err = handle(
            &st,
            &session(1),
            Request::UpdateTask {
                task_id: task.id,
                version: task.version,
                draft,
            },
        )
        .unwrap_err();
        match err {
            AppError::Conflict { current } => assert_eq!(current.title, "Dishes, properly"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn scan_toggles_and_reports_distinct_outcomes() {
        let st = state();
        let board = make_board(&st, 1, true, &[2]);
        let task = make_task(&st, 1, &board, "Dishes");
        let start = ScanPayload {
            action: taskologic_core::barcode::ScanAction::StartPause,
            short_id: task.short_id,
        }
        .encode(taskologic_core::barcode::Magic::Dots);
        let finish = ScanPayload {
            action: taskologic_core::barcode::ScanAction::Finish,
            short_id: task.short_id,
        }
        .encode(taskologic_core::barcode::Magic::Dots);

        let out = scan(&st, &session(2), &start).unwrap();
        assert!(
            matches!(&out, ScanOutcome::Applied { moved_to, .. } if *moved_to == board.started_col),
            "{out:?}"
        );
        let out = scan(&st, &session(2), &start).unwrap();
        assert!(
            matches!(&out, ScanOutcome::Applied { moved_to, .. } if *moved_to == board.paused_col)
        );
        assert!(matches!(
            scan(&st, &session(3), &start).unwrap(),
            ScanOutcome::NoPermission
        ));
        assert!(matches!(
            scan(&st, &session(2), "..1S000000A").unwrap(),
            ScanOutcome::UnknownTask | ScanOutcome::BadScan { .. }
        ));
        assert!(matches!(
            scan(&st, &session(2), "4006381333931").unwrap(),
            ScanOutcome::BadScan { .. }
        ));
        assert!(matches!(
            scan(&st, &session(2), &finish).unwrap(),
            ScanOutcome::Applied { .. }
        ));
        assert!(matches!(
            scan(&st, &session(2), &start).unwrap(),
            ScanOutcome::Refused { .. }
        ));
        assert_eq!(
            st.db
                .with(|c| repo::event_count(c, task.id, "scan_applied"))
                .unwrap(),
            3
        );
    }

    #[test]
    fn removing_a_member_with_tasks_asks_first() {
        let st = state();
        let board = make_board(&st, 1, true, &[2]);
        let draft = TaskDraft {
            title: "Dishes".into(),
            assignees: vec![2],
            ..Default::default()
        };
        let task = match handle(
            &st,
            &session(1),
            Request::CreateTask {
                board_id: board.id,
                column_id: None,
                draft,
            },
        )
        .unwrap()
        {
            Response::Task { task } => task,
            _ => unreachable!(),
        };
        let err = handle(
            &st,
            &session(1),
            Request::RemoveMember {
                board_id: board.id,
                uid: 2,
                unassign: false,
            },
        )
        .unwrap_err();
        assert!(matches!(err, AppError::MemberHasTasks(ref t) if *t == vec![task.id]));
        handle(
            &st,
            &session(1),
            Request::RemoveMember {
                board_id: board.id,
                uid: 2,
                unassign: true,
            },
        )
        .unwrap();
        let t = st.db.with(|c| repo::require_task(c, task.id)).unwrap();
        assert!(t.assignees.is_empty());
        assert_eq!(
            code(handle(&st, &session(2), Request::GetBoard { board_id: board.id }).unwrap_err()),
            ErrorCode::NotFound
        );
    }

    #[test]
    fn going_private_keeps_assignees_as_members() {
        let st = state();
        let board = make_board(&st, 1, false, &[]);
        let draft = TaskDraft {
            title: "Dishes".into(),
            assignees: vec![2],
            ..Default::default()
        };
        handle(
            &st,
            &session(1),
            Request::CreateTask {
                board_id: board.id,
                column_id: None,
                draft,
            },
        )
        .unwrap();
        let req = taskologic_proto::UpdateBoard {
            board_id: board.id,
            name: None,
            description: None,
            is_locked: None,
            is_private: Some(true),
            archive_after_secs: None,
            purge_deleted_after_secs: None,
            card_fields: None,
            roles: vec![],
        };
        let after = match handle(&st, &session(1), Request::UpdateBoard(req)).unwrap() {
            Response::Board(d) => d.board,
            _ => unreachable!(),
        };
        assert!(after.is_private);
        assert!(after.is_member(2), "assignee carried over");
        assert!(!after.is_member(3));
    }

    #[test]
    fn search_respects_visibility_and_archive_flag() {
        let st = state();
        let public = make_board(&st, 1, false, &[]);
        let private = make_board(&st, 1, true, &[]);
        make_task(&st, 1, &public, "Water the plants");
        make_task(&st, 1, &private, "Water the secret plants");
        let hits = match handle(
            &st,
            &session(2),
            Request::Search {
                query: "plants".into(),
                include_archived: false,
            },
        )
        .unwrap()
        {
            Response::SearchResults { hits } => hits,
            _ => unreachable!(),
        };
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].board_id, public.id);
        let hits = match handle(
            &st,
            &session(2),
            Request::Search {
                query: "100%".into(),
                include_archived: false,
            },
        )
        .unwrap()
        {
            Response::SearchResults { hits } => hits,
            _ => unreachable!(),
        };
        assert!(hits.is_empty(), "wildcards are literal");
    }
}
