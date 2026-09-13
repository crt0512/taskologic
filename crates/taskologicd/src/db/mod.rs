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
