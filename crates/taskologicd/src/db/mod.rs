//! The daemon is the only thing that ever touches the database file.
//!
//! One connection behind a mutex. SQLite calls here are sub millisecond and
//! the handlers run them inline. If contention ever shows up in profiles the
//! fix is a small blocking pool, not a rewrite, because everything goes
//! through `Db::with` and `Db::tx`.

pub mod repo;

use std::path::Path;
use std::sync::Mutex;

use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

use crate::error::AppError;

pub fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(include_str!("../../migrations/0001_init.sql")),
        M::up(include_str!("../../migrations/0002_soft_delete.sql")),
        M::up(include_str!("../../migrations/0003_cards_and_sort.sql")),
        M::up(include_str!("../../migrations/0004_board_description.sql")),
        M::up(include_str!("../../migrations/0005_checklist.sql")),
        M::up(include_str!("../../migrations/0006_autoprint_once.sql")),
        M::up(include_str!(
            "../../migrations/0007_reminders_and_template_options.sql"
        )),
        M::up(include_str!("../../migrations/0008_time_tracking.sql")),
    ])
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Db> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> anyhow::Result<Db> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> anyhow::Result<Db> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrations()
            .to_latest(&mut conn)
            .context("running migrations")?;
        Ok(Db {
            conn: Mutex::new(conn),
        })
    }

    pub fn with<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        f(&conn)
    }

    /// Runs `f` in a transaction, committing on Ok.
    pub fn tx<T>(&self, f: impl FnOnce(&Connection) -> Result<T, AppError>) -> Result<T, AppError> {
        let mut conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let tx = conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}

pub fn ts(t: DateTime<Utc>) -> i64 {
    t.timestamp()
}

pub fn dt(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs, 0).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_valid_and_apply() {
        migrations().validate().unwrap();
        let db = Db::open_in_memory().unwrap();
        let n: i64 = db
            .with(|c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert!(n >= 11, "expected all tables, found {n}");
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use rusqlite::params;

    /// A database as 0.1.10 left it, with a task, a reminder already sent and
    /// a template, then brought forward. This is what `make update` does to a
    /// running install, so nothing here may need a hand afterwards.
    #[test]
    fn a_populated_0_1_10_database_comes_forward_intact() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        // Everything up to and not including the time tracking migration.
        let old = Migrations::new(
            [
                include_str!("../../migrations/0001_init.sql"),
                include_str!("../../migrations/0002_soft_delete.sql"),
                include_str!("../../migrations/0003_cards_and_sort.sql"),
                include_str!("../../migrations/0004_board_description.sql"),
                include_str!("../../migrations/0005_checklist.sql"),
                include_str!("../../migrations/0006_autoprint_once.sql"),
                include_str!("../../migrations/0007_reminders_and_template_options.sql"),
            ]
            .map(M::up)
            .to_vec(),
        );
        old.to_latest(&mut conn).unwrap();

        conn.execute(
            "INSERT INTO boards (id, name, owner_uid, archive_after_secs, started_col, \
             paused_col, finished_col, created_at) VALUES (1, 'Kitchen', 1, 60, 2, 3, 4, 0)",
            [],
        )
        .unwrap();
        for (id, name, pos) in [(1, "Todo", 0), (2, "Doing", 1), (3, "Wait", 2), (4, "Done", 3)] {
            conn.execute(
                "INSERT INTO columns (id, board_id, name, position) VALUES (?1, 1, ?2, ?3)",
                params![id, name, pos],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO tasks (id, short_id, board_id, column_id, position, title, due_at, \
             created_by, created_at, version, reminder_minutes) \
             VALUES (1, 'AAA-111', 1, 1, 0, 'Water plants', 5000, 1, 0, 1, 90)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO reminders_sent (task_id, uid, due_at, sent_at) VALUES (1, 1, 5000, 4000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO templates (id, board_id, owner_uid, name, payload_json, options_json) \
             VALUES (1, 1, 1, 'Weekly', '{}', '{\"due_prefill\":{\"amount\":2,\"unit\":\"hours\"}}')",
            [],
        )
        .unwrap();

        migrations().to_latest(&mut conn).unwrap();

        // The reminder override survives under its new name, and the columns
        // that did not exist are simply empty.
        let (due_lead, start_lead, start_at, excluded, template): (
            Option<i64>,
            Option<i64>,
            Option<i64>,
            i64,
            Option<i64>,
        ) = conn
            .query_row(
                "SELECT reminder_due_minutes, reminder_start_minutes, start_at, \
                 exclude_from_stats, template_id FROM tasks WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(due_lead, Some(90), "the old override always meant 'before due'");
        assert_eq!(start_lead, None, "nobody gains a start reminder they never set");
        assert_eq!(start_at, None);
        assert_eq!(excluded, 0);
        assert_eq!(template, None, "per template history starts at 0.1.11");

        // The rebuilt bookkeeping table keeps what it knew, labelled.
        let (kind, anchor): (String, i64) = conn
            .query_row(
                "SELECT kind, anchor_at FROM reminders_sent WHERE task_id = 1 AND uid = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((kind.as_str(), anchor), ("due", 5000));

        // A start reminder for the same task is a different row now, which is
        // the whole point of widening the key.
        conn.execute(
            "INSERT INTO reminders_sent (task_id, uid, kind, anchor_at, sent_at) \
             VALUES (1, 1, 'start', 5000, 4100)",
            [],
        )
        .unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM reminders_sent", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);

        // Deleting the task still takes its reminder rows with it, so the new
        // table kept the foreign key the old one had.
        conn.execute("DELETE FROM tasks WHERE id = 1", params![])
            .unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM reminders_sent", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "the cascade survived the rebuild");

        // The template's saved rule still loads, now alongside the new ones.
        let json: String = conn
            .query_row("SELECT options_json FROM templates WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        let opts: taskologic_core::template::TemplateOptions =
            serde_json::from_str(&json).unwrap();
        assert_eq!(opts.due_prefill.map(|p| p.amount), Some(2));
        assert_eq!(opts.start_prefill, None);
    }
}
