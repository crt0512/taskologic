//! Every query in one place. Functions take a `Connection`, which is also
//! what a `Transaction` derefs to, so they compose inside `Db::tx`.

use std::collections::HashMap;
use std::str::FromStr;

use chrono::{DateTime, TimeDelta, Utc};
use chrono_tz::Tz;
use rusqlite::{Connection, OptionalExtension, Row, params};
use taskologic_core::board::{Board, Column, ColumnRemoval, ColumnRole};
use taskologic_core::event::{Event, EventKind};
use taskologic_core::ids::{
    BoardId, ColumnId, EventId, PrintJobId, ShortId, TaskId, TemplateId, Uid,
};
use taskologic_core::prefs::UserPrefs;
use taskologic_core::print::{DepLine, PrintJob, ReminderKind};
use taskologic_core::repeat::RepeatSpec;
use taskologic_core::stats;
use taskologic_core::task::{Task, TaskDraft};
use taskologic_core::template::{Template, TemplateOptions};
use taskologic_core::transition::MovePlan;
use taskologic_core::user::{User, UserSummary};
use taskologic_proto::{BoardSummary, CreateBoard, SearchHit, UpdateBoard};

use super::{dt, ts};
use crate::error::AppError;

type R<T> = Result<T, AppError>;

/// Boards `uid` may see: public ones, own ones, and private ones they belong to.
const VISIBLE_BOARDS: &str = "SELECT id FROM boards WHERE is_private = 0 OR owner_uid = ?1 \
     OR id IN (SELECT board_id FROM board_members WHERE uid = ?1)";

fn user_from_row(r: &Row) -> rusqlite::Result<User> {
    let tz: String = r.get("timezone")?;
    let prefs: String = r.get("prefs_json")?;
    let pin: Option<String> = r.get("pin_hash")?;
    Ok(User {
        uid: r.get::<_, i64>("uid")? as Uid,
        username: r.get("username")?,
        is_admin: r.get::<_, i64>("is_admin")? != 0,
        timezone: Tz::from_str(&tz).unwrap_or(chrono_tz::UTC),
        prefs: serde_json::from_str(&prefs).unwrap_or_default(),
        has_pin: pin.is_some(),
        created_at: dt(r.get("created_at")?),
    })
}

pub fn upsert_user_on_login(
    c: &Connection,
    uid: Uid,
    username: &str,
    host_tz: Tz,
    force_admin: bool,
    now: DateTime<Utc>,
) -> R<User> {
    let existing = get_user(c, uid)?;
    match existing {
        Some(u) => {
            let admin = u.is_admin || force_admin;
            c.execute(
                "UPDATE users SET username = ?2, is_admin = ?3, last_login_at = ?4 WHERE uid = ?1",
                params![i64::from(uid), username, admin as i64, ts(now)],
            )?;
        }
        None => {
            let first: i64 = c.query_row("SELECT count(*) FROM users", [], |r| r.get(0))?;
            let admin = force_admin || first == 0;
            c.execute(
                "INSERT INTO users (uid, username, is_admin, timezone, prefs_json, created_at, last_login_at) \
                 VALUES (?1, ?2, ?3, ?4, '{}', ?5, ?5)",
                params![i64::from(uid), username, admin as i64, host_tz.name(), ts(now)],
            )?;
        }
    }
    require_user(c, uid)
}

pub fn get_user(c: &Connection, uid: Uid) -> R<Option<User>> {
    Ok(c.query_row(
        "SELECT * FROM users WHERE uid = ?1",
        params![i64::from(uid)],
        user_from_row,
    )
    .optional()?)
}

pub fn require_user(c: &Connection, uid: Uid) -> R<User> {
    get_user(c, uid)?.ok_or_else(|| AppError::NotFound(format!("no user with uid {uid}")))
}

