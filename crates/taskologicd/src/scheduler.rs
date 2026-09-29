//! Background work that has to happen when nobody is logged in: archiving,
//! repeating tasks, reminders, and print queue housekeeping.

use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeDelta, Utc};
use taskologic_core::event::EventKind;
use taskologic_core::print::{
    PrintJobKind, ReminderKind, build_reminder_job, build_task_job, plan_reminders,
};
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
    auto_start(state)?;
    print_reminders(state)?;
    crate::slips::print_dated(state)?;
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
            // Dated from the moment the repetition fired, by its own rules.
            let draft = next_instance(&task, &board, now);
            let created = repo::create_task(
                c,
                &board,
                first_col,
                &draft,
                task.created_by,
                // A repeated task inherits the chain, not the template: the
                // copy is a sibling of the instance before it.
                task.template_id,
                now,
            )?;
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

/// Tasks that asked to start themselves when their start date came. Each
/// goes into the started column the way a person would move it, so a
/// program run reacts and the prints fire, only with nobody named as the
/// actor. One held back by open dependencies is left for a person.
fn auto_start(state: &Arc<AppState>) -> Result<(), AppError> {
    let now = Utc::now();
    for task in state.db.with(|c| repo::auto_start_due(c, now))? {
        let board = state.db.with(|c| repo::require_board(c, task.board_id))?;
        match crate::handlers::move_task_by(
            state,
            None,
            &board,
            &task,
            board.started_col,
            None,
            false,
            None,
        ) {
            Ok(_) | Err(AppError::Blocked { .. }) => {}
            Err(e) => tracing::warn!(task = %task.id, error = %e, "auto start failed"),
        }
    }
    Ok(())
}

