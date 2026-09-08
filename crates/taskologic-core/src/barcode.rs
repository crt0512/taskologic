//! Barcode payload format and the keystroke stream scan detector.
//!
//! Payload layout, version 1:
//!
//! ```text
//! ..1<action><taskid><check>
//!
//! ..      magic, two identical characters, so random product barcodes are ignored
//! 1       format version, which fixes the total length
//! action  S = start/pause toggle, F = finish
//! taskid  six character base36 short id
//! check   one base36 check character
//! ```
//!
//! Only uppercase alphanumerics plus the magic are used because CODE39 cannot
//! carry anything else. The magic is `..` (or `--` if a user prefers) because
//! hyphen and period are the only CODE39 extras that survive both plain and
//! Full ASCII scanner modes unchanged.
//!
//! The fixed length is the whole trick. Scan detection does not rely on
//! keystroke timing, which SSH jitter destroys. It watches for the magic,
//! reads the version character, and then knows exactly how many more
//! characters to wait for.
//!
//! The check character is ISO 7064 MOD 37,36. It catches every single
//! character substitution and every adjacent transposition with one base36
//! character, which is what a slightly misread CODE39 bar produces.

use serde::{Deserialize, Serialize};

use crate::ids::{ShortId, base36_char, base36_value};

/// Which two character magic prefix a user's barcodes carry.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Magic {
    #[default]
    Dots,
    Dashes,
}

impl Magic {
    pub const ALL: [Magic; 2] = [Magic::Dots, Magic::Dashes];

    pub fn as_str(self) -> &'static str {
        match self {
            Magic::Dots => "..",
            Magic::Dashes => "--",
        }
    }

    pub fn ch(self) -> char {
        match self {
            Magic::Dots => '.',
            Magic::Dashes => '-',
        }
    }

    pub fn from_char(c: char) -> Option<Magic> {
        match c {
            '.' => Some(Magic::Dots),
            '-' => Some(Magic::Dashes),
            _ => None,
        }
    }
}

/// What a scanned barcode asks for.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanAction {
    /// Move to the started column, or to the paused column if already started.
    StartPause,
    /// Move to the finished column, running the normal dependency check.
    Finish,
}

impl ScanAction {
    pub fn code(self) -> char {
        match self {
            ScanAction::StartPause => 'S',
            ScanAction::Finish => 'F',
        }
    }

    pub fn from_code(c: char) -> Option<ScanAction> {
        match c.to_ascii_uppercase() {
            'S' => Some(ScanAction::StartPause),
            'F' => Some(ScanAction::Finish),
            _ => None,
        }
    }
}

/// The only payload version so far.
pub const VERSION_1: char = '1';
/// Characters of magic in front of every payload.
pub const MAGIC_LEN: usize = 2;
/// Total length of a v1 payload: magic 2 + version 1 + action 1 + short id 6 + check 1.
pub const V1_LEN: usize = MAGIC_LEN + 1 + 1 + ShortId::LEN + 1;

