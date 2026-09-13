//! Background work that has to happen when nobody is logged in: archiving,
//! repeating tasks, reminders, and print queue housekeeping.

use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeDelta, Utc};
use taskologic_core::event::EventKind;
use taskologic_core::print::{build_reminder_job, reminder_at};
use taskologic_core::repeat::next_instance;
use taskologic_proto::{Event, TaskChange};

use crate::db::repo;
use crate::error::AppError;
use crate::state::AppState;

pub async fn run(state: Arc<AppState>) {
    let tick = Duration::from_secs(state.cfg.scheduler_tick_secs.max(1));
    loop {
        if let Err(e) = tick_once(&state) {
            tracing::error!(error = %e, "scheduler tick failed");
        }
        tokio::time::sleep(tick).await;
    }
}

/// One pass over everything. Public so tests can drive it.
pub fn tick_once(state: &Arc<AppState>) -> Result<(), AppError> {
    archive_finished(state)?;
    purge_deleted(state)?;
    fire_repeats(state)?;
    print_reminders(state)?;
    print_queue_housekeeping(state)?;
    Ok(())
}

fn archive_finished(state: &Arc<AppState>) -> Result<(), AppError> {
    let now = Utc::now();
    let due = state.db.with(|c| repo::tasks_due_for_archive(c, now))?;
    for task in due {
        let board = state.db.with(|c| repo::require_board(c, task.board_id))?;
        let archived = state.db.tx(|c| {
            let t = repo::archive_task(c, &task, now)?;
            repo::record_event(
                c,
                board.id,
                Some(t.id),
                None,
                &EventKind::TaskArchived {
                    from: task.column_id,
                },
                now,
            )?;
            Ok(t)
        })?;
        state.publish_board(
            &board,
            Event::TaskChanged {
                task: archived,
                change: TaskChange::Archived,
                actor: None,
            },
        );
    }
    Ok(())
}

/// Deleted tasks past their board's retention period go for good.
fn purge_deleted(state: &Arc<AppState>) -> Result<(), AppError> {
    let now = Utc::now();
    for task in state.db.with(|c| repo::tasks_due_for_purge(c, now))? {
        let board = state.db.with(|c| repo::require_board(c, task.board_id))?;
        state.db.tx(|c| {
            repo::record_event(
                c,
                board.id,
                Some(task.id),
                None,
                &EventKind::TaskPurged,
                now,
            )?;
            repo::purge_task(c, task.id)
        })?;
        state.publish_board(
            &board,
            Event::TaskChanged {
                task,
                change: TaskChange::Purged,
                actor: None,
            },
        );
    }
    Ok(())
}

fn fire_repeats(state: &Arc<AppState>) -> Result<(), AppError> {
    let now = Utc::now();
    let due = state.db.with(|c| repo::due_repeats(c, now))?;
    for (task, spec) in due {
        let board = state.db.with(|c| repo::require_board(c, task.board_id))?;
        let next = spec.next_fire_after(now);
        if !task.is_done(&board) {
            // Not finished yet: nothing happens, try again next occurrence.
            state.db.with(|c| repo::advance_repeat(c, task.id, next))?;
            continue;
        }
        let Some(first_col) = board.first_column().map(|c| c.id) else {
            continue;
        };
        let created = state.db.tx(|c| {
            let draft = next_instance(&task, &board);
            let created = repo::create_task(c, &board, first_col, &draft, task.created_by, now)?;
            repo::record_event(
                c,
                board.id,
                Some(created.id),
                None,
                &EventKind::RepeatSpawned { from_task: task.id },
                now,
            )?;
            // The chain moves on: the old instance stops repeating, the new one carries it.
            repo::advance_repeat(c, task.id, None)?;
            Ok(created)
        })?;
        state.publish_board(
            &board,
            Event::TaskChanged {
                task: created,
                change: TaskChange::Created,
                actor: None,
            },
        );
    }
    Ok(())
}

fn print_reminders(state: &Arc<AppState>) -> Result<(), AppError> {
    let now = Utc::now();
    let users = state.db.with(repo::list_users)?;
    // Deliberately not filtered on a default lead time: a task can carry its
    // own override, which has to fire for someone who has no default at all.
    for user in users {
        let tasks = state.db.with(|c| repo::reminder_candidates(c, user.uid))?;
        for task in tasks {
            let (Some(at), Some(due)) = (reminder_at(&task, &user.prefs.print), task.due_at) else {
                continue;
            };
            if at > now || due <= now {
                continue;
            }
            if state
                .db
                .with(|c| repo::reminder_sent(c, task.id, user.uid, due))?
            {
                continue;
            }
            let board = state.db.with(|c| repo::require_board(c, task.board_id))?;
            if !board.is_member(user.uid) {
                continue;
            }
            let job = build_reminder_job(&task, &board, &user, now);
            state
                .db
                .with(|c| repo::mark_reminder_sent(c, task.id, user.uid, due, now))?;
            state.enqueue_print(user.uid, &job)?;
        }
    }
    Ok(())
}

