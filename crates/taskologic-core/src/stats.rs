//! Time tracking: what a task's event trail says about how long it took.
//!
//! Nothing here reads a clock or a database. The daemon hands over the column
//! changes it has on file and gets back the numbers the analytics table
//! shows, which is what makes every rule below testable in isolation.
//!
//! Two numbers, deliberately different:
//!
//! - **Time taken** is time spent in the board's started column, pauses left
//!   out. It is the honest answer to "how long is this job", and it is what
//!   the averages and the template estimates are built on.
//! - **Wall clock** is first start to finish with everything in between. It
//!   is the answer to "how long did this drag on", which is a different
//!   question, so the detail view shows it and the table does not.
//!
//! A task that went straight to finished without ever being started has no
//! time taken at all. Calling that zero would quietly drag every average it
//! belongs to towards nothing.

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::ColumnId;

/// One column change from a task's history, in the order it happened.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    pub at: DateTime<Utc>,
    /// Where the task landed. None means it left the board's columns
    /// altogether, which is what archiving is.
    pub to: Option<ColumnId>,
}

/// What one task's trail adds up to.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timings {
    /// The first time it entered the started column. None if it never did.
    pub started_at: Option<DateTime<Utc>>,
    /// When it last entered the finished column, if it is still there.
    pub finished_at: Option<DateTime<Utc>>,
    /// Time in the started column, pauses excluded. None if it never
    /// started; still counting if it is started right now.
    pub time_taken: Option<TimeDelta>,
    /// First start to finish, pauses included. Runs to now while unfinished.
    pub wall_clock: Option<TimeDelta>,
}

impl Timings {
    /// True while the task sits in the started column, which is what makes
    /// `time_taken` a number that is still growing.
    pub fn running(&self) -> bool {
        self.started_at.is_some() && self.finished_at.is_none()
    }
}

/// Add up one task's trail. `trail` must be ordered oldest first; `now` ends
/// any interval the task is still inside.
pub fn timings(
    trail: &[Transition],
    started_col: ColumnId,
    finished_col: ColumnId,
    now: DateTime<Utc>,
) -> Timings {
    let mut out = Timings::default();
    let mut total = TimeDelta::zero();
    let mut running_since: Option<DateTime<Utc>> = None;

    for t in trail {
        let entering_started = t.to == Some(started_col);
        // Leaving the started column banks whatever was accrued in it.
        if let Some(since) = running_since
            && !entering_started
        {
            total += t.at - since;
            running_since = None;
        }
        if entering_started && running_since.is_none() {
            running_since = Some(t.at);
            out.started_at.get_or_insert(t.at);
        }
        // Finishing is not final: a task moved back out is unfinished again,
        // and the clock it stopped starts over.
        out.finished_at = (t.to == Some(finished_col)).then_some(t.at);
    }

    if let Some(since) = running_since {
        total += now.max(since) - since;
    }
    if out.started_at.is_some() {
        out.time_taken = Some(total);
    }
    if let Some(start) = out.started_at {
        let end = out.finished_at.unwrap_or(now);
        out.wall_clock = Some((end - start).max(TimeDelta::zero()));
    }
    out
}

/// The mean of a set of durations and how many it is over. None when there
/// is nothing to average, so a caller cannot mistake "no history" for zero.
pub fn average(samples: &[TimeDelta]) -> Option<(TimeDelta, u32)> {
    let n = i32::try_from(samples.len()).ok().filter(|n| *n > 0)?;
    let total = samples
        .iter()
        .try_fold(TimeDelta::zero(), |acc, d| acc.checked_add(d))?;
    Some((total / n, n as u32))
}

