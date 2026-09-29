//! What the program engine decides, done to the database.
//!
//! `taskologic_core::program::react` says what an event on a run means:
//! which tasks to make, which to date, whether the root is done. Everything
//! here applies that inside the transaction the event arrived in, so a move
//! and what it set off are one change or none. Nothing in here decides
//! anything the engine did not; it only knows where the rows are.

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use taskologic_core::board::Board;
use taskologic_core::event::{EventKind, FieldChange};
use taskologic_core::ids::{ColumnId, Uid};
use taskologic_core::print::{PrintJob, PrintWhen, SlipKind, build_sheet_job, recipients_of};
use taskologic_core::program::{
    Program, ROOT_KEY, Run, RunEvent, StepTask, draft_for, react,
};
use taskologic_core::task::Task;
use taskologic_core::transition::{MovePlan, plan_move};
use taskologic_core::user::User;

use crate::db::repo;
use crate::error::AppError;

type R<T> = Result<T, AppError>;

/// The tasks of `root`'s run that are in the started column: what the
/// sheet's stop code finishes.
pub fn running_children(c: &Connection, board: &Board, root: &Task) -> R<Vec<Task>> {
    let Some(link) = repo::task_link(c, root.id)? else {
        return Ok(Vec::new());
    };
    if link.step != ROOT_KEY {
        return Ok(Vec::new());
    }
    Ok(repo::run_tasks(c, link.run)?
        .into_iter()
        .map(|(_, t)| t)
        .filter(|t| t.id != root.id && !t.is_archived() && t.column_id == board.started_col)
        .collect())
}

/// What a caller has to tell the clients about once the transaction holds.
#[derive(Debug, Default)]
pub struct Effects {
    /// Tasks the run made. They want an autoprint pass too.
    pub created: Vec<Task>,
    /// Tasks the run dated, or the root with one more dependency.
    pub changed: Vec<Task>,
    /// The root, moved to the finished column by the run itself.
    pub finished_root: Option<Task>,
    /// Group sheets to queue, one per person, for fan-out steps that ask
    /// for one. Queued by the caller once the transaction holds.
    pub prints: Vec<(Uid, PrintJob)>,
}

impl Effects {
    fn extend(&mut self, other: Effects) {
        self.created.extend(other.created);
        self.changed.extend(other.changed);
        if other.finished_root.is_some() {
            self.finished_root = other.finished_root;
        }
        self.prints.extend(other.prints);
    }
}

/// Start a run of `program`: the root task, dated `start_at` and assigned
/// as its step says, then everything that comes with the root. `fan_out`
/// narrows the entries of fan-out steps for this run alone. Returns the
/// root as it stands once its dependencies are in.
#[allow(clippy::too_many_arguments)]
pub fn start(
    c: &Connection,
    board: &Board,
    program: &Program,
    starter: &User,
    column: ColumnId,
    start_at: Option<DateTime<Utc>>,
    fan_out: &[(String, Vec<String>)],
    now: DateTime<Utc>,
) -> R<(Task, Effects)> {
    // The run's own copy of the steps, with this run's fan-out lists.
    let mut program = program.clone();
    for (key, entries) in fan_out {
        let step = program
            .steps
            .iter_mut()
            .find(|s| &s.key == key && s.fan_out.is_some())
            .ok_or_else(|| AppError::bad(format!("step {key} does not fan out")))?;
        let entries: Vec<String> = entries
            .iter()
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty())
            .collect();
        if entries.is_empty() {
            return Err(AppError::bad(format!(
                "step {key} needs at least one entry to fan out over"
            )));
        }
        step.fan_out = Some(entries);
    }
    let root_step = program
        .steps
        .iter()
        .find(|s| s.is_root())
        .ok_or_else(|| AppError::bad("the program has no root step"))?;
    let draft = draft_for(root_step, starter.uid, board, start_at, Vec::new(), None);
    let root = repo::create_task(c, board, column, &draft, starter.uid, None, now)?;
    let run = repo::create_run(c, &program, root.id, starter.uid, column, now)?;
    repo::link_task(c, run.id, root.id, ROOT_KEY, 1, None, false, root_step.time_limit, None)?;
    repo::record_event(
        c,
        board.id,
        Some(root.id),
        Some(starter.uid),
        &EventKind::RunStarted { run: run.id },
        now,
    )?;
    let effects = apply(c, board, &run, &RunEvent::Created, starter.timezone, now)?;
    Ok((repo::require_task(c, root.id)?, effects))
}

