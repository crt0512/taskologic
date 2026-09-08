use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

pub const DEFAULT_PATH: &str = "/etc/taskologic/taskologicd.toml";

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub socket_path: PathBuf,
    pub db_path: PathBuf,
    /// System group whose members are Taskologic users.
    pub group: String,
    /// Uids that are admins no matter what the database says.
    pub always_admin_uids: Vec<u32>,
    /// Queued print jobs older than this are dropped unprinted.
    pub print_job_max_age_secs: u64,
    pub scheduler_tick_secs: u64,
    /// IANA zone used as the default for new users. Falls back to `TZ`,
    /// then UTC.
    pub host_timezone: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::from("/run/taskologic/taskologicd.sock"),
            db_path: PathBuf::from("/var/lib/taskologic/taskologic.db"),
            group: "taskologic".into(),
            always_admin_uids: Vec::new(),
            print_job_max_age_secs: 4 * 60 * 60,
            scheduler_tick_secs: 30,
            host_timezone: None,
        }
    }
}

impl Config {
    /// Missing file means defaults. A present but broken file is an error,
    /// silently ignoring a typo in a config path is how data ends up in the
    /// wrong place.
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn host_tz(&self) -> chrono_tz::Tz {
        self.host_timezone
            .clone()
            .or_else(|| std::env::var("TZ").ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(chrono_tz::UTC)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_partial_files() {
        let c: Config = toml::from_str("group = \"kanban\"\nalways_admin_uids = [1000]\n").unwrap();
        assert_eq!(c.group, "kanban");
        assert_eq!(c.always_admin_uids, vec![1000]);
        assert_eq!(c.print_job_max_age_secs, 14400);
        assert!(
            toml::from_str::<Config>("grup = 1").is_err(),
            "typos are errors"
        );
    }
}
