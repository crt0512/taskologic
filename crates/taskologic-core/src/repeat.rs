//! Repeating tasks.
//!
//! A repetition is a rule plus a local time of day in the creator's zone,
//! because "every Monday at 9" means 9 where the person lives, not 9 UTC.
//! When the schedule fires the daemon checks whether the current instance is
//! done. If it is, a fresh copy is created in the board's first column. If it
//! is not, nothing happens and the next occurrence is tried instead.

use chrono::{
    DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, TimeZone, Utc, Weekday,
};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::board::Board;
use crate::offset::Offset;
use crate::task::{Task, TaskDraft};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Repeat {
    /// Every N days counted from `from`.
    EveryDays {
        every: u32,
        from: NaiveDate,
    },
    Weekdays {
        days: Vec<Weekday>,
    },
    /// Clamped to the last day of shorter months, so 31 means "month end".
    DayOfMonth {
        day: u32,
    },
    /// Fires once. Kept as a repetition so the same machinery applies.
    FixedDate {
        date: NaiveDate,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepeatSpec {
    pub rule: Repeat,
    /// Local time of day the copy is created.
    pub at: NaiveTime,
    pub tz: Tz,
    /// What the copy's start date is, counted forward from the moment the
    /// repetition fires. None leaves the copy without one.
    #[serde(default)]
    pub start_rule: Option<Offset>,
    /// The same for its due date. Repetitions written before 0.1.11 carry
    /// neither, which is what they did anyway: the copy arrived undated.
    #[serde(default)]
    pub due_rule: Option<Offset>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RepeatError {
    #[error("repeat interval must be at least one day")]
    ZeroInterval,
    #[error("pick at least one weekday")]
    NoWeekdays,
    #[error("day of month must be between 1 and 31")]
    BadDayOfMonth,
}

impl Repeat {
    pub fn validate(&self) -> Result<(), RepeatError> {
        match self {
            Repeat::EveryDays { every: 0, .. } => Err(RepeatError::ZeroInterval),
            Repeat::Weekdays { days } if days.is_empty() => Err(RepeatError::NoWeekdays),
            Repeat::DayOfMonth { day } if !(1..=31).contains(day) => {
                Err(RepeatError::BadDayOfMonth)
            }
            _ => Ok(()),
        }
    }

    fn matches(&self, date: NaiveDate) -> bool {
        match self {
            Repeat::EveryDays { every, from } => {
                date >= *from && (date - *from).num_days() % i64::from(*every) == 0
            }
            Repeat::Weekdays { days } => days.contains(&date.weekday()),
            Repeat::DayOfMonth { day } => date.day() == (*day).min(days_in_month(date)),
            Repeat::FixedDate { date: d } => date == *d,
        }
    }

    /// How far ahead a match is guaranteed to exist, in days.
    fn horizon_days(&self) -> u32 {
        match self {
            Repeat::EveryDays { every, .. } => every.saturating_add(1),
            Repeat::Weekdays { .. } => 8,
            Repeat::DayOfMonth { .. } => 32,
            Repeat::FixedDate { .. } => 1,
        }
    }
}

impl RepeatSpec {
    pub fn validate(&self) -> Result<(), RepeatError> {
        self.rule.validate()
    }

    /// One readable line for lists: "every 3 days at 09:00".
    pub fn summary(&self) -> String {
        let rule = match &self.rule {
            Repeat::EveryDays { every: 1, .. } => "every day".to_string(),
            Repeat::EveryDays { every, .. } => format!("every {every} days"),
            Repeat::Weekdays { days } => days
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            Repeat::DayOfMonth { day } => format!("day {day} of the month"),
            Repeat::FixedDate { date } => format!("once on {date}"),
        };
        format!("{rule} at {}", self.at.format("%H:%M"))
    }

    /// The first firing strictly after `after`, in UTC. None when the rule is
    /// exhausted (a fixed date in the past).
    pub fn next_fire_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let local = after.with_timezone(&self.tz).naive_local();
        let mut date = local.date();
        let upto = match &self.rule {
            Repeat::FixedDate { date: d } => {
                if *d < date {
                    return None;
                }
                date = *d;
                1
            }
            Repeat::EveryDays { from, .. } if *from > date => {
                date = *from;
                self.rule.horizon_days()
            }
            _ => self.rule.horizon_days(),
        };
        for _ in 0..upto {
            if self.rule.matches(date) {
                let naive = date.and_time(self.at);
                if naive > local {
                    return Some(local_to_utc(self.tz, naive));
                }
            }
            date = date.succ_opt()?;
        }
        None
    }
}

fn days_in_month(date: NaiveDate) -> u32 {
    let (y, m) = (date.year(), date.month());
    let next = if m == 12 {
        NaiveDate::from_ymd_opt(y + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(y, m + 1, 1)
    };
    next.and_then(|n| n.pred_opt())
        .map(|d| d.day())
        .unwrap_or(28)
}

fn local_to_utc(tz: Tz, naive: NaiveDateTime) -> DateTime<Utc> {
    use chrono::LocalResult;
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => dt.with_timezone(&Utc),
        // DST gap. The wall clock time does not exist, fire an hour later.
        LocalResult::None => tz
            .from_local_datetime(&(naive + TimeDelta::hours(1)))
            .earliest()
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|| Utc.from_utc_datetime(&naive)),
    }
}

/// The draft for the next instance of a repeating task. Title, description,
/// reminder overrides, assignees, checklist and repetition carry over.
/// Dependencies and the finished state do not. Assignees who left the board
/// are dropped, and the checklist starts unticked.
///
/// The start and due dates are worked out from `fired_at` by the repetition's
/// own rules, so a daily task can arrive dated for that day rather than
/// arriving blank the way it did before 0.1.11.
pub fn next_instance(task: &Task, board: &Board, fired_at: DateTime<Utc>) -> TaskDraft {
    let rules = task.repeat.as_ref();
    let start_at = rules.and_then(|r| r.start_rule).and_then(|o| o.after(fired_at));
    let due_at = rules.and_then(|r| r.due_rule).and_then(|o| o.after(fired_at));
    TaskDraft {
        title: task.title.clone(),
        description: task.description.clone(),
        // A rule that would date the copy as due before it starts is a
        // broken rule, not a broken task: the copy keeps its start date and
        // goes out undated rather than being refused at save time.
        start_at,
        due_at: due_at.filter(|d| start_at.is_none_or(|s| s <= *d)),
        reminder_start_minutes: task.reminder_start_minutes,
        reminder_due_minutes: task.reminder_due_minutes,
        assignees: task
            .assignees
            .iter()
            .copied()
            .filter(|u| board.is_member(*u))
            .collect(),
        depends_on: Vec::new(),
        checklist: task
            .checklist
            .iter()
            .map(|c| crate::task::ChecklistItem {
                text: c.text.clone(),
                done: false,
            })
            .collect(),
        repeat: task.repeat.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn spec(rule: Repeat) -> RepeatSpec {
        RepeatSpec {
            rule,
            at: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            tz: chrono_tz::Europe::Berlin,
            start_rule: None,
            due_rule: None,
        }
    }

    #[test]
    fn every_n_days_from_an_anchor() {
        let s = spec(Repeat::EveryDays {
            every: 3,
            from: NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
        });
        // Matches 1st, 4th, 7th. After the 5th noon local, next is the 7th 09:00 Berlin = 07:00Z.
        assert_eq!(
            s.next_fire_after(utc("2026-09-05T10:00:00Z")),
            Some(utc("2026-09-07T07:00:00Z"))
        );
        // Exactly at the fire time does not count, strictly after.
        assert_eq!(
            s.next_fire_after(utc("2026-09-07T07:00:00Z")),
            Some(utc("2026-09-10T07:00:00Z"))
        );
        // Before the anchor starts at the anchor.
        assert_eq!(
            s.next_fire_after(utc("2026-08-01T00:00:00Z")),
            Some(utc("2026-09-01T07:00:00Z"))
        );
    }

    #[test]
    fn summaries_read_like_sentences() {
        let from = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        assert_eq!(
            spec(Repeat::EveryDays { every: 1, from }).summary(),
            "every day at 09:00"
        );
        assert_eq!(
            spec(Repeat::EveryDays { every: 3, from }).summary(),
            "every 3 days at 09:00"
        );
        assert_eq!(
            spec(Repeat::Weekdays {
                days: vec![Weekday::Mon, Weekday::Fri]
            })
            .summary(),
            "Mon, Fri at 09:00"
        );
        assert_eq!(
            spec(Repeat::DayOfMonth { day: 15 }).summary(),
            "day 15 of the month at 09:00"
        );
        assert_eq!(
            spec(Repeat::FixedDate { date: from }).summary(),
            "once on 2026-09-01 at 09:00"
        );
    }

    #[test]
    fn weekdays() {
        let s = spec(Repeat::Weekdays {
            days: vec![Weekday::Mon, Weekday::Fri],
        });
        // 2026-09-05 is a Saturday.
        assert_eq!(
            s.next_fire_after(utc("2026-09-05T10:00:00Z")),
            Some(utc("2026-09-07T07:00:00Z"))
        );
        assert_eq!(
            s.next_fire_after(utc("2026-09-07T08:00:00Z")),
            Some(utc("2026-09-11T07:00:00Z"))
        );
    }

    #[test]
    fn day_of_month_clamps_short_months() {
        let s = spec(Repeat::DayOfMonth { day: 31 });
        assert_eq!(
            s.next_fire_after(utc("2026-02-01T00:00:00Z")),
            Some(utc("2026-02-28T08:00:00Z"))
        );
        assert_eq!(
            s.next_fire_after(utc("2026-02-28T09:00:00Z")),
            Some(utc("2026-03-31T07:00:00Z"))
        );
        assert_eq!(
            s.next_fire_after(utc("2028-02-01T00:00:00Z")),
            Some(utc("2028-02-29T08:00:00Z"))
        );
    }

    #[test]
    fn fixed_date_fires_once() {
        let s = spec(Repeat::FixedDate {
            date: NaiveDate::from_ymd_opt(2026, 12, 24).unwrap(),
        });
        assert_eq!(
            s.next_fire_after(utc("2026-09-05T00:00:00Z")),
            Some(utc("2026-12-24T08:00:00Z"))
        );
        assert_eq!(s.next_fire_after(utc("2026-12-24T08:00:00Z")), None);
        assert_eq!(s.next_fire_after(utc("2027-01-01T00:00:00Z")), None);
    }

    #[test]
    fn dst_gap_moves_forward() {
        // 2026-03-29 02:30 does not exist in Berlin.
        let s = RepeatSpec {
            rule: Repeat::FixedDate {
                date: NaiveDate::from_ymd_opt(2026, 3, 29).unwrap(),
            },
            at: NaiveTime::from_hms_opt(2, 30, 0).unwrap(),
            tz: chrono_tz::Europe::Berlin,
            start_rule: None,
            due_rule: None,
        };
        assert_eq!(
            s.next_fire_after(utc("2026-03-28T00:00:00Z")),
            Some(utc("2026-03-29T01:30:00Z"))
        );
    }

    #[test]
    fn validation() {
        assert_eq!(
            Repeat::EveryDays {
                every: 0,
                from: NaiveDate::MIN
            }
            .validate(),
            Err(RepeatError::ZeroInterval)
        );
        assert_eq!(
            Repeat::Weekdays { days: vec![] }.validate(),
            Err(RepeatError::NoWeekdays)
        );
        assert_eq!(
            Repeat::DayOfMonth { day: 0 }.validate(),
            Err(RepeatError::BadDayOfMonth)
        );
        assert_eq!(
            Repeat::DayOfMonth { day: 32 }.validate(),
            Err(RepeatError::BadDayOfMonth)
        );
        assert_eq!(Repeat::DayOfMonth { day: 31 }.validate(), Ok(()));
    }

    #[test]
    fn next_instance_drops_deps_and_departed_assignees() {
        use crate::board::test_support::board_with_members;
        use crate::task::test_support::task_on;
        let board = board_with_members(1, &[1, 2]);
        let mut task = task_on(&board, 1);
        task.assignees = vec![1, 2, 3];
        task.depends_on = vec![crate::ids::TaskId(5)];
        task.due_at = Some(utc("2026-09-05T00:00:00Z"));
        task.repeat = Some(spec(Repeat::DayOfMonth { day: 1 }));
        let draft = next_instance(&task, &board, utc("2026-10-01T06:00:00Z"));
        assert_eq!(draft.assignees, vec![1, 2]);
        assert!(draft.depends_on.is_empty());
        assert_eq!(draft.due_at, None, "no rule, so the copy arrives undated");
        assert_eq!(draft.start_at, None);
        assert_eq!(draft.repeat, task.repeat);
        assert_eq!(draft.title, task.title);
    }

    #[test]
    fn the_copy_is_dated_from_the_moment_the_repetition_fired() {
        use crate::board::test_support::board_with_members;
        use crate::offset::{Offset, OffsetUnit};
        use crate::task::test_support::task_on;
        let board = board_with_members(1, &[1]);
        let mut task = task_on(&board, 1);
        let fired = utc("2026-10-01T06:00:00Z");
        let mut s = spec(Repeat::DayOfMonth { day: 1 });
        s.start_rule = Some(Offset {
            amount: 2,
            unit: OffsetUnit::Hours,
        });
        s.due_rule = Some(Offset {
            amount: 1,
            unit: OffsetUnit::Days,
        });
        task.repeat = Some(s);

        let draft = next_instance(&task, &board, fired);
        assert_eq!(draft.start_at, Some(utc("2026-10-01T08:00:00Z")));
        assert_eq!(draft.due_at, Some(utc("2026-10-02T06:00:00Z")));

        // One rule without the other dates only that end of the task.
        let mut only_due = task.clone();
        only_due.repeat.as_mut().unwrap().start_rule = None;
        let draft = next_instance(&only_due, &board, fired);
        assert_eq!(draft.start_at, None);
        assert_eq!(draft.due_at, Some(utc("2026-10-02T06:00:00Z")));
    }

    #[test]
    fn a_rule_that_would_land_due_before_start_drops_the_due_date() {
        use crate::board::test_support::board_with_members;
        use crate::offset::{Offset, OffsetUnit};
        use crate::task::test_support::task_on;
        let board = board_with_members(1, &[1]);
        let mut task = task_on(&board, 1);
        let mut s = spec(Repeat::DayOfMonth { day: 1 });
        s.start_rule = Some(Offset {
            amount: 3,
            unit: OffsetUnit::Days,
        });
        s.due_rule = Some(Offset {
            amount: 1,
            unit: OffsetUnit::Hours,
        });
        task.repeat = Some(s);

        // The copy still arrives. Refusing it would mean a repetition that
        // silently stops the day somebody mistypes a unit.
        let draft = next_instance(&task, &board, utc("2026-10-01T06:00:00Z"));
        assert_eq!(draft.start_at, Some(utc("2026-10-04T06:00:00Z")));
        assert_eq!(draft.due_at, None);
        assert_eq!(
            crate::task::validate_draft(&draft, &board),
            Ok(()),
            "whatever it produces has to be savable"
        );
    }
}
