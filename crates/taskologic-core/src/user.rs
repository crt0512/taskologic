//! A Taskologic user is a system user in the `taskologic` group. This is the app
//! level record that goes with them.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::ids::Uid;
use crate::prefs::UserPrefs;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub uid: Uid,
    /// Display snapshot, refreshed on every login. The uid is the key.
    pub username: String,
    /// App level admin, not unix root. First user ever to log in gets it.
    pub is_admin: bool,
    /// Timestamps are stored in UTC and rendered in this zone.
    pub timezone: Tz,
    pub prefs: UserPrefs,
    /// Whether a kiosk PIN is set. The hash itself never leaves the daemon.
    pub has_pin: bool,
    pub created_at: DateTime<Utc>,
}

/// Just enough of a user to fill an assignee picker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserSummary {
    pub uid: Uid,
    pub username: String,
    pub is_admin: bool,
}

impl From<&User> for UserSummary {
    fn from(u: &User) -> Self {
        UserSummary {
            uid: u.uid,
            username: u.username.clone(),
            is_admin: u.is_admin,
        }
    }
}