pub fn list_users(c: &Connection) -> R<Vec<User>> {
    let mut st = c.prepare("SELECT * FROM users ORDER BY username")?;
    let rows = st.query_map([], user_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn update_prefs(c: &Connection, uid: Uid, prefs: &UserPrefs) -> R<()> {
    c.execute(
        "UPDATE users SET prefs_json = ?2 WHERE uid = ?1",
        params![i64::from(uid), serde_json::to_string(prefs)?],
    )?;
    Ok(())
}

pub fn update_timezone(c: &Connection, uid: Uid, tz: Tz) -> R<()> {
    c.execute(
        "UPDATE users SET timezone = ?2 WHERE uid = ?1",
        params![i64::from(uid), tz.name()],
    )?;
    Ok(())
}

pub fn set_admin(c: &Connection, uid: Uid, is_admin: bool) -> R<()> {
    c.execute(
        "UPDATE users SET is_admin = ?2 WHERE uid = ?1",
        params![i64::from(uid), is_admin as i64],
    )?;
    Ok(())
}

pub fn username_map(c: &Connection) -> R<HashMap<Uid, String>> {
    let mut st = c.prepare("SELECT uid, username FROM users")?;
    let rows = st.query_map([], |r| {
        Ok((r.get::<_, i64>(0)? as Uid, r.get::<_, String>(1)?))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn columns_of(c: &Connection, board_id: BoardId) -> R<Vec<Column>> {
    let mut st = c.prepare(
        "SELECT id, board_id, name, position, sort_by_due FROM columns WHERE board_id = ?1 ORDER BY position, id",
    )?;
    let rows = st.query_map(params![board_id.0], |r| {
        Ok(Column {
            id: ColumnId(r.get(0)?),
            board_id: BoardId(r.get(1)?),
            name: r.get(2)?,
            position: r.get(3)?,
            sort_by_due: r.get::<_, i64>(4)? != 0,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn members_of(c: &Connection, board_id: BoardId) -> R<Vec<Uid>> {
    let mut st =
        c.prepare("SELECT uid FROM board_members WHERE board_id = ?1 ORDER BY added_at, uid")?;
    let rows = st.query_map(params![board_id.0], |r| Ok(r.get::<_, i64>(0)? as Uid))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn get_board(c: &Connection, id: BoardId) -> R<Option<Board>> {
    let row = c
        .query_row(
            "SELECT id, name, owner_uid, is_locked, is_private, archive_after_secs, started_col, paused_col, \
             finished_col, created_at, purge_deleted_after_secs, card_fields_json, description FROM boards WHERE id = ?1",
            params![id.0],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                    r.get::<_, Option<i64>>(7)?,
                    r.get::<_, Option<i64>>(8)?,
                    r.get::<_, i64>(9)?,
                    r.get::<_, i64>(10)?,
                    r.get::<_, String>(11)?,
                    r.get::<_, String>(12)?,
                ))
            },
        )
        .optional()?;
    let Some((
        _,
        name,
        owner,
        locked,
        private,
        archive,
        started,
        paused,
        finished,
        created,
        purge,
        cards,
        description,
    )) = row
    else {
        return Ok(None);
    };
    let (Some(started), Some(paused), Some(finished)) = (started, paused, finished) else {
        return Err(anyhow::anyhow!("board {id} has no designated columns").into());
    };
    Ok(Some(Board {
        id,
        name,
        description,
        owner_uid: owner as Uid,
        is_locked: locked != 0,
        is_private: private != 0,
        archive_after_secs: archive,
        purge_deleted_after_secs: purge,
        card_fields: serde_json::from_str(&cards).unwrap_or_default(),
        started_col: ColumnId(started),
        paused_col: ColumnId(paused),
        finished_col: ColumnId(finished),
        created_at: dt(created),
        columns: columns_of(c, id)?,
        members: members_of(c, id)?,
    }))
}

pub fn require_board(c: &Connection, id: BoardId) -> R<Board> {
    get_board(c, id)?.ok_or_else(|| AppError::NotFound("board not found".into()))
}

pub fn create_board(c: &Connection, owner: Uid, req: &CreateBoard, now: DateTime<Utc>) -> R<Board> {
    taskologic_core::board::validate_name(&req.name)?;
    taskologic_core::board::validate_column_names(&req.columns)?;
    if req.archive_after_secs < 0 {
        return Err(taskologic_core::board::BoardError::NegativeArchiveDelay.into());
    }
    if req.purge_deleted_after_secs < 0 {
        return Err(taskologic_core::board::BoardError::NegativePurgeDelay.into());
    }
    for (role, idx) in [
        (ColumnRole::Started, req.started_col),
        (ColumnRole::Paused, req.paused_col),
        (ColumnRole::Finished, req.finished_col),
    ] {
        if idx >= req.columns.len() {
            return Err(AppError::bad(format!(
                "{} column index {idx} is out of range",
                role.label()
            )));
        }
    }
    c.execute(
        "INSERT INTO boards (name, owner_uid, is_locked, is_private, archive_after_secs, created_at, purge_deleted_after_secs, card_fields_json, description) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            req.name.trim(),
            i64::from(owner),
            req.is_locked as i64,
            req.is_private as i64,
            req.archive_after_secs,
            ts(now),
            req.purge_deleted_after_secs,
            serde_json::to_string(&req.card_fields)?,
            req.description.trim()
        ],
    )?;
    let board_id = BoardId(c.last_insert_rowid());
    let mut ids = Vec::new();
    for (i, name) in req.columns.iter().enumerate() {
        c.execute(
            "INSERT INTO columns (board_id, name, position) VALUES (?1, ?2, ?3)",
            params![board_id.0, name.trim(), (i as i64) * 10],
        )?;
        ids.push(c.last_insert_rowid());
    }
    c.execute(
        "UPDATE boards SET started_col = ?2, paused_col = ?3, finished_col = ?4 WHERE id = ?1",
        params![
            board_id.0,
            ids[req.started_col],
            ids[req.paused_col],
            ids[req.finished_col]
        ],
    )?;
    let mut members = req.members.clone();
    if !members.contains(&owner) {
        members.insert(0, owner);
    }
    for uid in members {
        add_member(c, board_id, uid, now)?;
    }
    record_event(
        c,
        board_id,
        None,
        Some(owner),
        &EventKind::BoardCreated,
        now,
    )?;
    require_board(c, board_id)
}

pub fn list_boards_visible(c: &Connection, uid: Uid, is_admin: bool) -> R<Vec<BoardSummary>> {
    // Admins get every board listed, with is_member saying whether they may
    // open it. Everyone else only ever hears about boards they can see.
    let sql = format!(
        "SELECT b.id, b.name, b.owner_uid, b.is_locked, b.is_private, b.description, \
         (SELECT count(*) FROM tasks t WHERE t.board_id = b.id AND t.archived_at IS NULL), \
         b.id IN ({VISIBLE_BOARDS}) \
         FROM boards b WHERE b.id IN ({VISIBLE_BOARDS}) OR ?2 = 1 ORDER BY b.name, b.id"
    );
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map(params![i64::from(uid), is_admin as i64], |r| {
        Ok(BoardSummary {
            id: BoardId(r.get(0)?),
            name: r.get(1)?,
            owner_uid: r.get::<_, i64>(2)? as Uid,
            is_locked: r.get::<_, i64>(3)? != 0,
            is_private: r.get::<_, i64>(4)? != 0,
            description: r.get(5)?,
            task_count: r.get::<_, i64>(6)? as u32,
            is_member: r.get::<_, i64>(7)? != 0,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn update_board(c: &Connection, board: &Board, req: &UpdateBoard) -> R<Board> {
    if let Some(name) = &req.name {
        taskologic_core::board::validate_name(name)?;
        c.execute(
            "UPDATE boards SET name = ?2 WHERE id = ?1",
            params![board.id.0, name.trim()],
        )?;
    }
    if let Some(v) = req.is_locked {
        c.execute(
            "UPDATE boards SET is_locked = ?2 WHERE id = ?1",
            params![board.id.0, v as i64],
        )?;
    }
    if let Some(v) = req.is_private {
        c.execute(
            "UPDATE boards SET is_private = ?2 WHERE id = ?1",
            params![board.id.0, v as i64],
        )?;
    }
    if let Some(v) = req.archive_after_secs {
        if v < 0 {
            return Err(taskologic_core::board::BoardError::NegativeArchiveDelay.into());
        }
        c.execute(
            "UPDATE boards SET archive_after_secs = ?2 WHERE id = ?1",
            params![board.id.0, v],
        )?;
    }
    if let Some(v) = req.purge_deleted_after_secs {
        if v < 0 {
            return Err(taskologic_core::board::BoardError::NegativePurgeDelay.into());
        }
        c.execute(
            "UPDATE boards SET purge_deleted_after_secs = ?2 WHERE id = ?1",
            params![board.id.0, v],
        )?;
    }
    if let Some(d) = &req.description {
        c.execute(
            "UPDATE boards SET description = ?2 WHERE id = ?1",
            params![board.id.0, d.trim()],
        )?;
    }
    if let Some(cards) = &req.card_fields {
        c.execute(
            "UPDATE boards SET card_fields_json = ?2 WHERE id = ?1",
            params![board.id.0, serde_json::to_string(cards)?],
        )?;
    }
    for (role, col) in &req.roles {
        if !board.has_column(*col) {
            return Err(taskologic_core::board::BoardError::ColumnNotOnBoard(*col).into());
        }
        let field = match role {
            ColumnRole::Started => "started_col",
            ColumnRole::Paused => "paused_col",
            ColumnRole::Finished => "finished_col",
        };
        c.execute(
            &format!("UPDATE boards SET {field} = ?2 WHERE id = ?1"),
            params![board.id.0, col.0],
        )?;
    }
    require_board(c, board.id)
}

pub fn delete_board(c: &Connection, id: BoardId) -> R<()> {
    c.execute("DELETE FROM boards WHERE id = ?1", params![id.0])?;
    Ok(())
}

pub fn add_member(c: &Connection, board_id: BoardId, uid: Uid, now: DateTime<Utc>) -> R<()> {
    c.execute(
        "INSERT OR IGNORE INTO board_members (board_id, uid, added_at) VALUES (?1, ?2, ?3)",
        params![board_id.0, i64::from(uid), ts(now)],
    )?;
    Ok(())
}

pub fn remove_member(c: &Connection, board_id: BoardId, uid: Uid) -> R<()> {
    c.execute(
        "DELETE FROM board_members WHERE board_id = ?1 AND uid = ?2",
        params![board_id.0, i64::from(uid)],
    )?;
    Ok(())
}

/// Members as user summaries. On a public board that is every known user.
pub fn list_members(c: &Connection, board: &Board) -> R<Vec<UserSummary>> {
    let users = list_users(c)?;
    Ok(users
        .iter()
        .filter(|u| board.is_member(u.uid))
        .map(UserSummary::from)
        .collect())
}

pub fn add_column(c: &Connection, board: &Board, name: &str) -> R<Column> {
    let mut names: Vec<&str> = board.columns.iter().map(|c| c.name.as_str()).collect();
    names.push(name);
    taskologic_core::board::validate_column_names(&names)?;
    let pos = board
        .columns
        .iter()
        .map(|c| c.position)
        .max()
        .unwrap_or(-10)
        + 10;
    c.execute(
        "INSERT INTO columns (board_id, name, position) VALUES (?1, ?2, ?3)",
        params![board.id.0, name.trim(), pos],
    )?;
    Ok(Column {
        id: ColumnId(c.last_insert_rowid()),
        board_id: board.id,
        name: name.trim().into(),
        position: pos,
        sort_by_due: false,
    })
}

pub fn rename_column(c: &Connection, board: &Board, column_id: ColumnId, name: &str) -> R<()> {
    let names: Vec<&str> = board
        .columns
        .iter()
        .map(|col| {
            if col.id == column_id {
                name
            } else {
                col.name.as_str()
            }
        })
        .collect();
    taskologic_core::board::validate_column_names(&names)?;
    c.execute(
        "UPDATE columns SET name = ?2 WHERE id = ?1",
        params![column_id.0, name.trim()],
    )?;
    Ok(())
}

pub fn reorder_columns(c: &Connection, board: &Board, order: &[ColumnId]) -> R<()> {
    let mut expected: Vec<ColumnId> = board.columns.iter().map(|c| c.id).collect();
    let mut given = order.to_vec();
    expected.sort();
    given.sort();
    if expected != given {
        return Err(AppError::bad(
            "column order must list every column of the board exactly once",
        ));
    }
    for (i, id) in order.iter().enumerate() {
        c.execute(
            "UPDATE columns SET position = ?2 WHERE id = ?1",
            params![id.0, (i as i64) * 10],
        )?;
    }
    Ok(())
}

pub fn set_column_sort(c: &Connection, column_id: ColumnId, sort_by_due: bool) -> R<()> {
    c.execute(
        "UPDATE columns SET sort_by_due = ?2 WHERE id = ?1",
        params![column_id.0, sort_by_due as i64],
    )?;
    Ok(())
}

pub fn remove_column(c: &Connection, board: &Board, plan: &ColumnRemoval) -> R<()> {
    if let Some(dest) = plan.move_tasks_to {
        let base = next_position(c, dest)?;
        // Keep their relative order, appended after what is already there.
        c.execute(
            "UPDATE tasks SET column_id = ?2, position = ?3 + (SELECT count(*) FROM tasks t2 \
             WHERE t2.column_id = ?1 AND t2.position < tasks.position) * 10, version = version + 1 \
             WHERE column_id = ?1",
            params![plan.removed.0, dest.0, base],
        )?;
    }
    for (role, col) in &plan.reassign_roles {
        let field = match role {
            ColumnRole::Started => "started_col",
            ColumnRole::Paused => "paused_col",
            ColumnRole::Finished => "finished_col",
        };
        c.execute(
            &format!("UPDATE boards SET {field} = ?2 WHERE id = ?1"),
            params![board.id.0, col.0],
        )?;
    }
    c.execute("DELETE FROM columns WHERE id = ?1", params![plan.removed.0])?;
    Ok(())
}

pub fn column_board(c: &Connection, column_id: ColumnId) -> R<Option<BoardId>> {
    Ok(c.query_row(
        "SELECT board_id FROM columns WHERE id = ?1",
        params![column_id.0],
        |r| r.get::<_, i64>(0),
    )
    .optional()?
    .map(BoardId))
}

pub fn task_count_in_column(c: &Connection, column_id: ColumnId) -> R<i64> {
    Ok(c.query_row(
        "SELECT count(*) FROM tasks WHERE column_id = ?1",
        params![column_id.0],
        |r| r.get(0),
    )?)
}

fn task_from_row(r: &Row) -> rusqlite::Result<Task> {
    let short: String = r.get("short_id")?;
    let checklist: String = r.get("checklist")?;
    Ok(Task {
        id: TaskId(r.get("id")?),
        short_id: ShortId::parse(&short).unwrap_or(ShortId::from_index(0)),
        board_id: BoardId(r.get("board_id")?),
        column_id: ColumnId(r.get("column_id")?),
        position: r.get("position")?,
        title: r.get("title")?,
        description: r.get("description")?,
        start_at: r.get::<_, Option<i64>>("start_at")?.map(dt),
        due_at: r.get::<_, Option<i64>>("due_at")?.map(dt),
        reminder_start_minutes: r
            .get::<_, Option<i64>>("reminder_start_minutes")?
            .map(|m| m.max(0) as u32),
        reminder_due_minutes: r
            .get::<_, Option<i64>>("reminder_due_minutes")?
            .map(|m| m.max(0) as u32),
        created_by: r.get::<_, i64>("created_by")? as Uid,
        created_at: dt(r.get("created_at")?),
        finished_at: r.get::<_, Option<i64>>("finished_at")?.map(dt),
        version: r.get::<_, i64>("version")? as u64,
        archived_at: r.get::<_, Option<i64>>("archived_at")?.map(dt),
        archived_from_col: r.get::<_, Option<i64>>("archived_from_col")?.map(ColumnId),
        deleted_at: r.get::<_, Option<i64>>("deleted_at")?.map(dt),
        assignees: Vec::new(),
        depends_on: Vec::new(),
        checklist: serde_json::from_str(&checklist).unwrap_or_default(),
        repeat: None,
        template_id: r.get::<_, Option<i64>>("template_id")?.map(TemplateId),
        exclude_from_stats: r.get::<_, i64>("exclude_from_stats")? != 0,
    })
}

fn hydrate(c: &Connection, mut task: Task) -> R<Task> {
    let mut st = c.prepare("SELECT uid FROM assignees WHERE task_id = ?1 ORDER BY uid")?;
    task.assignees = st
        .query_map(params![task.id.0], |r| Ok(r.get::<_, i64>(0)? as Uid))?
        .collect::<Result<_, _>>()?;
    let mut st = c.prepare(
        "SELECT depends_on_task_id FROM deps WHERE task_id = ?1 ORDER BY depends_on_task_id",
    )?;
    task.depends_on = st
        .query_map(params![task.id.0], |r| Ok(TaskId(r.get(0)?)))?
        .collect::<Result<_, _>>()?;
    let rule: Option<String> = c
        .query_row(
            "SELECT rule_json FROM repeats WHERE task_id = ?1 AND active = 1",
            params![task.id.0],
            |r| r.get(0),
        )
        .optional()?;
    task.repeat = rule.and_then(|j| serde_json::from_str(&j).ok());
    Ok(task)
}

pub fn get_task(c: &Connection, id: TaskId) -> R<Option<Task>> {
    let t = c
        .query_row(
            "SELECT * FROM tasks WHERE id = ?1",
            params![id.0],
            task_from_row,
        )
        .optional()?;
    t.map(|t| hydrate(c, t)).transpose()
}

pub fn require_task(c: &Connection, id: TaskId) -> R<Task> {
    get_task(c, id)?.ok_or_else(|| AppError::NotFound("task not found".into()))
}

pub fn task_by_short_id(c: &Connection, short: ShortId) -> R<Option<Task>> {
    let t = c
        .query_row(
            "SELECT * FROM tasks WHERE short_id = ?1",
            params![short.as_str()],
            task_from_row,
        )
        .optional()?;
    t.map(|t| hydrate(c, t)).transpose()
}

pub fn list_tasks(c: &Connection, board_id: BoardId, archived: bool) -> R<Vec<Task>> {
    let sql = if archived {
        "SELECT * FROM tasks WHERE board_id = ?1 AND archived_at IS NOT NULL ORDER BY archived_at DESC, id"
    } else {
        "SELECT * FROM tasks WHERE board_id = ?1 AND archived_at IS NULL ORDER BY column_id, position, id"
    };
    let mut st = c.prepare(sql)?;
    let rows: Vec<Task> = st
        .query_map(params![board_id.0], task_from_row)?
        .collect::<Result<_, _>>()?;
    rows.into_iter().map(|t| hydrate(c, t)).collect()
}

fn next_position(c: &Connection, column_id: ColumnId) -> R<i64> {
    Ok(c.query_row(
        "SELECT COALESCE(MAX(position), -10) + 10 FROM tasks WHERE column_id = ?1 AND archived_at IS NULL",
        params![column_id.0],
        |r| r.get(0),
    )?)
}

fn fresh_short_id(c: &Connection) -> R<ShortId> {
    for _ in 0..32 {
        let n: i64 = c.query_row("SELECT abs(random())", [], |r| r.get(0))?;
        let id = ShortId::from_index(n as u64);
        let taken: i64 = c.query_row(
            "SELECT count(*) FROM tasks WHERE short_id = ?1",
            params![id.as_str()],
            |r| r.get(0),
        )?;
        if taken == 0 {
            return Ok(id);
        }
    }
    Err(anyhow::anyhow!("could not find a free short id after 32 tries").into())
}

fn write_relations(
    c: &Connection,
    task_id: TaskId,
    draft: &TaskDraft,
    now: DateTime<Utc>,
) -> R<()> {
    c.execute(
        "DELETE FROM assignees WHERE task_id = ?1",
        params![task_id.0],
    )?;
    for uid in &draft.assignees {
        c.execute(
            "INSERT OR IGNORE INTO assignees (task_id, uid) VALUES (?1, ?2)",
            params![task_id.0, i64::from(*uid)],
        )?;
    }
    c.execute("DELETE FROM deps WHERE task_id = ?1", params![task_id.0])?;
    for dep in &draft.depends_on {
        c.execute(
            "INSERT OR IGNORE INTO deps (task_id, depends_on_task_id) VALUES (?1, ?2)",
            params![task_id.0, dep.0],
        )?;
    }
    set_repeat(c, task_id, draft.repeat.as_ref(), now)
}

/// `from_template` records which template stamped this out, when one did.
/// Analytics groups sibling tasks by it, and it is set once at creation:
/// editing a task never makes it belong to a template it did not come from.
pub fn create_task(
    c: &Connection,
    board: &Board,
    column: ColumnId,
    draft: &TaskDraft,
    creator: Uid,
    from_template: Option<TemplateId>,
    now: DateTime<Utc>,
) -> R<Task> {
    taskologic_core::task::validate_draft(draft, board)?;
    let short = fresh_short_id(c)?;
    let pos = next_position(c, column)?;
    c.execute(
        "INSERT INTO tasks (short_id, board_id, column_id, position, title, description, start_at, due_at, created_by, created_at, \
         finished_at, version, checklist, reminder_start_minutes, reminder_due_minutes, template_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, ?12, ?13, ?14, ?15)",
        params![
            short.as_str(),
            board.id.0,
            column.0,
            pos,
            draft.title.trim(),
            draft.description,
            draft.start_at.map(ts),
            draft.due_at.map(ts),
            i64::from(creator),
            ts(now),
            (column == board.finished_col).then(|| ts(now)),
            serde_json::to_string(&draft.checklist).unwrap_or_else(|_| "[]".into()),
            draft.reminder_start_minutes.map(i64::from),
            draft.reminder_due_minutes.map(i64::from),
            from_template.map(|t| t.0),
        ],
    )?;
    let id = TaskId(c.last_insert_rowid());
    write_relations(c, id, draft, now)?;
    record_event(
        c,
        board.id,
        Some(id),
        Some(creator),
        &EventKind::TaskCreated {
            column: Some(column),
        },
        now,
    )?;
    require_task(c, id)
}

/// The caller has already compared versions and checked permissions.
pub fn update_task(c: &Connection, task: &Task, draft: &TaskDraft, now: DateTime<Utc>) -> R<Task> {
    c.execute(
        "UPDATE tasks SET title = ?2, description = ?3, start_at = ?4, due_at = ?5, checklist = ?6, \
         reminder_start_minutes = ?7, reminder_due_minutes = ?8, version = version + 1 WHERE id = ?1",
        params![
            task.id.0,
            draft.title.trim(),
            draft.description,
            draft.start_at.map(ts),
            draft.due_at.map(ts),
            serde_json::to_string(&draft.checklist).unwrap_or_else(|_| "[]".into()),
            draft.reminder_start_minutes.map(i64::from),
            draft.reminder_due_minutes.map(i64::from),
        ],
    )?;
    write_relations(c, task.id, draft, now)?;
    require_task(c, task.id)
}

/// Tick or untick one checklist item. Its own write instead of a draft
/// update so ticking from the viewer needs no version and cannot clobber
/// fields somebody else is editing.
pub fn set_checklist_item(c: &Connection, task: &Task, index: usize, done: bool) -> R<Task> {
    let mut items = task.checklist.clone();
    let item = items
        .get_mut(index)
        .ok_or_else(|| AppError::bad("no checklist item at that position"))?;
    item.done = done;
    c.execute(
        "UPDATE tasks SET checklist = ?2, version = version + 1 WHERE id = ?1",
        params![
            task.id.0,
            serde_json::to_string(&items).unwrap_or_else(|_| "[]".into())
        ],
    )?;
    require_task(c, task.id)
}

pub fn apply_move(
    c: &Connection,
    task: &Task,
    plan: &MovePlan,
    position: Option<i64>,
    now: DateTime<Utc>,
) -> R<Task> {
    let pos = match position {
        Some(p) => p,
        None if plan.from == plan.to => task.position,
        None => next_position(c, plan.to)?,
    };
    c.execute(
        "UPDATE tasks SET column_id = ?2, position = ?3, finished_at = ?4, version = version + 1 WHERE id = ?1",
        params![task.id.0, plan.to.0, pos, plan.finished_at.map(ts)],
    )?;
    if plan.from != plan.to {
        record_event(
            c,
            task.board_id,
            Some(task.id),
            None,
            &EventKind::TaskMoved {
                from: plan.from,
                to: plan.to,
            },
            now,
        )?;
    }
    require_task(c, task.id)
}

/// Move to the archive marked as deleted. The repeat, if any, pauses.
pub fn soft_delete_task(c: &Connection, task: &Task, now: DateTime<Utc>) -> R<Task> {
    c.execute(
        "UPDATE tasks SET archived_at = ?2, deleted_at = ?2, archived_from_col = column_id, version = version + 1 WHERE id = ?1",
        params![task.id.0, ts(now)],
    )?;
    c.execute(
        "UPDATE repeats SET active = 0 WHERE task_id = ?1",
        params![task.id.0],
    )?;
    require_task(c, task.id)
}

/// Remove the row for good. Dependency links and assignees cascade away.
pub fn purge_task(c: &Connection, id: TaskId) -> R<()> {
    c.execute("DELETE FROM tasks WHERE id = ?1", params![id.0])?;
    Ok(())
}

pub fn archive_task(c: &Connection, task: &Task, now: DateTime<Utc>) -> R<Task> {
    c.execute(
        "UPDATE tasks SET archived_at = ?2, archived_from_col = column_id, version = version + 1 WHERE id = ?1",
        params![task.id.0, ts(now)],
    )?;
    require_task(c, task.id)
}

pub fn restore_task(c: &Connection, task: &Task, to: ColumnId, now: DateTime<Utc>) -> R<Task> {
    let pos = next_position(c, to)?;
    c.execute(
        "UPDATE tasks SET archived_at = NULL, archived_from_col = NULL, deleted_at = NULL, column_id = ?2, \
         position = ?3, finished_at = NULL, version = version + 1 WHERE id = ?1",
        params![task.id.0, to.0, pos],
    )?;
    // A repeat paused by a delete picks up again from now.
    let rule: Option<String> = c
        .query_row(
            "SELECT rule_json FROM repeats WHERE task_id = ?1",
            params![task.id.0],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(spec) = rule.and_then(|j| serde_json::from_str::<RepeatSpec>(&j).ok()) {
        c.execute(
            "UPDATE repeats SET active = 1, next_fire_at = ?2 WHERE task_id = ?1",
            params![task.id.0, spec.next_fire_after(now).map(ts)],
        )?;
    }
    require_task(c, task.id)
}

/// Dependencies of `task` that are neither in their board's finished column
/// nor archived. Deleted ones are gone from the table already.
pub fn open_deps(c: &Connection, task: &Task) -> R<Vec<TaskId>> {
    let mut st = c.prepare(
        "SELECT t.id FROM deps d JOIN tasks t ON t.id = d.depends_on_task_id JOIN boards b ON b.id = t.board_id \
         WHERE d.task_id = ?1 AND t.archived_at IS NULL AND t.column_id != b.finished_col ORDER BY t.id",
    )?;
    let rows = st.query_map(params![task.id.0], |r| Ok(TaskId(r.get(0)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn dep_lines(c: &Connection, task: &Task) -> R<Vec<DepLine>> {
    let mut st = c.prepare(
        "SELECT t.short_id, t.title, (t.archived_at IS NOT NULL OR t.column_id = b.finished_col) \
         FROM deps d JOIN tasks t ON t.id = d.depends_on_task_id JOIN boards b ON b.id = t.board_id \
         WHERE d.task_id = ?1 ORDER BY t.id",
    )?;
    let rows = st.query_map(params![task.id.0], |r| {
        let s: String = r.get(0)?;
        Ok(DepLine {
            short_id: ShortId::parse(&s).unwrap_or(ShortId::from_index(0)),
            title: r.get(1)?,
            done: r.get::<_, i64>(2)? != 0,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn deps_graph(c: &Connection) -> R<HashMap<TaskId, Vec<TaskId>>> {
    let mut st = c.prepare("SELECT task_id, depends_on_task_id FROM deps")?;
    let mut graph: HashMap<TaskId, Vec<TaskId>> = HashMap::new();
    for row in st.query_map([], |r| Ok((TaskId(r.get(0)?), TaskId(r.get(1)?))))? {
        let (t, d) = row?;
        graph.entry(t).or_default().push(d);
    }
    Ok(graph)
}

pub fn assignee_uids_on_board(c: &Connection, board_id: BoardId) -> R<Vec<Uid>> {
    let mut st = c.prepare(
        "SELECT DISTINCT a.uid FROM assignees a JOIN tasks t ON t.id = a.task_id WHERE t.board_id = ?1 AND t.archived_at IS NULL",
    )?;
    let rows = st.query_map(params![board_id.0], |r| Ok(r.get::<_, i64>(0)? as Uid))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn tasks_assigned_on_board(c: &Connection, board_id: BoardId, uid: Uid) -> R<Vec<TaskId>> {
    let mut st = c.prepare(
        "SELECT t.id FROM assignees a JOIN tasks t ON t.id = a.task_id WHERE t.board_id = ?1 AND a.uid = ?2 AND t.archived_at IS NULL",
    )?;
    let rows = st.query_map(params![board_id.0, i64::from(uid)], |r| {
        Ok(TaskId(r.get(0)?))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn unassign_on_board(c: &Connection, board_id: BoardId, uid: Uid) -> R<Vec<TaskId>> {
    let tasks = tasks_assigned_on_board(c, board_id, uid)?;
    for t in &tasks {
        c.execute(
            "DELETE FROM assignees WHERE task_id = ?1 AND uid = ?2",
            params![t.0, i64::from(uid)],
        )?;
        c.execute(
            "UPDATE tasks SET version = version + 1 WHERE id = ?1",
            params![t.0],
        )?;
    }
    Ok(tasks)
}

pub fn search(c: &Connection, uid: Uid, query: &str, include_archived: bool) -> R<Vec<SearchHit>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let escaped = q
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    let archived = if include_archived {
        ""
    } else {
        "AND t.archived_at IS NULL"
    };
    let sql = format!(
        "SELECT t.id, b.id, b.name, c.name, t.title, t.archived_at IS NOT NULL \
         FROM tasks t JOIN boards b ON b.id = t.board_id JOIN columns c ON c.id = t.column_id \
         WHERE b.id IN ({VISIBLE_BOARDS}) AND (t.title LIKE ?2 ESCAPE '\\' OR t.description LIKE ?2 ESCAPE '\\') \
         {archived} ORDER BY t.archived_at IS NOT NULL, b.name, t.title LIMIT 100"
    );
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map(params![i64::from(uid), pattern], |r| {
        Ok(SearchHit {
            task_id: TaskId(r.get(0)?),
            board_id: BoardId(r.get(1)?),
            board_name: r.get(2)?,
            column_name: r.get(3)?,
            title: r.get(4)?,
            archived: r.get::<_, i64>(5)? != 0,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Deleted tasks whose retention period has run out.
pub fn tasks_due_for_purge(c: &Connection, now: DateTime<Utc>) -> R<Vec<Task>> {
    let mut st = c.prepare(
        "SELECT t.* FROM tasks t JOIN boards b ON b.id = t.board_id \
         WHERE t.deleted_at IS NOT NULL AND t.deleted_at + b.purge_deleted_after_secs <= ?1",
    )?;
    let rows: Vec<Task> = st
        .query_map(params![ts(now)], task_from_row)?
        .collect::<Result<_, _>>()?;
    rows.into_iter().map(|t| hydrate(c, t)).collect()
}

pub fn tasks_due_for_archive(c: &Connection, now: DateTime<Utc>) -> R<Vec<Task>> {
    let mut st = c.prepare(
        "SELECT t.* FROM tasks t JOIN boards b ON b.id = t.board_id \
         WHERE t.archived_at IS NULL AND t.column_id = b.finished_col AND t.finished_at IS NOT NULL \
         AND t.finished_at + b.archive_after_secs <= ?1",
    )?;
    let rows: Vec<Task> = st
        .query_map(params![ts(now)], task_from_row)?
        .collect::<Result<_, _>>()?;
    rows.into_iter().map(|t| hydrate(c, t)).collect()
}

pub fn record_event(
    c: &Connection,
    board_id: BoardId,
    task_id: Option<TaskId>,
    actor: Option<Uid>,
    kind: &EventKind,
    now: DateTime<Utc>,
) -> R<EventId> {
    let (from, to) = kind.columns();
    c.execute(
        "INSERT INTO events (task_id, board_id, actor_uid, kind, from_col, to_col, detail_json, at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            task_id.map(|t| t.0),
            board_id.0,
            actor.map(i64::from),
            kind.name(),
            from.map(|c| c.0),
            to.map(|c| c.0),
            serde_json::to_string(kind)?,
            ts(now),
        ],
    )?;
    Ok(EventId(c.last_insert_rowid()))
}

#[cfg(test)]
pub fn event_count(c: &Connection, task_id: TaskId, kind: &str) -> R<i64> {
    Ok(c.query_row(
        "SELECT count(*) FROM events WHERE task_id = ?1 AND kind = ?2",
        params![task_id.0, kind],
        |r| r.get(0),
    )?)
}

pub fn set_repeat(
    c: &Connection,
    task_id: TaskId,
    spec: Option<&RepeatSpec>,
    now: DateTime<Utc>,
) -> R<()> {
    match spec {
        Some(spec) => {
            spec.validate().map_err(|e| AppError::bad(e.to_string()))?;
            let next = spec.next_fire_after(now).map(ts);
            c.execute(
                "INSERT INTO repeats (task_id, rule_json, next_fire_at, active) VALUES (?1, ?2, ?3, 1) \
                 ON CONFLICT(task_id) DO UPDATE SET rule_json = excluded.rule_json, next_fire_at = excluded.next_fire_at, active = 1",
                params![task_id.0, serde_json::to_string(spec)?, next],
            )?;
        }
        None => {
            c.execute("DELETE FROM repeats WHERE task_id = ?1", params![task_id.0])?;
        }
    }
    Ok(())
}

pub fn due_repeats(c: &Connection, now: DateTime<Utc>) -> R<Vec<(Task, RepeatSpec)>> {
    let mut st = c.prepare(
        "SELECT t.*, r.rule_json FROM repeats r JOIN tasks t ON t.id = r.task_id \
         WHERE r.active = 1 AND r.next_fire_at IS NOT NULL AND r.next_fire_at <= ?1",
    )?;
    let rows: Vec<(Task, String)> = st
        .query_map(params![ts(now)], |r| {
            Ok((task_from_row(r)?, r.get("rule_json")?))
        })?
        .collect::<Result<_, _>>()?;
    rows.into_iter()
        .filter_map(|(t, j)| serde_json::from_str::<RepeatSpec>(&j).ok().map(|s| (t, s)))
        .map(|(t, s)| Ok((hydrate(c, t)?, s)))
        .collect()
}

pub fn advance_repeat(c: &Connection, task_id: TaskId, next: Option<DateTime<Utc>>) -> R<()> {
    match next {
        Some(n) => c.execute(
            "UPDATE repeats SET next_fire_at = ?2 WHERE task_id = ?1",
            params![task_id.0, ts(n)],
        )?,
        None => c.execute(
            "UPDATE repeats SET active = 0, next_fire_at = NULL WHERE task_id = ?1",
            params![task_id.0],
        )?,
    };
    Ok(())
}

/// Active repetitions on one board with their next fire time, soonest first.
/// Paused ones (deleted tasks) are left out, they resume on restore.
pub fn list_repeats(c: &Connection, board_id: BoardId) -> R<Vec<(Task, Option<DateTime<Utc>>)>> {
    let mut st = c.prepare(
        "SELECT t.*, r.next_fire_at FROM repeats r JOIN tasks t ON t.id = r.task_id \
         WHERE t.board_id = ?1 AND r.active = 1 \
         ORDER BY r.next_fire_at IS NULL, r.next_fire_at, t.id",
    )?;
    let rows: Vec<(Task, Option<i64>)> = st
        .query_map(params![board_id.0], |r| {
            Ok((task_from_row(r)?, r.get("next_fire_at")?))
        })?
        .collect::<Result<_, _>>()?;
    rows.into_iter()
        .map(|(t, n)| Ok((hydrate(c, t)?, n.map(dt))))
        .collect()
}

/// Turn a repetition off for good. The version bump makes open editors
/// notice the task changed under them.
pub fn stop_repeat(c: &Connection, task: &Task) -> R<Task> {
    c.execute("DELETE FROM repeats WHERE task_id = ?1", params![task.id.0])?;
    c.execute(
        "UPDATE tasks SET version = version + 1 WHERE id = ?1",
        params![task.id.0],
    )?;
    require_task(c, task.id)
}

fn template_from_row(r: &Row) -> rusqlite::Result<Template> {
    let payload: String = r.get("payload_json")?;
    let options: String = r.get("options_json")?;
    Ok(Template {
        id: TemplateId(r.get("id")?),
        board_id: BoardId(r.get("board_id")?),
        owner_uid: r.get::<_, i64>("owner_uid")? as Uid,
        name: r.get("name")?,
        draft: serde_json::from_str(&payload).unwrap_or_default(),
        options: serde_json::from_str(&options).unwrap_or_default(),
    })
}

pub fn list_templates(c: &Connection, board_id: BoardId) -> R<Vec<Template>> {
    let mut st =
        c.prepare("SELECT * FROM templates WHERE board_id = ?1 ORDER BY name COLLATE NOCASE, id")?;
    let rows: Vec<Template> = st
        .query_map(params![board_id.0], template_from_row)?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

pub fn get_template(c: &Connection, id: TemplateId) -> R<Option<Template>> {
    Ok(c.query_row(
        "SELECT * FROM templates WHERE id = ?1",
        params![id.0],
        template_from_row,
    )
    .optional()?)
}

pub fn require_template(c: &Connection, id: TemplateId) -> R<Template> {
    get_template(c, id)?.ok_or_else(|| AppError::NotFound("template not found".into()))
}

pub fn create_template(
    c: &Connection,
    board_id: BoardId,
    owner: Uid,
    name: &str,
    draft: &TaskDraft,
    options: &TemplateOptions,
) -> R<Template> {
    c.execute(
        "INSERT INTO templates (board_id, owner_uid, name, payload_json, options_json) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            board_id.0,
            i64::from(owner),
            name,
            serde_json::to_string(draft)?,
            serde_json::to_string(options)?
        ],
    )?;
    require_template(c, TemplateId(c.last_insert_rowid()))
}

pub fn update_template(
    c: &Connection,
    id: TemplateId,
    name: &str,
    draft: &TaskDraft,
    options: &TemplateOptions,
) -> R<Template> {
    c.execute(
        "UPDATE templates SET name = ?2, payload_json = ?3, options_json = ?4 WHERE id = ?1",
        params![
            id.0,
            name,
            serde_json::to_string(draft)?,
            serde_json::to_string(options)?
        ],
    )?;
    require_template(c, id)
}

/// Every template on a board mapped to the templates it depends on. Used to
/// check for cycles and to work out what a template stamps out.
pub fn template_deps_graph(
    c: &Connection,
    board_id: BoardId,
) -> R<HashMap<TemplateId, Vec<TemplateId>>> {
    Ok(list_templates(c, board_id)?
        .into_iter()
        .map(|t| (t.id, t.options.dep_templates))
        .collect())
}

pub fn delete_template(c: &Connection, id: TemplateId) -> R<()> {
    c.execute("DELETE FROM templates WHERE id = ?1", params![id.0])?;
    Ok(())
}

/// Who has already had this task printed for them automatically.
pub fn autoprinted_uids(c: &Connection, task: TaskId) -> R<Vec<Uid>> {
    let mut q = c.prepare("SELECT uid FROM task_autoprint WHERE task_id = ?1")?;
    let uids = q
        .query_map(params![task.0], |r| r.get::<_, Uid>(0))?
        .collect::<Result<_, _>>()?;
    Ok(uids)
}

/// Remember that this person has had a slip for this task. Idempotent: a
/// second move into the started column must not queue a second slip.
pub fn mark_autoprinted(c: &Connection, task: TaskId, uid: Uid, now: DateTime<Utc>) -> R<()> {
    c.execute(
        "INSERT OR IGNORE INTO task_autoprint (task_id, uid, printed_at) VALUES (?1, ?2, ?3)",
        params![task.0, uid, now.timestamp()],
    )?;
    Ok(())
}

pub fn enqueue_print(
    c: &Connection,
    uid: Uid,
    job: &PrintJob,
    max_age: TimeDelta,
    now: DateTime<Utc>,
) -> R<PrintJobId> {
    c.execute(
        "INSERT INTO print_queue (target_uid, job_json, created_at, expires_at, attempts) VALUES (?1, ?2, ?3, ?4, 0)",
        params![i64::from(uid), serde_json::to_string(job)?, ts(now), ts(now + max_age)],
    )?;
    Ok(PrintJobId(c.last_insert_rowid()))
}

/// Jobs waiting for a client: not expired and not currently handed to one.
pub fn pending_print_jobs(
    c: &Connection,
    uid: Uid,
    now: DateTime<Utc>,
) -> R<Vec<(PrintJobId, PrintJob)>> {
    let mut st = c.prepare(
        "SELECT id, job_json FROM print_queue WHERE target_uid = ?1 AND expires_at > ?2 AND in_flight_since IS NULL \
         ORDER BY created_at, id",
    )?;
    let rows: Vec<(i64, String)> = st
        .query_map(params![i64::from(uid), ts(now)], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, j)| {
            serde_json::from_str(&j)
                .ok()
                .map(|job| (PrintJobId(id), job))
        })
        .collect())
}

pub fn mark_print_in_flight(c: &Connection, id: PrintJobId, now: DateTime<Utc>) -> R<()> {
    c.execute(
        "UPDATE print_queue SET in_flight_since = ?2 WHERE id = ?1",
        params![id.0, ts(now)],
    )?;
    Ok(())
}

/// A job handed to a client that vanished goes back to pending.
pub fn release_stale_in_flight(c: &Connection, older_than: DateTime<Utc>) -> R<usize> {
    Ok(c.execute(
        "UPDATE print_queue SET in_flight_since = NULL WHERE in_flight_since IS NOT NULL AND in_flight_since < ?1",
        params![ts(older_than)],
    )?)
}

pub fn ack_print_ok(c: &Connection, id: PrintJobId) -> R<()> {
    c.execute("DELETE FROM print_queue WHERE id = ?1", params![id.0])?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum PrintAck {
    Requeued { attempts: i64 },
    Dropped,
}

pub const MAX_PRINT_ATTEMPTS: i64 = 3;

/// A dropped job leaves a trace in the event log. Skipped silently when the
/// task is already purged, there is no board to file it under then.
fn record_print_drop(c: &Connection, job_json: &str, reason: &str, now: DateTime<Utc>) -> R<()> {
    let Ok(job) = serde_json::from_str::<PrintJob>(job_json) else {
        return Ok(());
    };
    let board: Option<i64> = c
        .query_row(
            "SELECT board_id FROM tasks WHERE id = ?1",
            params![job.task_id.0],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(b) = board {
        record_event(
            c,
            BoardId(b),
            Some(job.task_id),
            None,
            &EventKind::PrintJobDropped {
                reason: reason.into(),
            },
            now,
        )?;
    }
    Ok(())
}

pub fn ack_print_err(
    c: &Connection,
    id: PrintJobId,
    error: &str,
    now: DateTime<Utc>,
) -> R<PrintAck> {
    c.execute(
        "UPDATE print_queue SET attempts = attempts + 1, last_error = ?2, in_flight_since = NULL WHERE id = ?1",
        params![id.0, error],
    )?;
    let (attempts, job_json): (i64, String) = c.query_row(
        "SELECT attempts, job_json FROM print_queue WHERE id = ?1",
        params![id.0],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if attempts >= MAX_PRINT_ATTEMPTS {
        c.execute("DELETE FROM print_queue WHERE id = ?1", params![id.0])?;
        record_print_drop(
            c,
            &job_json,
            &format!("failed {attempts} times, last error: {error}"),
            now,
        )?;
        Ok(PrintAck::Dropped)
    } else {
        Ok(PrintAck::Requeued { attempts })
    }
}

pub fn expire_print_jobs(c: &Connection, now: DateTime<Utc>) -> R<usize> {
    let mut st = c.prepare("SELECT job_json FROM print_queue WHERE expires_at <= ?1")?;
    let jobs: Vec<String> = st
        .query_map(params![ts(now)], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for j in &jobs {
        record_print_drop(c, j, "expired before any printer took it", now)?;
    }
    c.execute(
        "DELETE FROM print_queue WHERE expires_at <= ?1",
        params![ts(now)],
    )?;
    Ok(jobs.len())
}

pub fn uids_with_pending_print_jobs(c: &Connection, now: DateTime<Utc>) -> R<Vec<Uid>> {
    let mut st = c.prepare(
        "SELECT DISTINCT target_uid FROM print_queue WHERE expires_at > ?1 AND in_flight_since IS NULL",
    )?;
    let rows = st.query_map(params![ts(now)], |r| Ok(r.get::<_, i64>(0)? as Uid))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Unfinished tasks carrying at least one date that `uid` is assigned to, or
/// created and left unassigned. A task with neither date owes no reminder, so
/// it never reaches the scheduler in the first place.
pub fn reminder_candidates(c: &Connection, uid: Uid) -> R<Vec<Task>> {
    let mut st = c.prepare(
        "SELECT t.* FROM tasks t JOIN boards b ON b.id = t.board_id \
         WHERE t.archived_at IS NULL AND t.column_id != b.finished_col \
           AND (t.due_at IS NOT NULL OR t.start_at IS NOT NULL) AND ( \
           t.id IN (SELECT task_id FROM assignees WHERE uid = ?1) \
           OR (t.created_by = ?1 AND NOT EXISTS (SELECT 1 FROM assignees a WHERE a.task_id = t.id)))",
    )?;
    let rows: Vec<Task> = st
        .query_map(params![i64::from(uid)], task_from_row)?
        .collect::<Result<_, _>>()?;
    rows.into_iter().map(|t| hydrate(c, t)).collect()
}

/// Whether this person already had this particular slip. Keyed on the date it
/// counted back from, so moving a due date earns a fresh reminder instead of
/// being swallowed by the one already sent.
pub fn reminder_sent(
    c: &Connection,
    task_id: TaskId,
    uid: Uid,
    kind: ReminderKind,
    anchor_at: DateTime<Utc>,
) -> R<bool> {
    let n: i64 = c.query_row(
        "SELECT count(*) FROM reminders_sent WHERE task_id = ?1 AND uid = ?2 AND kind = ?3 AND anchor_at = ?4",
        params![task_id.0, i64::from(uid), kind.name(), ts(anchor_at)],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

pub fn mark_reminder_sent(
    c: &Connection,
    task_id: TaskId,
    uid: Uid,
    kind: ReminderKind,
    anchor_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> R<()> {
    c.execute(
        "INSERT OR IGNORE INTO reminders_sent (task_id, uid, kind, anchor_at, sent_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            task_id.0,
            i64::from(uid),
            kind.name(),
            ts(anchor_at),
            ts(now)
        ],
    )?;
    Ok(())
}

/// Whether the task was ever in the board's started column, from the event
/// log rather than from where it sits now. A board can have columns past the
/// started one, so "further along than started" is not a question the current
/// column can answer.
pub fn was_ever_started(c: &Connection, task_id: TaskId, started_col: ColumnId) -> R<bool> {
    let n: i64 = c.query_row(
        "SELECT count(*) FROM events WHERE task_id = ?1 AND to_col = ?2",
        params![task_id.0, started_col.0],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

// ---------------------------------------------------------------------------
// Analytics
//
// The event log has been filling up since 0.1.0 for exactly this. Nothing
// below writes; it reads the trail back and hands it to `taskologic_core::stats`,
// which owns every rule about what the numbers mean.
// ---------------------------------------------------------------------------

/// Live and archived tasks on the boards `uid` may see, newest first, each
/// with the name of the board it is on. One board narrows it to that board,
/// after the same visibility check every other request makes.
///
/// Purged tasks are gone from this table by definition, so a row here always
/// has a title to show even when its events outlive it.
pub fn analytics_candidates(
    c: &Connection,
    uid: Uid,
    board: Option<BoardId>,
) -> R<Vec<(Task, String)>> {
    let sql = format!(
        "SELECT t.*, b.name AS board_name FROM tasks t JOIN boards b ON b.id = t.board_id \
         WHERE t.board_id IN ({VISIBLE_BOARDS}) AND (?2 IS NULL OR t.board_id = ?2) \
         ORDER BY COALESCE(t.finished_at, t.archived_at, t.created_at) DESC, t.id DESC"
    );
    let mut st = c.prepare(&sql)?;
    let rows: Vec<(Task, String)> = st
        .query_map(params![i64::from(uid), board.map(|b| b.0)], |r| {
            Ok((task_from_row(r)?, r.get::<_, String>("board_name")?))
        })?
        .collect::<Result<_, _>>()?;
    // Assignees are needed for the "assigned to me" filter; dependencies and
    // repetitions are not, so the hydrate is deliberately partial.
    rows.into_iter()
        .map(|(mut t, name)| {
            let mut st =
                c.prepare("SELECT uid FROM assignees WHERE task_id = ?1 ORDER BY uid")?;
            t.assignees = st
                .query_map(params![t.id.0], |r| Ok(r.get::<_, i64>(0)? as Uid))?
                .collect::<Result<_, _>>()?;
            Ok((t, name))
        })
        .collect()
}

/// Every column change for these tasks, oldest first, ready for
/// [`taskologic_core::stats::timings`].
pub fn transitions_for(
    c: &Connection,
    tasks: &[TaskId],
) -> R<HashMap<TaskId, Vec<stats::Transition>>> {
    let mut out: HashMap<TaskId, Vec<stats::Transition>> = HashMap::new();
    if tasks.is_empty() {
        return Ok(out);
    }
    // The kinds that move a task between columns, and the creation that put
    // it in its first one. Anything else leaves the trail unchanged.
    let mut st = c.prepare(
        "SELECT task_id, to_col, at FROM events \
         WHERE task_id = ?1 AND kind IN ('task_created', 'task_moved', 'task_archived', 'task_restored') \
         ORDER BY at, id",
    )?;
    for id in tasks {
        let rows = st.query_map(params![id.0], |r| {
            Ok(stats::Transition {
                at: dt(r.get("at")?),
                to: r.get::<_, Option<i64>>("to_col")?.map(ColumnId),
            })
        })?;
        let trail: Vec<stats::Transition> = rows.collect::<Result<_, _>>()?;
        if !trail.is_empty() {
            out.insert(*id, trail);
        }
    }
    Ok(out)
}

/// Child to parent for every repetition chain on the boards `uid` may see.
/// A repeated task is a sibling of the ones before it in its chain, which is
/// how the averages group them without a template to go by.
pub fn repeat_parents(c: &Connection, uid: Uid) -> R<HashMap<TaskId, TaskId>> {
    let sql = format!(
        "SELECT task_id, detail_json FROM events \
         WHERE kind = 'repeat_spawned' AND task_id IS NOT NULL AND board_id IN ({VISIBLE_BOARDS})"
    );
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map(params![i64::from(uid)], |r| {
        Ok((r.get::<_, i64>("task_id")?, r.get::<_, String>("detail_json")?))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (child, detail) = row?;
        if let Ok(EventKind::RepeatSpawned { from_task }) =
            serde_json::from_str::<EventKind>(&detail)
        {
            out.insert(TaskId(child), from_task);
        }
    }
    Ok(out)
}

/// One task's whole recorded history, oldest first, with the name of whoever
/// did each thing. The scheduler acts as nobody, which is why the name is
/// optional rather than a lie about a user.
pub fn task_history(c: &Connection, task_id: TaskId) -> R<Vec<(Event, Option<String>)>> {
    let mut st = c.prepare(
        "SELECT e.id, e.board_id, e.task_id, e.actor_uid, e.detail_json, e.at, u.username \
         FROM events e LEFT JOIN users u ON u.uid = e.actor_uid \
         WHERE e.task_id = ?1 ORDER BY e.at, e.id",
    )?;
    let rows = st.query_map(params![task_id.0], |r| {
        let detail: String = r.get("detail_json")?;
        let kind = serde_json::from_str::<EventKind>(&detail).unwrap_or(EventKind::TaskReordered);
        Ok((
            Event {
                id: EventId(r.get("id")?),
                board_id: BoardId(r.get("board_id")?),
                task_id: r.get::<_, Option<i64>>("task_id")?.map(TaskId),
                actor_uid: r.get::<_, Option<i64>>("actor_uid")?.map(|u| u as Uid),
                kind,
                at: dt(r.get("at")?),
            },
            r.get::<_, Option<String>>("username")?,
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Take a task in or out of the averages.
pub fn set_exclude_from_stats(c: &Connection, task_id: TaskId, excluded: bool) -> R<Task> {
    c.execute(
        "UPDATE tasks SET exclude_from_stats = ?2, version = version + 1 WHERE id = ?1",
        params![task_id.0, i64::from(excluded)],
    )?;
    require_task(c, task_id)
}

/// How long the tasks stamped out of `template` have taken, and over how many
/// of them, for the template's "due from average" rule. Excluded tasks and
/// ones that were never started do not count.
pub fn template_average(
    c: &Connection,
    template: TemplateId,
    now: DateTime<Utc>,
) -> R<Option<(TimeDelta, u32)>> {
    let mut st = c.prepare(
        "SELECT t.id, b.started_col, b.finished_col FROM tasks t JOIN boards b ON b.id = t.board_id \
         WHERE t.template_id = ?1 AND t.exclude_from_stats = 0 AND t.deleted_at IS NULL",
    )?;
    let rows: Vec<(TaskId, ColumnId, ColumnId)> = st
        .query_map(params![template.0], |r| {
            Ok((
                TaskId(r.get("id")?),
                ColumnId(r.get("started_col")?),
                ColumnId(r.get("finished_col")?),
            ))
        })?
        .collect::<Result<_, _>>()?;

    let ids: Vec<TaskId> = rows.iter().map(|(id, _, _)| *id).collect();
    let trails = transitions_for(c, &ids)?;
    let samples: Vec<TimeDelta> = rows
        .iter()
        .filter_map(|(id, started, finished)| {
            let trail = trails.get(id)?;
            let t = stats::timings(trail, *started, *finished, now);
            // Only tasks that actually finished say anything about how long
            // this kind of work takes. One still running would report the
            // time since it started, which is not the same number.
            t.finished_at?;
            t.time_taken
        })
        .collect();
    Ok(stats::average(&samples))
}