/// Print what the clock says is owed.
///
/// A task with one date earns a reminder before it. A task with both earns a
/// reminder before its start date and the task slip, the one carrying the
/// finish barcode, before its due date. The rules themselves live in
/// `taskologic_core::print::plan_reminders`; what is here is the clock, the
/// permissions and the bookkeeping that stops a slip printing twice.
fn print_reminders(state: &Arc<AppState>) -> Result<(), AppError> {
    let now = Utc::now();
    // How late a slip may be and still be worth printing. A daemon that was
    // off over the weekend must not come back and spool every reminder it
    // missed, but a lead time of zero has to survive the gap between ticks.
    let grace = TimeDelta::seconds(state.cfg.scheduler_tick_secs.max(1) as i64);
    let users = state.db.with(repo::list_users)?;
    // Deliberately not filtered on a default lead time: a task can carry its
    // own override, which has to fire for someone who has no default at all.
    for user in users {
        let tasks = state.db.with(|c| repo::reminder_candidates(c, user.uid))?;
        for task in tasks {
            let board = state.db.with(|c| repo::require_board(c, task.board_id))?;
            if !board.is_member(user.uid) {
                continue;
            }
            // A task with rules of its own has said how it prints, and the
            // prefs stand down for it. Its dated rules run in `slips`.
            if !task.print_rules.is_empty() {
                continue;
            }
            for planned in plan_reminders(&task, &user.prefs.print) {
                let anchor = match planned.kind {
                    ReminderKind::Start => task.start_at,
                    ReminderKind::Due => task.due_at,
                };
                let Some(anchor) = anchor else { continue };
                if planned.at > now || now > anchor + grace {
                    continue;
                }
                if state.db.with(|c| {
                    repo::reminder_sent(c, task.id, user.uid, planned.kind, anchor)
                })? {
                    continue;
                }
                // Telling somebody to start what they already started is
                // noise. Where the task sits now cannot answer that, because
                // a board may have columns beyond the started one.
                if planned.kind == ReminderKind::Start
                    && state
                        .db
                        .with(|c| repo::was_ever_started(c, task.id, board.started_col))?
                {
                    continue;
                }
                let job = match planned.job {
                    // A reminder never plans a sheet; the arm keeps the
                    // match honest if a kind is ever added to the planner.
                    PrintJobKind::Reminder | PrintJobKind::Sheet => {
                        build_reminder_job(&task, &board, &user, now)
                    }
                    // The slip you work from, so nobody gets two of them:
                    // whoever already has one, by autoprint or by hand, is
                    // not handed another.
                    PrintJobKind::Task => {
                        let already = state
                            .db
                            .with(|c| repo::autoprinted_uids(c, task.id))?
                            .contains(&user.uid);
                        if already {
                            // Still recorded, or every tick would ask again.
                            state.db.with(|c| {
                                repo::mark_reminder_sent(
                                    c,
                                    task.id,
                                    user.uid,
                                    planned.kind,
                                    anchor,
                                    now,
                                )
                            })?;
                            continue;
                        }
                        state.db.with(|c| {
                            let names = repo::username_map(c)?;
                            let deps = repo::dep_lines(c, &task)?;
                            repo::mark_autoprinted(c, task.id, user.uid, now)?;
                            Ok(build_task_job(
                                &task,
                                &board,
                                &deps,
                                &|u| {
                                    names
                                        .get(&u)
                                        .cloned()
                                        .unwrap_or_else(|| format!("uid {u}"))
                                },
                                &user,
                                now,
                            ))
                        })?
                    }
                };
                state.db.with(|c| {
                    repo::mark_reminder_sent(c, task.id, user.uid, planned.kind, anchor, now)
                })?;
                state.enqueue_print(user.uid, &job)?;
            }
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
    use taskologic_core::board::{
        Board, ColumnRole, DEFAULT_ARCHIVE_AFTER_SECS, DEFAULT_PURGE_DELETED_AFTER_SECS,
    };
    use taskologic_core::offset::{Offset, OffsetUnit};
    use taskologic_core::print::{PrintRule, PrintWhen, Recipients, SlipKind};
    use taskologic_core::task::TaskDraft;
    use taskologic_proto::{CreateBoard, Request, Response};

    /// A state with one user, one board, and the two reminder lead times set.
    fn reminder_setup(start_lead: Option<u32>, due_lead: Option<u32>) -> (Arc<AppState>, Board) {
        let st = AppState::new(Config::default(), Db::open_in_memory().unwrap());
        st.db
            .with(|c| repo::upsert_user_on_login(c, 1, "alice", chrono_tz::UTC, false, Utc::now()))
            .unwrap();
        st.db
            .with(|c| {
                let mut user = repo::require_user(c, 1)?;
                user.prefs.print.reminder_start_minutes = start_lead;
                user.prefs.print.reminder_due_minutes = due_lead;
                repo::update_prefs(c, 1, &user.prefs)?;
                Ok(())
            })
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
            other => panic!("{other:?}"),
        };
        (st, board)
    }

    fn dated_task(
        st: &Arc<AppState>,
        board: &Board,
        start_in_mins: Option<i64>,
        due_in_mins: Option<i64>,
    ) -> taskologic_core::task::Task {
        let now = Utc::now();
        let draft = TaskDraft {
            title: "Water plants".into(),
            start_at: start_in_mins.map(|m| now + TimeDelta::minutes(m)),
            due_at: due_in_mins.map(|m| now + TimeDelta::minutes(m)),
            ..Default::default()
        };
        match handle(
            st,
            &Session {
                uid: 1,
                is_admin: true,
            },
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

    fn slips(st: &Arc<AppState>) -> Vec<taskologic_core::print::PrintJobKind> {
        st.db
            .with(|c| repo::pending_print_jobs(c, 1, Utc::now()))
            .unwrap()
            .into_iter()
            .map(|(_, job)| job.kind)
            .collect()
    }

    #[test]
    fn a_task_with_no_dates_never_prints_a_reminder() {
        let (st, board) = reminder_setup(Some(30), Some(6));
        dated_task(&st, &board, None, None);
        tick_once(&st).unwrap();
        assert!(slips(&st).is_empty(), "nothing to count back from");
    }

    #[test]
    fn one_date_prints_one_reminder_and_only_once() {
        // Due in five minutes with a six minute lead: the moment has come.
        let (st, board) = reminder_setup(Some(30), Some(6));
        dated_task(&st, &board, None, Some(5));
        tick_once(&st).unwrap();
        assert_eq!(slips(&st), vec![PrintJobKind::Reminder]);
        // Ticking again must not spool a second copy.
        tick_once(&st).unwrap();
        assert_eq!(slips(&st).len(), 1);
    }

    #[test]
    fn a_reminder_whose_moment_has_not_come_waits() {
        // Due in an hour with a six minute lead.
        let (st, board) = reminder_setup(Some(30), Some(6));
        dated_task(&st, &board, None, Some(60));
        tick_once(&st).unwrap();
        assert!(slips(&st).is_empty());
    }

    #[test]
    fn a_reminder_for_something_long_overdue_is_not_worth_printing() {
        // Due two hours ago. Coming back from a weekend off must not spool
        // every reminder that was missed.
        let (st, board) = reminder_setup(Some(30), Some(6));
        dated_task(&st, &board, None, Some(-120));
        tick_once(&st).unwrap();
        assert!(slips(&st).is_empty());
    }

    #[test]
    fn both_dates_print_a_reminder_to_start_and_the_slip_to_finish_with() {
        // Starts in ten minutes (30 minute lead: due now), due in an hour
        // (6 minute lead: not yet).
        let (st, board) = reminder_setup(Some(30), Some(6));
        let task = dated_task(&st, &board, Some(10), Some(60));
        tick_once(&st).unwrap();
        assert_eq!(
            slips(&st),
            vec![PrintJobKind::Reminder],
            "the nudge to start, first"
        );

        // Wind the due date back so its own moment arrives.
        st.db
            .with(|c| {
                Ok(c.execute(
                    "UPDATE tasks SET due_at = ?2 WHERE id = ?1",
                    rusqlite::params![task.id.0, (Utc::now() + TimeDelta::minutes(5)).timestamp()],
                )?)
            })
            .unwrap();
        tick_once(&st).unwrap();
        assert_eq!(
            slips(&st),
            vec![PrintJobKind::Reminder, PrintJobKind::Task],
            "then the paper you finish with"
        );
    }

    #[test]
    fn nobody_is_told_to_start_what_they_already_started() {
        let (st, board) = reminder_setup(Some(30), Some(6));
        let task = dated_task(&st, &board, Some(10), None);
        // Into the started column and straight back out again, so where it
        // sits now says nothing about whether it was ever started.
        for to in [board.started_col, board.column_for(ColumnRole::Paused)] {
            handle(
                &st,
                &Session {
                    uid: 1,
                    is_admin: true,
                },
                Request::MoveTask {
                    task_id: task.id,
                    to_column: to,
                    position: None,
                    override_deps: false,
                    answer: None,
                },
            )
            .unwrap();
        }
        tick_once(&st).unwrap();
        assert!(slips(&st).is_empty(), "the event log remembers the start");
    }

    #[test]
    fn whoever_already_has_the_slip_is_not_handed_a_second_one() {
        let (st, board) = reminder_setup(Some(30), Some(6));
        let task = dated_task(&st, &board, Some(-600), Some(5));
        // Printing it by hand counts as having it.
        handle(
            &st,
            &Session {
                uid: 1,
                is_admin: true,
            },
            Request::PrintTask { task_id: task.id },
        )
        .unwrap();
        assert_eq!(slips(&st), vec![PrintJobKind::Task]);
        tick_once(&st).unwrap();
        assert_eq!(
            slips(&st),
            vec![PrintJobKind::Task],
            "the due reminder saw the manual print and stood down"
        );
    }

    /// A task carrying its own print rules, dated the same way `dated_task`
    /// dates one.
    fn ruled_task(
        st: &Arc<AppState>,
        board: &Board,
        rules: Vec<PrintRule>,
        start_in_mins: Option<i64>,
        due_in_mins: Option<i64>,
    ) -> taskologic_core::task::Task {
        let now = Utc::now();
        let draft = TaskDraft {
            title: "Water plants".into(),
            start_at: start_in_mins.map(|m| now + TimeDelta::minutes(m)),
            due_at: due_in_mins.map(|m| now + TimeDelta::minutes(m)),
            print_rules: rules,
            ..Default::default()
        };
        match handle(
            st,
            &Session {
                uid: 1,
                is_admin: true,
            },
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

    fn set_date(st: &Arc<AppState>, task: taskologic_core::ids::TaskId, column: &str, at: chrono::DateTime<Utc>) {
        st.db
            .with(|c| {
                Ok(c.execute(
                    &format!("UPDATE tasks SET {column} = ?2 WHERE id = ?1"),
                    rusqlite::params![task.0, at.timestamp()],
                )?)
            })
            .unwrap();
    }

    #[test]
    fn a_before_due_rule_prints_at_its_moment_and_follows_a_moved_due_date() {
        // No reminder lead times at all: whatever prints, the rule did it.
        let (st, board) = reminder_setup(None, None);
        let rule = PrintRule {
            when: PrintWhen::BeforeDue(Offset {
                amount: 10,
                unit: OffsetUnit::Minutes,
            }),
            slip: SlipKind::Task,
            to: Recipients::Creator,
        };
        let task = ruled_task(&st, &board, vec![rule], None, Some(60));
        tick_once(&st).unwrap();
        assert!(slips(&st).is_empty(), "due in an hour, ten minutes before is not yet");

        set_date(&st, task.id, "due_at", Utc::now() + TimeDelta::minutes(5));
        tick_once(&st).unwrap();
        assert_eq!(slips(&st), vec![PrintJobKind::Task], "the moment has come");
        tick_once(&st).unwrap();
        assert_eq!(slips(&st).len(), 1, "and it prints once");

        // A moved due date is a new moment, with a slip of its own.
        set_date(&st, task.id, "due_at", Utc::now() + TimeDelta::minutes(4));
        tick_once(&st).unwrap();
        assert_eq!(slips(&st).len(), 2);

        // Long past its due date, a slip nobody printed is not worth spooling.
        let late = ruled_task(
            &st,
            &board,
            vec![PrintRule {
                when: PrintWhen::BeforeDue(Offset {
                    amount: 10,
                    unit: OffsetUnit::Minutes,
                }),
                slip: SlipKind::Task,
                to: Recipients::Creator,
            }],
            None,
            Some(-120),
        );
        tick_once(&st).unwrap();
        assert_eq!(slips(&st).len(), 2, "nothing for {}", late.title);
    }

    #[test]
    fn an_on_start_rule_fires_by_date_or_by_move_but_never_both() {
        // The prefs would print reminders here; the rules take their place.
        let (st, board) = reminder_setup(Some(30), Some(6));
        let rule = || PrintRule {
            when: PrintWhen::OnStart,
            slip: SlipKind::Task,
            to: Recipients::Creator,
        };
        // Started a minute ago by the calendar and never touched: the date prints it.
        let by_date = ruled_task(&st, &board, vec![rule()], Some(-1), None);
        tick_once(&st).unwrap();
        assert_eq!(
            slips(&st),
            vec![PrintJobKind::Task],
            "one task slip, and no reminder from the prefs"
        );
        tick_once(&st).unwrap();
        assert_eq!(slips(&st).len(), 1);
        handle(
            &st,
            &Session {
                uid: 1,
                is_admin: true,
            },
            Request::MoveTask {
                task_id: by_date.id,
                to_column: board.started_col,
                position: None,
                override_deps: false,
                answer: None,
            },
        )
        .unwrap();
        assert_eq!(slips(&st).len(), 1, "starting it by hand afterwards adds nothing");

        // The other way round: started by hand before its date, then the
        // date comes and finds the slip already handed over.
        let by_hand = ruled_task(&st, &board, vec![rule()], Some(10), None);
        handle(
            &st,
            &Session {
                uid: 1,
                is_admin: true,
            },
            Request::MoveTask {
                task_id: by_hand.id,
                to_column: board.started_col,
                position: None,
                override_deps: false,
                answer: None,
            },
        )
        .unwrap();
        assert_eq!(slips(&st).len(), 2);
        set_date(&st, by_hand.id, "start_at", Utc::now() - TimeDelta::minutes(1));
        tick_once(&st).unwrap();
        assert_eq!(slips(&st).len(), 2);
    }

    #[test]
    fn a_task_with_the_flag_starts_itself_once_its_date_comes_and_only_once() {
        let (st, board) = reminder_setup(None, None);
        let now = Utc::now();
        let draft = TaskDraft {
            title: "Kettle on".into(),
            start_at: Some(now + TimeDelta::minutes(10)),
            auto_start: true,
            ..Default::default()
        };
        let task = match handle(
            &st,
            &Session {
                uid: 1,
                is_admin: true,
            },
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
        let column = || st.db.with(|c| repo::require_task(c, task.id)).unwrap().column_id;
        tick_once(&st).unwrap();
        assert_ne!(column(), board.started_col, "not yet");
        set_date(&st, task.id, "start_at", now - TimeDelta::minutes(1));
        tick_once(&st).unwrap();
        assert_eq!(column(), board.started_col, "the date came");
        // Put back to todo by hand: the scheduler does not start it again.
        handle(
            &st,
            &Session {
                uid: 1,
                is_admin: true,
            },
            Request::MoveTask {
                task_id: task.id,
                to_column: board.columns[0].id,
                position: None,
                override_deps: false,
                answer: None,
            },
        )
        .unwrap();
        tick_once(&st).unwrap();
        assert_ne!(column(), board.started_col, "somebody decided otherwise");
        // The move is on record, as the scheduler's.
        let n = st
            .db
            .with(|c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM events WHERE task_id = ?1 AND kind = 'task_moved' AND actor_uid IS NULL",
                    rusqlite::params![task.id.0],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .unwrap();
        assert_eq!(n, 1);
    }

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
                answer: None,
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