/// How long something took, for a table cell: `3d 04h`, `2h 15m`, `45m 00s`.
/// Two units at a time and never wider than seven characters, because the
/// column is narrow and the reader wants the size of the number rather than
/// its last second. A task parked in the started column for a fortnight
/// switches to days rather than reporting three hundred hours.
pub fn format_duration(d: TimeDelta) -> String {
    let secs = d.num_seconds().max(0);
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h >= 100 {
        let days = h / 24;
        // Past three digits of days the spare hours are noise, and the
        // column would rather have the space back.
        return if days >= 100 {
            format!("{days}d")
        } else {
            format!("{days}d {:02}h", h % 24)
        };
    }
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODO: ColumnId = ColumnId(1);
    const STARTED: ColumnId = ColumnId(2);
    const PAUSED: ColumnId = ColumnId(3);
    const DONE: ColumnId = ColumnId(4);

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + secs, 0).unwrap()
    }

    fn trail(steps: &[(i64, Option<ColumnId>)]) -> Vec<Transition> {
        steps.iter().map(|(s, to)| Transition { at: at(*s), to: *to }).collect()
    }

    #[test]
    fn a_task_that_was_never_started_has_no_time_taken() {
        // Straight from the to-do column to done, the way a two second job
        // gets closed. Zero would be a lie that drags every average down.
        let t = timings(&trail(&[(60, Some(DONE))]), STARTED, DONE, at(600));
        assert_eq!(t.started_at, None);
        assert_eq!(t.time_taken, None);
        assert_eq!(t.wall_clock, None);
        assert_eq!(t.finished_at, Some(at(60)));
        assert!(!t.running());
    }

    #[test]
    fn pauses_come_out_of_the_time_taken_but_not_the_wall_clock() {
        // Started, paused for an hour, restarted, finished.
        let t = timings(
            &trail(&[
                (0, Some(STARTED)),
                (600, Some(PAUSED)),
                (4_200, Some(STARTED)),
                (4_800, Some(DONE)),
            ]),
            STARTED,
            DONE,
            at(10_000),
        );
        assert_eq!(t.started_at, Some(at(0)));
        assert_eq!(t.finished_at, Some(at(4_800)));
        assert_eq!(
            t.time_taken,
            Some(TimeDelta::seconds(1_200)),
            "ten minutes before the pause and ten after"
        );
        assert_eq!(
            t.wall_clock,
            Some(TimeDelta::seconds(4_800)),
            "the hour off the boil still happened"
        );
        assert!(!t.running());
    }

    #[test]
    fn a_task_still_in_the_started_column_counts_up_to_now() {
        let t = timings(&trail(&[(0, Some(STARTED))]), STARTED, DONE, at(900));
        assert_eq!(t.time_taken, Some(TimeDelta::seconds(900)));
        assert_eq!(t.wall_clock, Some(TimeDelta::seconds(900)));
        assert!(t.running());

        // A clock that somehow reads earlier than the last move does not run
        // backwards into a negative duration.
        let backwards = timings(&trail(&[(900, Some(STARTED))]), STARTED, DONE, at(0));
        assert_eq!(backwards.time_taken, Some(TimeDelta::zero()));
    }

    #[test]
    fn unfinishing_a_task_restarts_its_clock() {
        // Finished, pulled back out, worked on again, finished again.
        let t = timings(
            &trail(&[
                (0, Some(STARTED)),
                (600, Some(DONE)),
                (1_200, Some(STARTED)),
                (1_800, Some(DONE)),
            ]),
            STARTED,
            DONE,
            at(5_000),
        );
        assert_eq!(t.time_taken, Some(TimeDelta::seconds(1_200)));
        assert_eq!(t.finished_at, Some(at(1_800)), "the finish that stuck");

        // Reopened and left open: not finished, and the wall clock runs on.
        let open = timings(
            &trail(&[(0, Some(STARTED)), (600, Some(DONE)), (1_200, Some(TODO))]),
            STARTED,
            DONE,
            at(5_000),
        );
        assert_eq!(open.finished_at, None);
        assert_eq!(open.time_taken, Some(TimeDelta::seconds(600)));
        assert_eq!(open.wall_clock, Some(TimeDelta::seconds(5_000)));
    }

    #[test]
    fn archiving_out_of_the_started_column_stops_the_clock() {
        // Archived straight out of started: the trail leaves every column.
        let t = timings(
            &trail(&[(0, Some(STARTED)), (600, None)]),
            STARTED,
            DONE,
            at(9_000),
        );
        assert_eq!(
            t.time_taken,
            Some(TimeDelta::seconds(600)),
            "not still running in the archive"
        );
    }

    #[test]
    fn a_move_within_the_started_column_does_not_restart_anything() {
        // Reordering inside a column records a move to the same place.
        let t = timings(
            &trail(&[(0, Some(STARTED)), (300, Some(STARTED)), (600, Some(DONE))]),
            STARTED,
            DONE,
            at(9_000),
        );
        assert_eq!(t.started_at, Some(at(0)));
        assert_eq!(t.time_taken, Some(TimeDelta::seconds(600)));
    }

    #[test]
    fn averages_say_how_many_they_are_over() {
        assert_eq!(average(&[]), None);
        assert_eq!(
            average(&[TimeDelta::minutes(10), TimeDelta::minutes(20)]),
            Some((TimeDelta::minutes(15), 2))
        );
        assert_eq!(
            average(&[TimeDelta::minutes(10)]),
            Some((TimeDelta::minutes(10), 1))
        );
    }

    #[test]
    fn durations_read_at_a_glance() {
        assert_eq!(format_duration(TimeDelta::seconds(0)), "0s");
        assert_eq!(format_duration(TimeDelta::seconds(45)), "45s");
        assert_eq!(format_duration(TimeDelta::seconds(90)), "1m 30s");
        assert_eq!(format_duration(TimeDelta::minutes(45)), "45m 00s");
        assert_eq!(format_duration(TimeDelta::minutes(135)), "2h 15m");
        assert_eq!(format_duration(TimeDelta::hours(30)), "30h 00m");
        assert_eq!(format_duration(TimeDelta::hours(99)), "99h 00m");
        // Past four days the hours stop being readable and it switches unit.
        assert_eq!(format_duration(TimeDelta::hours(100)), "4d 04h");
        assert_eq!(format_duration(TimeDelta::days(104)), "104d");
        assert_eq!(format_duration(TimeDelta::seconds(-5)), "0s");
        for d in [0, 45, 90, 3_600, 86_400, 900_000, 9_000_000] {
            let text = format_duration(TimeDelta::seconds(d));
            assert!(text.len() <= 7, "{d} rendered as {text}");
        }
    }
}