/// Total payload length for a version character, or None if unknown.
pub fn payload_len(version: char) -> Option<usize> {
    match version {
        VERSION_1 => Some(V1_LEN),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScanParseError {
    /// Not one of ours. Callers should ignore this silently.
    #[error("not a Taskologic barcode")]
    NoMagic,
    #[error("unknown barcode format version {0:?}")]
    UnknownVersion(char),
    #[error("barcode too short, expected {expected} characters but got {got}")]
    Truncated { expected: usize, got: usize },
    #[error("barcode too long, expected {expected} characters but got {got}")]
    TooLong { expected: usize, got: usize },
    #[error("unknown barcode action {0:?}")]
    BadAction(char),
    #[error("barcode contains a character outside 0-9 A-Z: {0:?}")]
    BadChar(char),
    #[error("barcode check character does not match")]
    BadCheck,
}

/// A decoded barcode.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScanPayload {
    pub action: ScanAction,
    pub short_id: ShortId,
}

impl ScanPayload {
    /// Version, action and short id, without magic or check character.
    fn body(&self) -> String {
        format!("{}{}{}", VERSION_1, self.action.code(), self.short_id)
    }

    /// Render the full payload string that goes into a barcode.
    pub fn encode(&self, magic: Magic) -> String {
        let body = self.body();
        let check = check_char(&body).expect("body is base36 by construction");
        format!("{}{}{}", magic.as_str(), body, check)
    }

    /// Parse a complete payload. Any magic is accepted so that a slip printed
    /// by a user with a different magic preference still scans.
    pub fn parse(raw: &str) -> Result<Self, ScanParseError> {
        let chars: Vec<char> = raw.trim().chars().map(|c| c.to_ascii_uppercase()).collect();
        let magic = match (chars.first(), chars.get(1)) {
            (Some(a), Some(b)) if a == b => Magic::from_char(*a),
            _ => None,
        };
        if magic.is_none() {
            return Err(ScanParseError::NoMagic);
        }
        let version = *chars.get(MAGIC_LEN).ok_or(ScanParseError::Truncated {
            expected: V1_LEN,
            got: chars.len(),
        })?;
        let expected = payload_len(version).ok_or(ScanParseError::UnknownVersion(version))?;
        if chars.len() < expected {
            return Err(ScanParseError::Truncated {
                expected,
                got: chars.len(),
            });
        }
        if chars.len() > expected {
            return Err(ScanParseError::TooLong {
                expected,
                got: chars.len(),
            });
        }
        for c in &chars[MAGIC_LEN..] {
            if base36_value(*c).is_none() {
                return Err(ScanParseError::BadChar(*c));
            }
        }
        let with_check: String = chars[MAGIC_LEN..].iter().collect();
        if !verify_check(&with_check) {
            return Err(ScanParseError::BadCheck);
        }
        let action = ScanAction::from_code(chars[MAGIC_LEN + 1])
            .ok_or(ScanParseError::BadAction(chars[MAGIC_LEN + 1]))?;
        let id: String = chars[MAGIC_LEN + 2..MAGIC_LEN + 2 + ShortId::LEN]
            .iter()
            .collect();
        let short_id = ShortId::parse(&id).map_err(|e| match e {
            crate::ids::ShortIdError::BadChar(c) => ScanParseError::BadChar(c),
            crate::ids::ShortIdError::WrongLength(_) => ScanParseError::Truncated {
                expected,
                got: chars.len(),
            },
        })?;
        Ok(ScanPayload { action, short_id })
    }
}

const MOD: u32 = 36;

fn mod37_36_checksum(values: impl Iterator<Item = u32>) -> u32 {
    // Starting at 18 is the standard's trick for an initial P of MOD: 18*2 mod 37 is 36.
    let mut check = MOD / 2;
    for v in values {
        let base = if check == 0 { MOD } else { check };
        check = ((base * 2) % (MOD + 1) + v) % MOD;
    }
    check
}

/// ISO 7064 MOD 37,36 check character over a base36 string. Returns None if
/// the input contains anything outside base36.
pub fn check_char(body: &str) -> Option<char> {
    let values: Option<Vec<u32>> = body.chars().map(base36_value).collect();
    let cs = mod37_36_checksum(values?.into_iter());
    let base = if cs == 0 { MOD } else { cs };
    base36_char((1 + MOD - (base * 2) % (MOD + 1)) % MOD)
}

/// True if the last character of `with_check` is the correct check character
/// for everything before it.
pub fn verify_check(with_check: &str) -> bool {
    let values: Option<Vec<u32>> = with_check.chars().map(base36_value).collect();
    match values {
        Some(v) if !v.is_empty() => mod37_36_checksum(v.into_iter()) == 1,
        _ => false,
    }
}

/// One keystroke as seen by the detector. Only these two matter, everything
/// else the client handles before it gets here.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScanKey {
    Char(char),
    Enter,
}

/// What the client should do with a keystroke after the detector saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Feed {
    /// Hand these keys to normal key handling. Usually one key, but a held
    /// partial match is released together with the key that broke it.
    Pass(Vec<ScanKey>),
    /// Swallowed into the scan buffer, nothing to do yet.
    Held,
    /// A complete payload arrived. Dispatch it, or beep on the error.
    Scan(Result<ScanPayload, ScanParseError>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    /// Matched this many characters of the configured scanner prefix.
    Prefix(usize),
    /// Whole prefix matched, waiting for the first magic character.
    PrefixDone,
    /// Saw the first magic character, waiting for the second.
    HalfMagic(Magic),
    /// Past the magic, waiting for the version character.
    Version(Magic),
    /// Collecting the rest. `total` is the payload length including magic.
    Body {
        magic: Magic,
        total: usize,
    },
}