fn print_queue_housekeeping(state: &Arc<AppState>) -> Result<(), AppError> {
    let now = Utc::now();
    let expired = state.db.with(|c| repo::expire_print_jobs(c, now))?;
    if expired > 0 {
        tracing::info!(expired, "dropped stale print jobs");
    }
    // A client that took a job and vanished without an ack: give it back.
    let stale = state
        .db
        .with(|c| repo::release_stale_in_flight(c, now - TimeDelta::minutes(2)))?;
    if stale > 0 {
        tracing::info!(stale, "released print jobs from vanished clients");
    }
    for uid in state
        .db
        .with(|c| repo::uids_with_pending_print_jobs(c, now))?
    {
        state.dispatch_print_jobs(uid)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::Db;
    use crate::handlers::{Session, handle};
    use taskologic_core::board::{DEFAULT_ARCHIVE_AFTER_SECS, DEFAULT_PURGE_DELETED_AFTER_SECS};
    use taskologic_core::task::TaskDraft;
    use taskologic_proto::{CreateBoard, Request, Response};

    #[test]
    fn finished_tasks_get_archived_after_the_delay() {
        let st = AppState::new(Config::default(), Db::open_in_memory().unwrap());
        st.db
            .with(|c| repo::upsert_user_on_login(c, 1, "alice", chrono_tz::UTC, false, Utc::now()))
            .unwrap();
        let s = Session {
            uid: 1,
            is_admin: true,
        };
        let req = CreateBoard {
            name: "B".into(),
            description: String::new(),
            columns: vec!["Todo".into(), "Doing".into(), "Wait".into(), "Done".into()],
            started_col: 1,
            paused_col: 2,
            finished_col: 3,
            archive_after_secs: DEFAULT_ARCHIVE_AFTER_SECS,
            purge_deleted_after_secs: DEFAULT_PURGE_DELETED_AFTER_SECS,
            card_fields: Default::default(),
            is_private: false,
            is_locked: false,
            members: vec![],
        };
        let board = match handle(&st, &s, Request::CreateBoard(req)).unwrap() {
            Response::Board(d) => d.board,
            _ => unreachable!(),
        };
        let draft = TaskDraft {
            title: "T".into(),
            ..Default::default()
        };
        let task = match handle(
            &st,
            &s,
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
        handle(
            &st,
            &s,
            Request::MoveTask {
                task_id: task.id,
                to_column: board.finished_col,
                position: None,
                override_deps: false,
            },
        )
        .unwrap();

        tick_once(&st).unwrap();
        assert!(
            !st.db
                .with(|c| repo::require_task(c, task.id))
                .unwrap()
                .is_archived(),
            "not yet"
        );

        // Backdate the finish and tick again.
        st.db
            .with(|c| {
                Ok(c.execute(
                    "UPDATE tasks SET finished_at = finished_at - ?2 WHERE id = ?1",
                    rusqlite::params![task.id.0, DEFAULT_ARCHIVE_AFTER_SECS + 1],
                )?)
            })
            .unwrap();
        tick_once(&st).unwrap();
        let t = st.db.with(|c| repo::require_task(c, task.id)).unwrap();
        assert!(t.is_archived());
        assert_eq!(t.archived_from_col, Some(board.finished_col));
        assert_eq!(
            st.db
                .with(|c| repo::event_count(c, task.id, "task_archived"))
                .unwrap(),
            1
        );

        // Restore puts it back where it was, clock stopped.
        let restored = match handle(&st, &s, Request::RestoreTask { task_id: task.id }).unwrap() {
            Response::Task { task } => task,
            _ => unreachable!(),
        };
        assert!(!restored.is_archived());
        assert_eq!(restored.column_id, board.finished_col);
        assert_eq!(restored.finished_at, None);

        // Deleted tasks wait out the retention period, then go for good.
        handle(&st, &s, Request::DeleteTask { task_id: task.id }).unwrap();
        tick_once(&st).unwrap();
        assert!(
            st.db
                .with(|c| repo::require_task(c, task.id))
                .unwrap()
                .is_deleted(),
            "still in the archive"
        );
        st.db
            .with(|c| {
                Ok(c.execute(
                    "UPDATE tasks SET deleted_at = deleted_at - ?2 WHERE id = ?1",
                    rusqlite::params![task.id.0, DEFAULT_PURGE_DELETED_AFTER_SECS + 1],
                )?)
            })
            .unwrap();
        tick_once(&st).unwrap();
        assert!(
            st.db
                .with(|c| repo::get_task(c, task.id))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            st.db
                .with(|c| repo::event_count(c, task.id, "task_purged"))
                .unwrap(),
            1
        );
    }
}