/// One of a run's tasks moved. `task` is the task after the move, `answer`
/// what whoever finished it said to its question, if it has one and they
/// did. Runs inside the move's transaction.
pub fn after_move(
    c: &Connection,
    board: &Board,
    task: &Task,
    plan: &MovePlan,
    answer: Option<String>,
    now: DateTime<Utc>,
) -> R<Effects> {
    let Some(link) = repo::task_link(c, task.id)? else {
        return Ok(Effects::default());
    };
    let run = repo::require_run(c, link.run)?;
    if !run.is_active() || plan.from == plan.to {
        return Ok(Effects::default());
    }
    let mut effects = Effects::default();
    let (entering, leaving) = (plan.to, plan.from);

    // The time limit, by its four rules: entering the started column sets
    // the due date from the limit, unless it is there already; coming back
    // from the paused column adds the time spent there; any column that is
    // not work clears it, so the next start sets it fresh; and the due date
    // itself is the only truth, an edit by hand moves the limit.
    if let Some(limit) = link.time_limit {
        let due = if entering == board.started_col {
            match (leaving == board.paused_col, link.paused_at, task.due_at) {
                (true, Some(paused), Some(due)) => Some(Some(due + (now - paused))),
                (_, _, None) => Some(limit.after(now)),
                _ => None,
            }
        } else if entering != board.paused_col
            && entering != board.finished_col
            && task.due_at.is_some()
        {
            Some(None)
        } else {
            None
        };
        if let Some(due) = due {
            let changed = repo::set_due_at(c, task, due)?;
            repo::record_event(
                c,
                board.id,
                Some(task.id),
                None,
                &EventKind::TaskEdited {
                    changed: vec![FieldChange::DueAt {
                        from: task.due_at,
                        to: due,
                    }],
                },
                now,
            )?;
            effects.changed.push(changed);
        }
    }
    repo::set_paused_at(c, task.id, (entering == board.paused_col).then_some(now))?;

    let event = if entering == board.started_col && link.step == ROOT_KEY && run.started_at.is_none()
    {
        repo::start_run(c, run.id, now)?;
        Some(RunEvent::RootStarted)
    } else if entering == board.finished_col {
        Some(RunEvent::Finished {
            task: task.id,
            answer,
        })
    } else {
        None
    };
    if let Some(event) = event {
        let run = repo::require_run(c, run.id)?;
        let tz = starter_tz(c, run.started_by)?;
        effects.extend(apply(c, board, &run, &event, tz, now)?);
    }
    Ok(effects)
}

/// One of a run's tasks was deleted: the step is skipped, and the root may
/// have nothing left to wait for.
pub fn after_delete(c: &Connection, board: &Board, task: &Task, now: DateTime<Utc>) -> R<Effects> {
    let Some(link) = repo::task_link(c, task.id)? else {
        return Ok(Effects::default());
    };
    let run = repo::require_run(c, link.run)?;
    if !run.is_active() {
        return Ok(Effects::default());
    }
    repo::set_skipped(c, task.id, true)?;
    let tz = starter_tz(c, run.started_by)?;
    apply(c, board, &run, &RunEvent::Deleted { task: task.id }, tz, now)
}

/// A finished task's question is answered afterwards. Nothing to answer, or
/// not an answer the question takes, is refused with a reason.
pub fn answer(
    c: &Connection,
    board: &Board,
    task: &Task,
    answer: &str,
    now: DateTime<Utc>,
) -> R<Effects> {
    let link = repo::task_link(c, task.id)?
        .filter(|l| l.question.is_some())
        .ok_or_else(|| AppError::bad("this task has no question to answer"))?;
    let run = repo::require_run(c, link.run)?;
    if !run.is_active() {
        return Err(AppError::bad("the program this task belongs to is over"));
    }
    if !link.pending_answer() {
        return Err(AppError::bad("this task's question has been answered already"));
    }
    let question = link.question.as_ref().expect("filtered above");
    let canonical = question
        .canonical(answer)
        .ok_or_else(|| AppError::bad(format!("{answer:?} is not one of the answers")))?;
    let tz = starter_tz(c, run.started_by)?;
    apply(
        c,
        board,
        &run,
        &RunEvent::Answered {
            task: task.id,
            answer: canonical,
        },
        tz,
        now,
    )
}

/// A task came back from the archive: it is a step again.
pub fn after_restore(c: &Connection, task: &Task) -> R<()> {
    if repo::task_link(c, task.id)?.is_some() {
        repo::set_skipped(c, task.id, false)?;
    }
    Ok(())
}

/// Stop a run. With `delete_open`, the tasks it made that are still open go
/// to the archive as deleted, the root among them; finished ones stay.
/// Returns what was deleted.
pub fn cancel(
    c: &Connection,
    board: &Board,
    run: &Run,
    delete_open: bool,
    actor: Uid,
    now: DateTime<Utc>,
) -> R<Vec<Task>> {
    repo::cancel_run(c, run.id, now)?;
    repo::record_event(
        c,
        board.id,
        Some(run.root_task),
        Some(actor),
        &EventKind::RunCancelled { run: run.id },
        now,
    )?;
    let mut deleted = Vec::new();
    if delete_open {
        for (_, task) in repo::run_tasks(c, run.id)? {
            if task.is_done(board) || task.is_archived() {
                continue;
            }
            let t = repo::soft_delete_task(c, &task, now)?;
            repo::record_event(
                c,
                board.id,
                Some(t.id),
                Some(actor),
                &EventKind::TaskDeleted,
                now,
            )?;
            deleted.push(t);
        }
    }
    Ok(deleted)
}