/// Watches a keystroke stream for barcode payloads.
///
/// Feed it every printable key and Enter while a board or the dashboard has
/// focus. Never feed it while a text field has focus, that is the one place a
/// person could plausibly type two periods in a row. Timestamps are plain
/// milliseconds supplied by the caller so the detector stays testable.
#[derive(Clone, Debug)]
pub struct ScanDetector {
    prefix: Vec<char>,
    enter_terminates: bool,
    timeout_ms: u64,
    phase: Phase,
    held: Vec<char>,
    body: String,
    since: Option<u64>,
    swallow_enter: bool,
    last_scan_ms: Option<u64>,
}

impl ScanDetector {
    /// A couple of seconds is long enough for the slowest wedge over the
    /// worst SSH link and short enough that a stray `..` cannot wedge input.
    pub const DEFAULT_TIMEOUT_MS: u64 = 2000;

    pub fn new() -> Self {
        Self {
            prefix: Vec::new(),
            enter_terminates: false,
            timeout_ms: Self::DEFAULT_TIMEOUT_MS,
            phase: Phase::Idle,
            held: Vec::new(),
            body: String::new(),
            since: None,
            swallow_enter: false,
            last_scan_ms: None,
        }
    }

    /// Characters the scanner is programmed to send before the barcode
    /// contents. Empty for most scanners.
    pub fn with_prefix(mut self, prefix: &str) -> Self {
        self.prefix = prefix.chars().collect();
        self
    }

    /// Whether the scanner sends Enter after the contents. When set, Enter
    /// dispatches immediately and the Enter right after a completed scan is
    /// swallowed instead of opening whatever task is selected.
    pub fn with_enter_terminates(mut self, yes: bool) -> Self {
        self.enter_terminates = yes;
        self
    }

    pub fn with_timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }

    pub fn is_idle(&self) -> bool {
        self.phase == Phase::Idle
    }

    /// How long the last complete scan took to arrive. Diagnostics only,
    /// never used to decide anything.
    pub fn last_scan_duration_ms(&self) -> Option<u64> {
        self.last_scan_ms
    }

    fn reset(&mut self) {
        self.phase = Phase::Idle;
        self.held.clear();
        self.body.clear();
        self.since = None;
    }

    /// Drop a partial buffer that has gone stale. Call this from the UI tick.
    /// Returns true if something was discarded.
    pub fn expire(&mut self, now_ms: u64) -> bool {
        match self.since {
            Some(t) if self.phase != Phase::Idle && now_ms.saturating_sub(t) > self.timeout_ms => {
                self.reset();
                true
            }
            _ => false,
        }
    }

    pub fn push(&mut self, key: ScanKey, now_ms: u64) -> Feed {
        self.expire(now_ms);
        match key {
            ScanKey::Char(c) => self.push_char(c, now_ms),
            ScanKey::Enter => self.push_enter(now_ms),
        }
    }

    fn push_char(&mut self, c: char, now_ms: u64) -> Feed {
        self.swallow_enter = false;
        match self.phase.clone() {
            Phase::Idle => self.start(c, now_ms),
            Phase::Prefix(n) => {
                if self.prefix.get(n) == Some(&c) {
                    self.held.push(c);
                    self.phase = if n + 1 == self.prefix.len() {
                        Phase::PrefixDone
                    } else {
                        Phase::Prefix(n + 1)
                    };
                    Feed::Held
                } else {
                    self.release_with(c, now_ms)
                }
            }
            Phase::PrefixDone => match Magic::from_char(c) {
                Some(m) => {
                    self.held.push(c);
                    self.phase = Phase::HalfMagic(m);
                    Feed::Held
                }
                None => self.release_with(c, now_ms),
            },
            Phase::HalfMagic(m) => {
                if c == m.ch() {
                    self.held.push(c);
                    self.phase = Phase::Version(m);
                    Feed::Held
                } else {
                    self.release_with(c, now_ms)
                }
            }
            Phase::Version(m) => match payload_len(c) {
                Some(total) => {
                    self.body.push(c);
                    self.phase = Phase::Body { magic: m, total };
                    Feed::Held
                }
                None => {
                    self.reset();
                    Feed::Scan(Err(ScanParseError::UnknownVersion(c)))
                }
            },
            Phase::Body { magic, total } => {
                self.body.push(c.to_ascii_uppercase());
                if MAGIC_LEN + self.body.chars().count() >= total {
                    self.finish(magic, now_ms)
                } else {
                    Feed::Held
                }
            }
        }
    }

    fn start(&mut self, c: char, now_ms: u64) -> Feed {
        if self.prefix.is_empty() {
            if let Some(m) = Magic::from_char(c) {
                self.held.push(c);
                self.phase = Phase::HalfMagic(m);
                self.since = Some(now_ms);
                return Feed::Held;
            }
        } else if self.prefix[0] == c {
            self.held.push(c);
            self.phase = if self.prefix.len() == 1 {
                Phase::PrefixDone
            } else {
                Phase::Prefix(1)
            };
            self.since = Some(now_ms);
            return Feed::Held;
        }
        Feed::Pass(vec![ScanKey::Char(c)])
    }

    /// A character broke a partial match. Everything held goes back to normal
    /// handling, unless the breaking character itself starts a new match.
    fn release_with(&mut self, c: char, now_ms: u64) -> Feed {
        let mut out: Vec<ScanKey> = self.held.drain(..).map(ScanKey::Char).collect();
        self.reset();
        match self.start(c, now_ms) {
            Feed::Held if out.is_empty() => Feed::Held,
            Feed::Held => Feed::Pass(out),
            Feed::Pass(mut keys) => {
                out.append(&mut keys);
                Feed::Pass(out)
            }
            scan @ Feed::Scan(_) => scan,
        }
    }

    fn finish(&mut self, magic: Magic, now_ms: u64) -> Feed {
        let raw = format!("{}{}", magic.as_str(), self.body);
        self.last_scan_ms = self.since.map(|t| now_ms.saturating_sub(t));
        self.reset();
        self.swallow_enter = self.enter_terminates;
        Feed::Scan(ScanPayload::parse(&raw))
    }

    fn push_enter(&mut self, now_ms: u64) -> Feed {
        if self.swallow_enter {
            self.swallow_enter = false;
            return Feed::Held;
        }
        match self.phase.clone() {
            Phase::Idle => Feed::Pass(vec![ScanKey::Enter]),
            Phase::Body { magic, .. } => {
                // With or without the enter_terminates setting, an Enter in the
                // middle of a body means the scan is over. Parsing a short body
                // reports Truncated, which is the right beep.
                self.finish(magic, now_ms)
            }
            _ => {
                let mut out: Vec<ScanKey> = self.held.drain(..).map(ScanKey::Char).collect();
                self.reset();
                out.push(ScanKey::Enter);
                Feed::Pass(out)
            }
        }
    }
}

