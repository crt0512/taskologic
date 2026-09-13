//! "So many minutes, hours or days after something."
//!
//! Templates prefill a start and a due date this way, and a repetition works
//! out what its next copy is dated the same way. One type, so the form widget
//! and the validation are written once.

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

/// The unit an [`Offset`] is counted in.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OffsetUnit {
    Minutes,
    #[default]
    Hours,
    Days,
}

impl OffsetUnit {
    pub const ALL: [OffsetUnit; 3] = [OffsetUnit::Minutes, OffsetUnit::Hours, OffsetUnit::Days];

    pub fn label(self) -> &'static str {
        match self {
            OffsetUnit::Minutes => "minutes",
            OffsetUnit::Hours => "hours",
            OffsetUnit::Days => "days",
        }
    }

    pub fn minutes(self) -> i64 {
        match self {
            OffsetUnit::Minutes => 1,
            OffsetUnit::Hours => 60,
            OffsetUnit::Days => 24 * 60,
        }
    }
}

/// How far ahead of some moment a date lands. An amount of zero means
/// exactly that moment.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Offset {
    pub amount: u32,
    pub unit: OffsetUnit,
}

impl Offset {
    /// The date this offset produces, or None if it overflows.
    pub fn after(self, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
        from.checked_add_signed(self.delta()?)
    }

    /// The offset as a duration, or None if it does not fit one.
    pub fn delta(self) -> Option<TimeDelta> {
        TimeDelta::try_minutes(i64::from(self.amount).checked_mul(self.unit.minutes())?)
    }
}

/// The largest offset anything may carry, so a typo cannot push a date out
/// past what chrono can represent.
pub const MAX_OFFSET_AMOUNT: u32 = 100_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_offset_counts_forward_in_its_own_unit() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let at = |amount, unit| Offset { amount, unit }.after(now).map(|d| d.timestamp());
        assert_eq!(at(0, OffsetUnit::Hours), Some(now.timestamp()));
        assert_eq!(at(90, OffsetUnit::Minutes), Some(now.timestamp() + 5_400));
        assert_eq!(at(2, OffsetUnit::Hours), Some(now.timestamp() + 7_200));
        assert_eq!(at(3, OffsetUnit::Days), Some(now.timestamp() + 259_200));
        assert_eq!(at(u32::MAX, OffsetUnit::Days), None, "past what chrono has");
    }

    #[test]
    fn the_largest_allowed_offset_still_lands_on_a_real_date() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let far = Offset {
            amount: MAX_OFFSET_AMOUNT,
            unit: OffsetUnit::Days,
        };
        assert!(far.after(now).is_some());
    }
}