fn starter_tz(c: &Connection, uid: Uid) -> R<chrono_tz::Tz> {
    Ok(repo::get_user(c, uid)?
        .map(|u| u.timezone)
        .unwrap_or(chrono_tz::UTC))
}

/// Ask the engine what `event` means for `run` and do it.
fn apply(
    c: &Connection,
    board: &Board,
    run: &Run,
    event: &RunEvent,
    tz: chrono_tz::Tz,
    now: DateTime<Utc>,
) -> R<Effects> {
    let rows = repo::run_tasks(c, run.id)?;
    let tasks: Vec<StepTask> = rows
        .iter()
        .map(|(link, task)| StepTask {
            step: link.step.clone(),
            iteration: link.iteration,
            param: link.param.clone(),
            task_id: task.id,
            done: task.is_done(board),
            skipped: link.skipped,
            counts: link.counts,
            pending_answer: link.pending_answer(),
        })
        .collect();
    let plan = react(&run.steps, &tasks, run.started_at.is_some(), event, now, tz);

    let mut effects = Effects::default();
    let mut root_changed = false;
    // Which steps made a group this time, for the sheets below.
    let mut groups: Vec<usize> = Vec::new();
    for creation in &plan.create {
        let step = &run.steps[creation.step];
        let draft = draft_for(
            step,
            run.started_by,
            board,
            creation.start_at,
            creation.depends_on.clone(),
            creation.param.as_deref(),
        );
        let task = repo::create_task(c, board, run.column_id, &draft, run.started_by, None, now)?;
        repo::link_task(
            c,
            run.id,
            task.id,
            &step.key,
            creation.iteration,
            creation.param.as_deref(),
            creation.counts,
            step.time_limit,
            step.question.as_ref(),
        )?;
        if creation.counts {
            repo::add_dep(c, run.root_task, task.id)?;
            root_changed = true;
        }
        if step.fan_out.is_some() && !groups.contains(&creation.step) {
            groups.push(creation.step);
        }
        // Reloaded so the task carries its program link.
        effects.created.push(repo::require_task(c, task.id)?);
    }
    // One sheet per fan-out group per person who gets one, never one per
    // task: the sheet is the paper for the whole group.
    for index in groups {
        let step = &run.steps[index];
        let group: Vec<Task> = effects
            .created
            .iter()
            .filter(|t| t.program.as_ref().is_some_and(|p| p.step == step.key))
            .cloned()
            .collect();
        let Some(first) = group.first() else { continue };
        let root = repo::require_task(c, run.root_task)?;
        for rule in step
            .print
            .iter()
            .filter(|r| r.slip == SlipKind::Sheet && r.when == PrintWhen::OnCreate)
        {
            let kind = rule.sent_kind();
            for uid in recipients_of(rule, first, board) {
                if repo::slip_sent(c, root.id, uid, &kind, first.created_at)? {
                    continue;
                }
                let Some(user) = repo::get_user(c, uid)? else {
                    continue;
                };
                repo::mark_slip_sent(c, root.id, uid, &kind, first.created_at, now)?;
                effects
                    .prints
                    .push((uid, build_sheet_job(&root, &group, board, &user, now)));
            }
        }
    }
    for id in &plan.set_start {
        let task = repo::require_task(c, *id)?;
        if task.start_at.is_none() {
            let dated = repo::set_start_at(c, &task, now)?;
            repo::record_event(
                c,
                board.id,
                Some(task.id),
                None,
                &EventKind::TaskEdited {
                    changed: vec![FieldChange::StartAt {
                        from: None,
                        to: Some(now),
                    }],
                },
                now,
            )?;
            effects.changed.push(dated);
        }
    }
    if let Some(task) = plan.ask {
        repo::set_asked(c, task)?;
        effects.changed.push(repo::require_task(c, task)?);
    }
    if let Some((task, answer)) = &plan.answered {
        repo::set_answer(c, *task, answer)?;
        repo::record_event(
            c,
            board.id,
            Some(*task),
            None,
            &EventKind::QuestionAnswered {
                answer: answer.clone(),
            },
            now,
        )?;
        effects.changed.push(repo::require_task(c, *task)?);
    }
    if root_changed {
        effects.changed.push(repo::require_task(c, run.root_task)?);
    }
    if plan.finish_root {
        let root = repo::require_task(c, run.root_task)?;
        if !root.is_archived() && root.column_id != board.finished_col {
            // Everything the root waits for is done, which is what the engine
            // just established, so there is nothing to override.
            let mp = plan_move(board, &root, board.finished_col, &[], false, now)?;
            let moved = repo::apply_move(c, &root, &mp, None, now)?;
            repo::finish_run(c, run.id, now)?;
            effects.finished_root = Some(moved);
        }
    }
    if plan.end_run {
        repo::finish_run(c, run.id, now)?;
    }
    Ok(effects)
}
