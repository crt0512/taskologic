//! Slips a task asked for itself: its print rules, fired at the moments they
//! name and remembered so nobody is handed the same paper twice.
//!
//! The rules on a task stand in for every user's own printing preferences on
//! that task. A task with rules never goes through the autoprint mode or the
//! reminder lead times; it only ever comes through here. Two of the moments
//! are events, creation and entering the started column, and the handlers
//! fire those. The dated ones, a start date arriving and a lead time before
//! the due date, belong to the scheduler.

use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use rusqlite::Connection;
use taskologic_core::board::Board;
use taskologic_core::ids::Uid;
use taskologic_core::print::{
    AutoprintTrigger, PrintJob, PrintRule, PrintWhen, SlipKind, build_reminder_job,
    build_task_job, plan_rule_prints, recipients_of,
};
use taskologic_core::task::Task;
use taskologic_core::user::User;

use crate::db::repo;
use crate::error::AppError;
use crate::state::AppState;

/// The job a slip kind stands for, with everything the task has on it. What
/// reaches the paper is the printer's layout for that kind to decide.
pub fn build(
    c: &Connection,
    kind: SlipKind,
    task: &Task,
    board: &Board,
    user: &User,
    now: DateTime<Utc>,
) -> Result<PrintJob, AppError> {
    Ok(match kind {
        SlipKind::Task => {
            let names = repo::username_map(c)?;
            let deps = repo::dep_lines(c, task)?;
            build_task_job(
                task,
                board,
                &deps,
                &|u| {
                    names
                        .get(&u)
                        .cloned()
                        .unwrap_or_else(|| format!("uid {u}"))
                },
                user,
                now,
            )
        }
        SlipKind::Reminder => build_reminder_job(task, board, user, now),
        // A sheet is printed for a whole group when a program makes it, in
        // `programs`; a rule asking for one on a single task is not worth a
        // blank page, so it gets the task slip.
        SlipKind::Sheet => {
            let names = repo::username_map(c)?;
            let deps = repo::dep_lines(c, task)?;
            build_task_job(
                task,
                board,
                &deps,
                &|u| {
                    names
                        .get(&u)
                        .cloned()
                        .unwrap_or_else(|| format!("uid {u}"))
                },
                user,
                now,
            )
        }
    })
}

/// Fire the rules an event has just earned: "when created" as the task is
/// made, "on start" as it enters the started column. Failures are logged and
/// never surfaced to whoever made or moved the task, the same as autoprint.
pub fn fire(state: &Arc<AppState>, board: &Board, task: &Task, trigger: AutoprintTrigger) {
    let when = match trigger {
        AutoprintTrigger::AddedToBoard => PrintWhen::OnCreate,
        AutoprintTrigger::MovedToStarted => PrintWhen::OnStart,
    };
    let now = Utc::now();
    let result = state.db.with(|c| {
        let mut jobs = Vec::new();
        for rule in task
            .print_rules
            .iter()
            .filter(|r| r.when == when && r.slip != SlipKind::Sheet)
        {
            // Keyed on the creation time: an "on start" slip printed here
            // and one the start date would print are the same slip.
            jobs.extend(queue_rule(c, rule, task, board, task.created_at, now)?);
        }
        Ok(jobs)
    });
    match result {
        Ok(jobs) => {
            for (uid, job) in jobs {
                if let Err(e) = state.enqueue_print(uid, &job) {
                    tracing::warn!(uid, error = %e, "could not queue a task's own print");
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "print rule lookup failed"),
    }
}

/// Everybody a rule owes a slip who has not had one under this key yet.
/// Remembers each one as sent and hands back the jobs to queue.
fn queue_rule(
    c: &Connection,
    rule: &PrintRule,
    task: &Task,
    board: &Board,
    anchor: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Vec<(Uid, PrintJob)>, AppError> {
    let kind = rule.sent_kind();
    let mut jobs = Vec::new();
    for uid in recipients_of(rule, task, board) {
        if repo::slip_sent(c, task.id, uid, &kind, anchor)? {
            continue;
        }
        // Named on a public board without ever having logged in: there is
        // no user to print for, and nothing to remember either.
        let Some(user) = repo::get_user(c, uid)? else {
            continue;
        };
        repo::mark_slip_sent(c, task.id, uid, &kind, anchor, now)?;
        jobs.push((uid, build(c, rule.slip, task, board, &user, now)?));
    }
    Ok(jobs)
}

/// The scheduler's half: rules whose moment is a date. A "before due" slip
/// is printed from its moment until the due date is one tick past, the same
/// grace reminders get, so a daemon that was off does not come back and
/// spool everything it missed. A start slip has no such cutoff: a task whose
/// start date went by while nobody started it is exactly one still waiting
/// for its paper.
pub fn print_dated(state: &Arc<AppState>) -> Result<(), AppError> {
    let now = Utc::now();
    let grace = TimeDelta::seconds(state.cfg.scheduler_tick_secs.max(1) as i64);
    for task in state.db.with(repo::dated_rule_tasks)? {
        let board = state.db.with(|c| repo::require_board(c, task.board_id))?;
        let jobs = state.db.with(|c| {
            let mut jobs = Vec::new();
            for planned in plan_rule_prints(&task) {
                if planned.at > now || planned.anchor.is_some_and(|a| now > a + grace) {
                    continue;
                }
                let rule = &task.print_rules[planned.index];
                jobs.extend(queue_rule(c, rule, &task, &board, planned.sent_anchor, now)?);
            }
            Ok(jobs)
        })?;
        for (uid, job) in jobs {
            state.enqueue_print(uid, &job)?;
        }
    }
    Ok(())
}
