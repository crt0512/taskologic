//! Control codes on the client side: what a `--1...--` frame does once the
//! detector has handed its body over. The grammar lives in
//! `taskologic_core::control`; this is the state between scans and the
//! turning of values into text.

use chrono::{DateTime, Datelike, Months, TimeDelta, Utc};
use chrono_tz::Tz;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use taskologic_core::control::{Control, NamedKey, Unit, Value};

/// A command that arrived without the value it needs, waiting for one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Waiting {
    Insert,
    Replace,
    Search { archive: bool },
}

impl Waiting {
    pub fn label(&self) -> &'static str {
        match self {
            Waiting::Insert => "insert",
            Waiting::Replace => "replace",
            Waiting::Search { archive: false } => "search",
            Waiting::Search { archive: true } => "search the archive too",
        }
    }
}

/// A "next scanned task" command waiting for its task: the next system
/// code scan, or the selected element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Armed {
    pub command: Control,
    pub values: Vec<Value>,
    /// Stays armed after each use, until Esc or the timeout.
    pub sticky: bool,
}

impl Armed {
    pub fn label(&self) -> String {
        let mut s = self.command.label();
        if !self.values.is_empty() {
            s.push_str(": ");
            s.push_str(&self.values.iter().map(Value::label).collect::<Vec<_>>().join(", "));
        }
        s
    }
}

/// What the client remembers between control scans.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControlState {
    pub waiting: Option<Waiting>,
    pub armed: Option<Armed>,
    /// When the state was last set or used, for the timeout.
    pub since_ms: u64,
}

impl ControlState {
    pub fn wait_for(&mut self, w: Waiting, now_ms: u64) {
        self.waiting = Some(w);
        self.since_ms = now_ms;
    }

    pub fn arm(&mut self, a: Armed, now_ms: u64) {
        self.armed = Some(a);
        self.since_ms = now_ms;
    }

    /// Whether anything is armed or waiting.
    pub fn busy(&self) -> bool {
        self.waiting.is_some() || self.armed.is_some()
    }

    /// Drop everything; what it was, for a toast.
    pub fn clear(&mut self) -> Option<String> {
        let w = self.waiting.take().map(|w| w.label().to_string());
        let a = self.armed.take().map(|a| a.label());
        w.or(a)
    }

    pub fn take_waiting(&mut self) -> Option<Waiting> {
        self.waiting.take()
    }

    /// The armed command for one use: gone afterwards unless sticky, in
    /// which case its clock starts again.
    pub fn use_armed(&mut self, now_ms: u64) -> Option<Armed> {
        let a = self.armed.clone()?;
        if a.sticky {
            self.since_ms = now_ms;
        } else {
            self.armed = None;
        }
        Some(a)
    }

    /// Whether the state has gone on longer than `timeout_ms`; 0 never.
    pub fn stale(&self, now_ms: u64, timeout_ms: u64) -> bool {
        self.busy() && timeout_ms > 0 && now_ms.saturating_sub(self.since_ms) > timeout_ms
    }

    pub fn status(&self) -> Option<String> {
        if let Some(a) = &self.armed {
            return Some(if a.sticky {
                format!("every scan: {} (Esc ends)", a.label())
            } else {
                format!("next scan: {} (Esc cancels)", a.label())
            });
        }
        self.waiting
            .as_ref()
            .map(|w| format!("waiting for a value to {} (Esc cancels)", w.label()))
    }
}

/// A date from a run of values, counted from what the field holds when it
/// holds anything: now, today keeping the field's time, this time keeping
/// the field's date, plus and minus, clear. Text is not a date.
pub fn values_date(
    values: &[Value],
    current: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    tz: Tz,
) -> Result<Option<DateTime<Utc>>, String> {
    let mut cur = current;
    for v in values {
        cur = match v {
            Value::Now => Some(now),
            Value::Today => {
                let today = now.with_timezone(&tz);
                let time = cur.map(|c| c.with_timezone(&tz).time()).unwrap_or(today.time());
                Some(local_to_utc(tz, today.date_naive().and_time(time), now))
            }
            Value::Time => {
                let local_now = now.with_timezone(&tz);
                let date = cur.map(|c| c.with_timezone(&tz).date_naive()).unwrap_or(local_now.date_naive());
                Some(local_to_utc(tz, date.and_time(local_now.time()), now))
            }
            Value::Plus(n, u) | Value::Minus(n, u) => {
                let base = cur.unwrap_or(now).with_timezone(&tz);
                Some(shift(base, *n, *u, matches!(v, Value::Minus(..))).with_timezone(&Utc))
            }
            Value::Clear => None,
            Value::Me | Value::Text(_) => return Err("that is text, not a date".into()),
            Value::Split => return Err("only start and due together take a second value".into()),
        };
    }
    Ok(cur)
}

