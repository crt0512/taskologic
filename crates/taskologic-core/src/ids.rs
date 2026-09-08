//! Identifier types. Database ids are i64 newtypes so they cannot be mixed up.
//! Short ids are the six character base36 codes printed inside barcodes.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Unix uid. Users are keyed by uid rather than username because usernames
/// get recycled, and a recycled name must not inherit someone else's tasks.
pub type Uid = u32;

macro_rules! db_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub i64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<i64> for $name {
            fn from(v: i64) -> Self {
                Self(v)
            }
        }
    };
}

db_id!(
    /// Row id of a board.
    BoardId
);
db_id!(
    /// Row id of a column.
    ColumnId
);
db_id!(
    /// Row id of a task. Never printed on paper, use [`ShortId`] for that.
    TaskId
);
db_id!(
    /// Row id of a task template.
    TemplateId
);
db_id!(
    /// Row id of an event log entry.
    EventId
);
db_id!(
    /// Row id of a queued print job.
    PrintJobId
);

/// The base36 alphabet, uppercase, in value order.
pub const BASE36_ALPHABET: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// Value of a base36 digit. Lowercase is accepted because some scanners are
/// configured to emit it.
pub fn base36_value(c: char) -> Option<u32> {
    match c {
        '0'..='9' => Some(c as u32 - '0' as u32),
        'A'..='Z' => Some(c as u32 - 'A' as u32 + 10),
        'a'..='z' => Some(c as u32 - 'a' as u32 + 10),
        _ => None,
    }
}

/// Uppercase base36 digit for a value below 36.
pub fn base36_char(v: u32) -> Option<char> {
    BASE36_ALPHABET.get(v as usize).map(|b| *b as char)
}

/// Six character base36 task id.
///
/// Short rather than a uuid so a CODE39 barcode of it still fits on 58mm
/// paper. Always stored and compared uppercase.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShortId([u8; ShortId::LEN]);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShortIdError {
    #[error("short id must be {len} characters, got {0}", len = ShortId::LEN)]
    WrongLength(usize),
    #[error("short id may only contain 0-9 and A-Z, found {0:?}")]
    BadChar(char),
}

impl ShortId {
    /// Length in characters. Changing this changes the barcode payload
    /// version, see [`crate::barcode`].
    pub const LEN: usize = 6;
    /// Number of distinct short ids.
    pub const SPACE: u64 = 36u64.pow(ShortId::LEN as u32);

    /// Build a short id from a number. The number is reduced modulo
    /// [`ShortId::SPACE`], so any u64 is accepted. Randomness is the caller's
    /// business, this crate has no IO.
    pub fn from_index(n: u64) -> Self {
        let mut n = n % Self::SPACE;
        let mut out = [b'0'; Self::LEN];
        for slot in out.iter_mut().rev() {
            *slot = BASE36_ALPHABET[(n % 36) as usize];
            n /= 36;
        }
        Self(out)
    }

    /// Inverse of [`ShortId::from_index`].
    pub fn index(self) -> u64 {
        self.0.iter().fold(0u64, |acc, b| {
            acc * 36 + u64::from(base36_value(*b as char).unwrap_or(0))
        })
    }

    /// Parse and normalise to uppercase.
    pub fn parse(s: &str) -> Result<Self, ShortIdError> {
        let mut out = [0u8; Self::LEN];
        let mut n = 0;
        for c in s.chars() {
            if n == Self::LEN {
                return Err(ShortIdError::WrongLength(s.chars().count()));
            }
            let v = base36_value(c).ok_or(ShortIdError::BadChar(c))?;
            out[n] = BASE36_ALPHABET[v as usize];
            n += 1;
        }
        if n != Self::LEN {
            return Err(ShortIdError::WrongLength(n));
        }
        Ok(Self(out))
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("short ids are ascii by construction")
    }
}

impl fmt::Display for ShortId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for ShortId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ShortId({})", self.as_str())
    }
}

impl FromStr for ShortId {
    type Err = ShortIdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for ShortId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ShortId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_round_trips() {
        for n in [
            0u64,
            1,
            35,
            36,
            12345,
            ShortId::SPACE - 1,
            ShortId::SPACE,
            u64::MAX,
        ] {
            let id = ShortId::from_index(n);
            assert_eq!(id.index(), n % ShortId::SPACE, "{n}");
            assert_eq!(ShortId::parse(id.as_str()).unwrap(), id);
        }
        assert_eq!(ShortId::from_index(0).as_str(), "000000");
        assert_eq!(ShortId::from_index(35).as_str(), "00000Z");
        assert_eq!(ShortId::from_index(36).as_str(), "000010");
    }

    #[test]
    fn parse_normalises_case_and_rejects_junk() {
        assert_eq!(ShortId::parse("k4m9q2").unwrap().as_str(), "K4M9Q2");
        assert_eq!(
            ShortId::parse("K4M9Q").unwrap_err(),
            ShortIdError::WrongLength(5)
        );
        assert_eq!(
            ShortId::parse("K4M9Q2X").unwrap_err(),
            ShortIdError::WrongLength(7)
        );
        assert_eq!(
            ShortId::parse("K4M9Q.").unwrap_err(),
            ShortIdError::BadChar('.')
        );
    }

    #[test]
    fn serde_is_a_plain_string() {
        let id = ShortId::parse("K4M9Q2").unwrap();
        let json = serde_json_lite(&id);
        assert_eq!(json, "\"K4M9Q2\"");
    }

    // Tiny stand in so the crate does not need serde_json as a dev dependency
    // just for one assertion.
    fn serde_json_lite(id: &ShortId) -> String {
        format!("\"{}\"", id.as_str())
    }
}