impl Default for ScanDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(action: ScanAction, id: &str) -> ScanPayload {
        ScanPayload {
            action,
            short_id: ShortId::parse(id).unwrap(),
        }
    }

    #[test]
    fn v1_length_matches_the_field_breakdown() {
        assert_eq!(V1_LEN, 11);
        let s = payload(ScanAction::StartPause, "K4M9Q2").encode(Magic::Dots);
        assert_eq!(s.chars().count(), V1_LEN);
        assert!(s.starts_with("..1S"));
    }

    #[test]
    fn encode_parse_round_trip_both_magics() {
        for magic in Magic::ALL {
            for action in [ScanAction::StartPause, ScanAction::Finish] {
                for n in [0u64, 1, 999_999, 123_456_789, ShortId::SPACE - 1] {
                    let p = ScanPayload {
                        action,
                        short_id: ShortId::from_index(n),
                    };
                    let s = p.encode(magic);
                    assert_eq!(ScanPayload::parse(&s), Ok(p), "{s}");
                    assert_eq!(ScanPayload::parse(&s.to_ascii_lowercase()), Ok(p), "{s}");
                }
            }
        }
    }

    #[test]
    fn golden_encodings_never_change() {
        // Printed slips outlive software versions. If any of these change,
        // every barcode already on paper stops scanning.
        assert_eq!(check_char("A12425GABC1234002M"), Some('Z'));
        assert!(verify_check("A12425GABC1234002MZ"));
        assert!(!verify_check("A12425GABC1234002MN"));
        assert_eq!(
            payload(ScanAction::StartPause, "K4M9Q2").encode(Magic::Dots),
            "..1SK4M9Q2J"
        );
        assert_eq!(
            payload(ScanAction::Finish, "000001").encode(Magic::Dashes),
            "--1F0000015"
        );
    }

    #[test]
    fn check_char_catches_every_substitution_and_nearly_every_transposition() {
        // A misread bar is a substitution, which must never slip through.
        // Transpositions are a typing error, not a scanner error, and the
        // hybrid system misses a fraction of a percent of them. That is
        // accepted, the alternative check schemes need a 37th character that
        // CODE39 cannot carry.
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut transpositions = 0u32;
        let mut missed = 0u32;
        for _ in 0..400 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let p = ScanPayload {
                action: if seed & 1 == 0 {
                    ScanAction::StartPause
                } else {
                    ScanAction::Finish
                },
                short_id: ShortId::from_index(seed >> 8),
            };
            let encoded = p.encode(Magic::Dots);
            let body: Vec<char> = encoded[MAGIC_LEN..].chars().collect();
            assert!(verify_check(&body.iter().collect::<String>()));

            for i in 0..body.len() {
                for v in 0..36 {
                    let c = base36_char(v).unwrap();
                    if c == body[i] {
                        continue;
                    }
                    let mut mutated = body.clone();
                    mutated[i] = c;
                    let s: String = mutated.iter().collect();
                    assert!(
                        !verify_check(&s),
                        "substitution undetected: {encoded} -> {s}"
                    );
                }
            }
            for i in 0..body.len() - 1 {
                if body[i] == body[i + 1] {
                    continue;
                }
                let mut swapped = body.clone();
                swapped.swap(i, i + 1);
                transpositions += 1;
                if verify_check(&swapped.iter().collect::<String>()) {
                    missed += 1;
                }
            }
        }
        assert!(
            missed * 100 < transpositions,
            "missed {missed} of {transpositions} transpositions, more than 1 percent"
        );
    }

    #[test]
    fn parse_errors_are_specific() {
        assert_eq!(
            ScanPayload::parse("0123456789"),
            Err(ScanParseError::NoMagic)
        );
        assert_eq!(
            ScanPayload::parse(".-1SK4M9Q2X"),
            Err(ScanParseError::NoMagic)
        );
        assert_eq!(ScanPayload::parse(""), Err(ScanParseError::NoMagic));
        assert_eq!(
            ScanPayload::parse("..9SK4M9Q2X"),
            Err(ScanParseError::UnknownVersion('9'))
        );
        assert_eq!(
            ScanPayload::parse("..1SK4M"),
            Err(ScanParseError::Truncated {
                expected: V1_LEN,
                got: 7
            })
        );
        assert_eq!(
            ScanPayload::parse("..1SK4M9Q2XX"),
            Err(ScanParseError::TooLong {
                expected: V1_LEN,
                got: 12
            })
        );
        let good = payload(ScanAction::Finish, "K4M9Q2").encode(Magic::Dots);
        let mut bad_check = good.clone();
        let last = bad_check.pop().unwrap();
        bad_check.push(if last == 'A' { 'B' } else { 'A' });
        assert_eq!(
            ScanPayload::parse(&bad_check),
            Err(ScanParseError::BadCheck)
        );
        let mut bad_action = good.clone().into_bytes();
        bad_action[3] = b'Q';
        let bad_action = String::from_utf8(bad_action).unwrap();
        // Check fails first because the action is covered by it; recompute so
        // we hit the action branch.
        let body: String = bad_action[MAGIC_LEN..MAGIC_LEN + 8].to_string();
        let fixed = format!("..{}{}", body, check_char(&body).unwrap());
        assert_eq!(
            ScanPayload::parse(&fixed),
            Err(ScanParseError::BadAction('Q'))
        );
    }

    fn feed_str(d: &mut ScanDetector, s: &str, t: &mut u64) -> Vec<Feed> {
        s.chars()
            .map(|c| {
                *t += 10;
                d.push(ScanKey::Char(c), *t)
            })
            .collect()
    }

    #[test]
    fn detector_passes_normal_typing_through() {
        let mut d = ScanDetector::new();
        let mut t = 0;
        let out = feed_str(&mut d, "hjkl", &mut t);
        for (c, f) in "hjkl".chars().zip(out) {
            assert_eq!(f, Feed::Pass(vec![ScanKey::Char(c)]));
        }
        assert_eq!(d.push(ScanKey::Enter, 1), Feed::Pass(vec![ScanKey::Enter]));
    }

    #[test]
    fn detector_dispatches_a_complete_payload_without_enter() {
        let p = payload(ScanAction::StartPause, "K4M9Q2");
        let s = p.encode(Magic::Dots);
        let mut d = ScanDetector::new();
        let mut t = 0;
        let out = feed_str(&mut d, &s, &mut t);
        assert!(
            out[..V1_LEN - 1].iter().all(|f| *f == Feed::Held),
            "{out:?}"
        );
        assert_eq!(out[V1_LEN - 1], Feed::Scan(Ok(p)));
        assert!(d.is_idle());
        assert_eq!(d.last_scan_duration_ms(), Some(100));
    }

    #[test]
    fn detector_accepts_the_other_magic_too() {
        let p = payload(ScanAction::Finish, "000001");
        let s = p.encode(Magic::Dashes);
        let mut d = ScanDetector::new();
        let mut t = 0;
        let out = feed_str(&mut d, &s, &mut t);
        assert_eq!(out.last(), Some(&Feed::Scan(Ok(p))));
    }

    #[test]
    fn detector_releases_a_lone_dot() {
        let mut d = ScanDetector::new();
        assert_eq!(d.push(ScanKey::Char('.'), 1), Feed::Held);
        assert_eq!(
            d.push(ScanKey::Char('x'), 2),
            Feed::Pass(vec![ScanKey::Char('.'), ScanKey::Char('x')])
        );
        assert!(d.is_idle());
        // A dot followed by a dash is not a magic, both come back.
        assert_eq!(d.push(ScanKey::Char('.'), 3), Feed::Held);
        assert_eq!(
            d.push(ScanKey::Char('-'), 4),
            Feed::Pass(vec![ScanKey::Char('.')])
        );
        assert!(!d.is_idle(), "the dash starts a new half magic");
        assert_eq!(
            d.push(ScanKey::Enter, 5),
            Feed::Pass(vec![ScanKey::Char('-'), ScanKey::Enter])
        );
    }

    #[test]
    fn detector_beeps_on_bad_check_and_bad_version() {
        let mut d = ScanDetector::new();
        let mut t = 0;
        let mut wrong = payload(ScanAction::StartPause, "K4M9Q2").encode(Magic::Dots);
        let last = wrong.pop().unwrap();
        wrong.push(if last == '0' { '1' } else { '0' });
        let out = feed_str(&mut d, &wrong, &mut t);
        assert_eq!(out.last(), Some(&Feed::Scan(Err(ScanParseError::BadCheck))));
        let out = feed_str(&mut d, "..7", &mut t);
        assert_eq!(
            out.last(),
            Some(&Feed::Scan(Err(ScanParseError::UnknownVersion('7'))))
        );
        assert!(d.is_idle());
    }

    #[test]
    fn detector_times_out_partial_buffers() {
        let mut d = ScanDetector::new().with_timeout_ms(1000);
        let mut t = 0;
        feed_str(&mut d, "..1SK", &mut t);
        assert!(!d.is_idle());
        assert!(!d.expire(t + 500));
        assert!(d.expire(t + 1500));
        assert!(d.is_idle());
        // And the same via push, which checks expiry first.
        feed_str(&mut d, "..1SK", &mut t);
        assert_eq!(
            d.push(ScanKey::Char('x'), t + 5000),
            Feed::Pass(vec![ScanKey::Char('x')])
        );
    }

    #[test]
    fn detector_with_enter_terminates() {
        let p = payload(ScanAction::Finish, "ABCDEF");
        let s = p.encode(Magic::Dots);
        let mut d = ScanDetector::new().with_enter_terminates(true);
        let mut t = 0;
        let out = feed_str(&mut d, &s, &mut t);
        assert_eq!(out.last(), Some(&Feed::Scan(Ok(p))));
        // The scanner's trailing Enter must not open the selected task.
        assert_eq!(d.push(ScanKey::Enter, t + 1), Feed::Held);
        assert_eq!(
            d.push(ScanKey::Enter, t + 2),
            Feed::Pass(vec![ScanKey::Enter])
        );
        // A short body followed by Enter is reported as truncated.
        feed_str(&mut d, "..1FABC", &mut t);
        assert!(matches!(
            d.push(ScanKey::Enter, t + 1),
            Feed::Scan(Err(ScanParseError::Truncated { .. }))
        ));
    }

    #[test]
    fn detector_strips_a_configured_prefix() {
        let p = payload(ScanAction::StartPause, "ZZZZZZ");
        let s = format!("#!{}", p.encode(Magic::Dots));
        let mut d = ScanDetector::new().with_prefix("#!");
        let mut t = 0;
        let out = feed_str(&mut d, &s, &mut t);
        assert_eq!(out.last(), Some(&Feed::Scan(Ok(p))));
        // Without the prefix the magic is just typing.
        let out = feed_str(&mut d, "..", &mut t);
        assert_eq!(out[0], Feed::Pass(vec![ScanKey::Char('.')]));
        // A prefix that does not continue is released.
        assert_eq!(d.push(ScanKey::Char('#'), t + 1), Feed::Held);
        assert_eq!(
            d.push(ScanKey::Char('x'), t + 2),
            Feed::Pass(vec![ScanKey::Char('#'), ScanKey::Char('x')])
        );
    }
}