fn local_to_utc(tz: Tz, naive: chrono::NaiveDateTime, fallback: DateTime<Utc>) -> DateTime<Utc> {
    use chrono::TimeZone;
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(t) => t.with_timezone(&Utc),
        chrono::LocalResult::Ambiguous(a, _) => a.with_timezone(&Utc),
        chrono::LocalResult::None => fallback,
    }
}

/// Minutes from values, for a reminder lead time: a plain number is
/// minutes, an offset counts, clear is none.
pub fn values_minutes(values: &[Value]) -> Result<Option<u32>, String> {
    let mut out = None;
    for v in values {
        out = match v {
            Value::Clear => None,
            Value::Text(t) => Some(t.trim().parse().map_err(|_| format!("{t:?} is not a number of minutes"))?),
            Value::Plus(n, u) => Some(match u {
                Unit::Minutes => *n,
                Unit::Hours => n * 60,
                Unit::Days => n * 60 * 24,
                Unit::Weeks => n * 60 * 24 * 7,
                Unit::Months => n * 60 * 24 * 30,
                Unit::Years => n * 60 * 24 * 365,
            }),
            _ => return Err("a reminder wants a number of minutes or an offset".into()),
        };
    }
    Ok(out)
}

/// The values for start and for due when both are set in one code: what
/// comes before `Split` is the start's, what follows is the due's. With no
/// split the same values go to both, and a side left empty is left alone.
pub fn split_values(values: &[Value]) -> (Option<&[Value]>, Option<&[Value]>) {
    match values.iter().position(|v| *v == Value::Split) {
        None => (Some(values), Some(values)),
        Some(i) => {
            let (a, b) = (&values[..i], &values[i + 1..]);
            ((!a.is_empty()).then_some(a), (!b.is_empty()).then_some(b))
        }
    }
}

/// The key event a named key stands for.
pub fn named_key_event(k: NamedKey) -> KeyEvent {
    let (code, m) = match k {
        NamedKey::Up => (KeyCode::Up, KeyModifiers::NONE),
        NamedKey::Down => (KeyCode::Down, KeyModifiers::NONE),
        NamedKey::Left => (KeyCode::Left, KeyModifiers::NONE),
        NamedKey::Right => (KeyCode::Right, KeyModifiers::NONE),
        NamedKey::Esc => (KeyCode::Esc, KeyModifiers::NONE),
        NamedKey::Enter => (KeyCode::Enter, KeyModifiers::NONE),
        NamedKey::Tab => (KeyCode::Tab, KeyModifiers::NONE),
        NamedKey::BackTab => (KeyCode::BackTab, KeyModifiers::SHIFT),
        NamedKey::F(n) => (KeyCode::F(n), KeyModifiers::NONE),
    };
    KeyEvent::new(code, m)
}

/// What a run of values types: a date and time in the form the task form
/// takes, then any text. Offsets count from now, since a scan cannot see
/// what a field already holds.
pub fn values_text(values: &[Value], now: DateTime<Utc>, tz: Tz, username: &str) -> String {
    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        DateTime,
        Date,
        Time,
    }
    let mut when: Option<(DateTime<Tz>, Kind)> = None;
    let mut text = String::new();
    let local = now.with_timezone(&tz);
    for v in values {
        match v {
            Value::Now => when = Some((local, Kind::DateTime)),
            Value::Today => when = Some((local, Kind::Date)),
            Value::Time => when = Some((local, Kind::Time)),
            Value::Plus(n, u) | Value::Minus(n, u) => {
                let (t, k) = when.unwrap_or((local, Kind::DateTime));
                let back = matches!(v, Value::Minus(..));
                when = Some((shift(t, *n, *u, back), k));
            }
            Value::Me => text.push_str(username),
            Value::Clear => text.clear(),
            Value::Split => {}
            Value::Text(s) => text.push_str(s),
        }
    }
    let mut out = String::new();
    if let Some((t, k)) = when {
        out = match k {
            Kind::DateTime => t.format("%Y-%m-%d %H:%M").to_string(),
            Kind::Date => t.format("%Y-%m-%d").to_string(),
            Kind::Time => t.format("%H:%M").to_string(),
        };
    }
    out.push_str(&text);
    out
}

fn shift(t: DateTime<Tz>, n: u32, unit: Unit, back: bool) -> DateTime<Tz> {
    let n64 = i64::from(n);
    let moved = match unit {
        Unit::Minutes => Some(t + TimeDelta::minutes(if back { -n64 } else { n64 })),
        Unit::Hours => Some(t + TimeDelta::hours(if back { -n64 } else { n64 })),
        Unit::Days => Some(t + TimeDelta::days(if back { -n64 } else { n64 })),
        Unit::Weeks => Some(t + TimeDelta::weeks(if back { -n64 } else { n64 })),
        Unit::Months => {
            let m = Months::new(n);
            if back { t.checked_sub_months(m) } else { t.checked_add_months(m) }
        }
        Unit::Years => {
            let target = if back { t.year() - n as i32 } else { t.year() + n as i32 };
            t.with_year(target).or_else(|| {
                // February 29th in a year without one: the day before.
                (t - TimeDelta::days(1)).with_year(target)
            })
        }
    };
    moved.unwrap_or(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn values_type_what_the_forms_take() {
        let tz: Tz = "Europe/Zurich".parse().unwrap();
        let now = at("2026-03-10T21:30:00Z"); // 22:30 in Zurich
        let t = |vals: &[Value]| values_text(vals, now, tz, "alice");
        assert_eq!(t(&[Value::Now]), "2026-03-10 22:30");
        assert_eq!(t(&[Value::Today]), "2026-03-10");
        assert_eq!(t(&[Value::Time]), "22:30");
        assert_eq!(t(&[Value::Now, Value::Plus(2, Unit::Hours)]), "2026-03-11 00:30");
        assert_eq!(t(&[Value::Today, Value::Plus(1, Unit::Days)]), "2026-03-11");
        assert_eq!(t(&[Value::Plus(30, Unit::Minutes)]), "2026-03-10 23:00", "an offset alone counts from now");
        assert_eq!(t(&[Value::Now, Value::Minus(1, Unit::Months)]), "2026-02-10 22:30");
        assert_eq!(t(&[Value::Now, Value::Plus(1, Unit::Years)]), "2027-03-10 22:30");
        assert_eq!(t(&[Value::Me]), "alice");
        assert_eq!(t(&[Value::Text("hi there".into())]), "hi there");
        assert_eq!(t(&[Value::Text("x".into()), Value::Clear, Value::Me]), "alice");
    }

    #[test]
    fn dates_from_values_count_from_what_the_field_holds() {
        let tz: Tz = "Europe/Zurich".parse().unwrap();
        let now = at("2026-03-10T21:30:00Z"); // 22:30 in Zurich
        let due = Some(at("2026-03-01T08:00:00Z")); // 09:00 in Zurich
        let d = |vals: &[Value], cur| values_date(vals, cur, now, tz).unwrap();
        assert_eq!(d(&[Value::Now], due), Some(now));
        assert_eq!(d(&[Value::Plus(2, Unit::Hours)], due), Some(at("2026-03-01T10:00:00Z")), "from the field's value");
        assert_eq!(d(&[Value::Plus(2, Unit::Hours)], None), Some(at("2026-03-10T23:30:00Z")), "from now when empty");
        assert_eq!(d(&[Value::Today], due), Some(at("2026-03-10T08:00:00Z")), "today, the field's 09:00 kept");
        assert_eq!(d(&[Value::Time], due), Some(at("2026-03-01T21:30:00Z")), "the field's day at this time");
        assert_eq!(d(&[Value::Clear], due), None);
        assert_eq!(d(&[Value::Now, Value::Plus(1, Unit::Days)], None), Some(at("2026-03-11T21:30:00Z")));
        assert!(values_date(&[Value::Me], None, now, tz).is_err());
        assert_eq!(values_minutes(&[Value::Plus(2, Unit::Hours)]).unwrap(), Some(120));
        assert_eq!(values_minutes(&[Value::Text("45".into())]).unwrap(), Some(45));
        assert_eq!(values_minutes(&[Value::Clear]).unwrap(), None);
    }

    #[test]
    fn an_armed_command_is_one_shot_unless_sticky() {
        let mut s = ControlState::default();
        s.arm(Armed { command: taskologic_core::control::Control::MoveLeft, values: vec![], sticky: false }, 0);
        assert_eq!(s.status().as_deref(), Some("next scan: move left (Esc cancels)"));
        assert!(s.use_armed(5).is_some());
        assert!(s.armed.is_none());
        s.arm(Armed { command: taskologic_core::control::Control::MoveLeft, values: vec![], sticky: true }, 0);
        assert!(s.use_armed(5).is_some());
        assert!(s.armed.is_some(), "sticky stays");
        assert_eq!(s.since_ms, 5, "and its clock restarts");
        assert_eq!(s.clear().as_deref(), Some("move left"));
    }

    #[test]
    fn a_wait_goes_stale_on_the_clock_unless_the_clock_is_off() {
        let mut s = ControlState::default();
        assert!(!s.stale(10_000, 5_000), "nothing waiting");
        s.wait_for(Waiting::Insert, 1_000);
        assert!(!s.stale(5_000, 5_000));
        assert!(s.stale(6_001, 5_000));
        assert!(!s.stale(1_000_000, 0), "0 is forever");
        assert_eq!(s.clear().as_deref(), Some("insert"));
        assert_eq!(s.clear(), None);
    }
}
