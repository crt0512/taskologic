//! Control codes: barcodes that drive the client rather than one task.
//!
//! ```text
//! --1<command>[/<command>]*--
//!
//! --       opening frame
//! 1        format version
//! command  a verb of one to three uppercase letters with its arguments
//!          attached, or a value
//! /        joins commands into one code
//! --       closing frame
//! ```
//!
//! Spelling is terse on purpose: every character is bar width, and 58 mm
//! paper holds about twelve characters of CODE39 at the usual bar width.
//! The label printed above a code is what a person reads; the payload under
//! it is for comparing against what the scanner typed when something goes
//! wrong. There is no check character for now.
//!
//! Arguments attach without a separator when their length is fixed (a
//! column letter, a digit, a six character id, an offset like `2H`). Free
//! text follows `V.` and is always the last thing in the frame, taking
//! everything up to the closing frame, dots and slashes included.
//!
//! The detector in [`crate::barcode`] finds the frames in the keystroke
//! stream and hands the body (what sits between `--1` and `--`) to
//! [`parse`]. The client interprets the commands; nothing here has side
//! effects.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::ShortId;
use crate::print::Symbology;

/// Opens and closes a frame.
pub const FRAME: &str = "--";
/// The only format version so far.
pub const VERSION_1: char = '1';
/// Separates commands inside a frame.
pub const JOIN: char = '/';
/// Introduces free text, which runs to the end of the frame.
pub const TEXT: &str = "V.";

/// A column the way a code names it: by role or by position from the left.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnRef {
    /// The first column that carries no role.
    Todo,
    Paused,
    Doing,
    Finished,
    /// 1 to 9, counted from the left.
    Index(u8),
    /// The rightmost column.
    Last,
}

impl ColumnRef {
    pub fn code(self) -> char {
        match self {
            ColumnRef::Todo => 'T',
            ColumnRef::Paused => 'P',
            ColumnRef::Doing => 'D',
            ColumnRef::Finished => 'F',
            ColumnRef::Index(n) => char::from(b'0' + n.clamp(1, 9)),
            ColumnRef::Last => 'L',
        }
    }

    pub fn from_code(c: char) -> Option<Self> {
        match c.to_ascii_uppercase() {
            'T' => Some(ColumnRef::Todo),
            'P' => Some(ColumnRef::Paused),
            'D' => Some(ColumnRef::Doing),
            'F' => Some(ColumnRef::Finished),
            '1'..='9' => Some(ColumnRef::Index(c as u8 - b'0')),
            'L' => Some(ColumnRef::Last),
            _ => None,
        }
    }

    pub fn label(self) -> String {
        match self {
            ColumnRef::Todo => "todo".into(),
            ColumnRef::Paused => "paused".into(),
            ColumnRef::Doing => "doing".into(),
            ColumnRef::Finished => "done".into(),
            ColumnRef::Index(n) => format!("column {n}"),
            ColumnRef::Last => "the last column".into(),
        }
    }
}

/// A unit for plus and minus.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    #[default]
    Minutes,
    Hours,
    Days,
    Weeks,
    Months,
    Years,
}

impl Unit {
    pub const ALL: [Unit; 6] = [
        Unit::Minutes,
        Unit::Hours,
        Unit::Days,
        Unit::Weeks,
        Unit::Months,
        Unit::Years,
    ];

    /// Minutes are the unit nobody has to spell.
    pub fn code(self) -> &'static str {
        match self {
            Unit::Minutes => "",
            Unit::Hours => "H",
            Unit::Days => "D",
            Unit::Weeks => "W",
            Unit::Months => "MO",
            Unit::Years => "Y",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "" => Some(Unit::Minutes),
            "H" => Some(Unit::Hours),
            "D" => Some(Unit::Days),
            "W" => Some(Unit::Weeks),
            "MO" => Some(Unit::Months),
            "Y" => Some(Unit::Years),
            _ => None,
        }
    }

    pub fn label(self, n: u32) -> String {
        let word = match self {
            Unit::Minutes => "minute",
            Unit::Hours => "hour",
            Unit::Days => "day",
            Unit::Weeks => "week",
            Unit::Months => "month",
            Unit::Years => "year",
        };
        if n == 1 {
            format!("{n} {word}")
        } else {
            format!("{n} {word}s")
        }
    }
}

/// What a command that waits for a value accepts. Combinable: `N/P2H` is
/// now plus two hours, `TD/P1D` is tomorrow.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Value {
    /// Now, date and time.
    Now,
    /// Today, date only; a time already in the field is kept.
    Today,
    /// The current time, time only; a date already in the field is kept.
    Time,
    /// So much later than the field's value, or than now when it has none.
    Plus(u32, Unit),
    Minus(u32, Unit),
    /// The scanning user's username.
    Me,
    /// Clear the field.
    Clear,
    /// Free text, last in the frame.
    Text(String),
    /// Ends the start date's values: what follows is for the due date
    /// (`SB/P30/U/P60`). Only start and due together look at it.
    Split,
}

impl Value {
    pub fn encode(&self) -> String {
        match self {
            Value::Now => "N".into(),
            Value::Today => "TD".into(),
            Value::Time => "TM".into(),
            Value::Plus(n, u) => format!("P{n}{}", u.code()),
            Value::Minus(n, u) => format!("M{n}{}", u.code()),
            Value::Me => "ME".into(),
            Value::Clear => "X".into(),
            Value::Split => "U".into(),
            Value::Text(t) => format!("{TEXT}{t}"),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Value::Now => "now".into(),
            Value::Today => "today".into(),
            Value::Time => "this time".into(),
            Value::Plus(n, u) => format!("plus {}", u.label(*n)),
            Value::Minus(n, u) => format!("minus {}", u.label(*n)),
            Value::Me => "my name".into(),
            Value::Clear => "clear".into(),
            Value::Split => "then the due date".into(),
            Value::Text(t) => format!("\"{t}\""),
        }
    }

    /// Whether this is text, which has to come last.
    pub fn is_text(&self) -> bool {
        matches!(self, Value::Text(_))
    }
}

/// A field a `SET` names.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Title,
    Description,
    Start,
    Due,
    RemindStart,
    RemindDue,
    Assign,
    /// Add a checklist item with the value as its text.
    Checklist,
    /// Tick the n-th checklist item.
    ChecklistItem(u8),
    ExcludeFromStats,
    /// Start and due together, the same value applied to each on its own.
    StartAndDue,
}

impl Field {
    pub fn code(self) -> String {
        match self {
            Field::Title => "T".into(),
            Field::Description => "D".into(),
            Field::Start => "S".into(),
            Field::Due => "U".into(),
            Field::RemindStart => "RS".into(),
            Field::RemindDue => "RU".into(),
            Field::Assign => "A".into(),
            Field::Checklist => "C".into(),
            Field::ChecklistItem(n) => format!("C{}", n.clamp(1, 9)),
            Field::ExcludeFromStats => "X".into(),
            Field::StartAndDue => "B".into(),
        }
    }

    fn from_code(s: &str) -> Option<Self> {
        match s {
            "T" => Some(Field::Title),
            "D" => Some(Field::Description),
            "S" => Some(Field::Start),
            "U" => Some(Field::Due),
            "RS" => Some(Field::RemindStart),
            "RU" => Some(Field::RemindDue),
            "A" => Some(Field::Assign),
            "C" => Some(Field::Checklist),
            "X" => Some(Field::ExcludeFromStats),
            "B" => Some(Field::StartAndDue),
            _ => {
                let mut it = s.chars();
                match (it.next(), it.next(), it.next()) {
                    (Some('C'), Some(d @ '1'..='9'), None) => Some(Field::ChecklistItem(d as u8 - b'0')),
                    _ => None,
                }
            }
        }
    }

    pub fn label(self) -> String {
        match self {
            Field::Title => "title".into(),
            Field::Description => "description".into(),
            Field::Start => "start".into(),
            Field::Due => "due".into(),
            Field::RemindStart => "reminder before start".into(),
            Field::RemindDue => "reminder before due".into(),
            Field::Assign => "assignee".into(),
            Field::Checklist => "checklist item".into(),
            Field::ChecklistItem(n) => format!("checklist item {n}"),
            Field::ExcludeFromStats => "exclude from stats".into(),
            Field::StartAndDue => "start and due".into(),
        }
    }
}

/// Which slip a print override asks for.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlipChoice {
    /// The reminder slip, laid out around the start code.
    Reminder,
    /// The task slip, laid out around the finish codes.
    Finish,
    /// The fixed card with the start/pause code alone.
    Pause,
    /// Reminder and task slip both.
    Combo,
}

impl SlipChoice {
    fn code(self) -> char {
        match self {
            SlipChoice::Reminder => 'R',
            SlipChoice::Finish => 'F',
            SlipChoice::Pause => 'P',
            SlipChoice::Combo => 'C',
        }
    }

    fn from_code(c: char) -> Option<Self> {
        match c {
            'R' => Some(SlipChoice::Reminder),
            'F' => Some(SlipChoice::Finish),
            'P' => Some(SlipChoice::Pause),
            'C' => Some(SlipChoice::Combo),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SlipChoice::Reminder => "print the reminder slip",
            SlipChoice::Finish => "print the finish slip",
            SlipChoice::Pause => "print the pause card",
            SlipChoice::Combo => "print reminder and finish slips",
        }
    }
}

/// A key with no character of its own.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamedKey {
    Up,
    Down,
    Left,
    Right,
    Esc,
    Enter,
    Tab,
    BackTab,
    /// F1 to F12, printed `XF1` to `XF12`.
    F(u8),
}

impl NamedKey {
    pub const ALL: [NamedKey; 20] = [
        NamedKey::Up,
        NamedKey::Down,
        NamedKey::Left,
        NamedKey::Right,
        NamedKey::Esc,
        NamedKey::Enter,
        NamedKey::Tab,
        NamedKey::BackTab,
        NamedKey::F(1),
        NamedKey::F(2),
        NamedKey::F(3),
        NamedKey::F(4),
        NamedKey::F(5),
        NamedKey::F(6),
        NamedKey::F(7),
        NamedKey::F(8),
        NamedKey::F(9),
        NamedKey::F(10),
        NamedKey::F(11),
        NamedKey::F(12),
    ];

    fn code(self) -> String {
        match self {
            NamedKey::Up => "U".into(),
            NamedKey::Down => "D".into(),
            NamedKey::Left => "L".into(),
            NamedKey::Right => "R".into(),
            NamedKey::Esc => "E".into(),
            NamedKey::Enter => "N".into(),
            NamedKey::Tab => "T".into(),
            NamedKey::BackTab => "B".into(),
            NamedKey::F(n) => format!("F{n}"),
        }
    }

    fn from_code(c: &str) -> Option<Self> {
        match c {
            "U" => Some(NamedKey::Up),
            "D" => Some(NamedKey::Down),
            "L" => Some(NamedKey::Left),
            "R" => Some(NamedKey::Right),
            "E" => Some(NamedKey::Esc),
            "N" => Some(NamedKey::Enter),
            "T" => Some(NamedKey::Tab),
            "B" => Some(NamedKey::BackTab),
            _ => {
                let n: u8 = c.strip_prefix('F')?.parse().ok()?;
                (1..=12).contains(&n).then_some(NamedKey::F(n))
            }
        }
    }

    pub fn label(self) -> String {
        match self {
            NamedKey::F(n) => format!("F{n}"),
            NamedKey::Up => "up".into(),
            NamedKey::Down => "down".into(),
            NamedKey::Left => "left".into(),
            NamedKey::Right => "right".into(),
            NamedKey::Esc => "Esc".into(),
            NamedKey::Enter => "Enter".into(),
            NamedKey::Tab => "Tab".into(),
            NamedKey::BackTab => "Shift+Tab".into(),
        }
    }
}

/// One command of a frame. Values are commands too, syntactically: a frame
/// is a list, and the client works through it in order.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Control {
    // Next scanned task: these arm an override.
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveTop,
    MoveBottom,
    /// None asks on screen which column.
    MoveTo(Option<ColumnRef>),
    Delete,
    Print(SlipChoice),
    AssignMe,
    UnassignMe,
    ToggleAssign,
    /// Opens the task form on the field, or sets it when a value follows.
    Set(Field),
    /// Opens the next scanned task, or the one named.
    Show(Option<ShortId>),
    /// The highlighted task stands in for a scan.
    Selected,
    /// Modifier: the override stays armed after each use.
    Sticky,
    // Navigation and keys.
    Key(char),
    Named(NamedKey),
    /// A named key pressed several times, printed `XT3`; not for F keys.
    NamedTimes(NamedKey, u8),
    Dashboard,
    NextBoard,
    PrevBoard,
    /// Wants a text value: opens the search with it.
    Search { archive: bool },
    /// Toasts "scanner ok".
    Ping,
    /// Undo the last change a control code made to a task.
    Undo,
    // Data entry into the focused field. Both want a value.
    Insert,
    Replace,
    /// A value for whatever is waiting.
    Value(Value),
    // Targeted.
    ShowBoard(ShortId),
    Analytics(ShortId),
    StartProgram(ShortId),
    StartProgramNow(ShortId),
    NewFromTemplate(ShortId, ColumnRef),
    NewFromTemplateAsk(ShortId, ColumnRef),
}

impl Control {
    /// The command as it sits in a frame, without the frame.
    pub fn encode(&self) -> String {
        use Control::*;
        match self {
            MoveLeft => "ML".into(),
            MoveRight => "MR".into(),
            MoveUp => "MU".into(),
            MoveDown => "MD".into(),
            MoveTop => "MT".into(),
            MoveBottom => "MB".into(),
            MoveTo(None) => "MC".into(),
            MoveTo(Some(c)) => format!("MC{}", c.code()),
            Delete => "DEL".into(),
            Print(s) => format!("P{}", s.code()),
            AssignMe => "AM".into(),
            UnassignMe => "AU".into(),
            ToggleAssign => "AT".into(),
            Set(f) => format!("S{}", f.code()),
            Show(None) => "SH".into(),
            Show(Some(id)) => format!("SH{id}"),
            Selected => "SEL".into(),
            Sticky => "STK".into(),
            Key(c) => format!("K{c}"),
            Named(k) => format!("X{}", k.code()),
            NamedTimes(k, n) => format!("X{}{n}", k.code()),
            Dashboard => "GD".into(),
            NextBoard => "GN".into(),
            PrevBoard => "GP".into(),
            Search { archive: false } => "Q".into(),
            Search { archive: true } => "QA".into(),
            Ping => "PING".into(),
            Undo => "UNDO".into(),
            Insert => "I".into(),
            Replace => "R".into(),
            Value(v) => v.encode(),
            ShowBoard(id) => format!("GB{id}"),
            Analytics(id) => format!("GA{id}"),
            StartProgram(id) => format!("RP{id}"),
            StartProgramNow(id) => format!("RN{id}"),
            NewFromTemplate(id, c) => format!("NT{id}{}", c.code()),
            NewFromTemplateAsk(id, c) => format!("NA{id}{}", c.code()),
        }
    }

    /// What the slip says above the code.
    pub fn label(&self) -> String {
        use Control::*;
        match self {
            MoveLeft => "move left".into(),
            MoveRight => "move right".into(),
            MoveUp => "move up".into(),
            MoveDown => "move down".into(),
            MoveTop => "move to the top".into(),
            MoveBottom => "move to the bottom".into(),
            MoveTo(None) => "move to a column".into(),
            MoveTo(Some(c)) => format!("move to {}", c.label()),
            Delete => "delete (archive)".into(),
            Print(s) => s.label().into(),
            AssignMe => "assign to me".into(),
            UnassignMe => "unassign me".into(),
            ToggleAssign => "toggle assigning to me".into(),
            Set(f) => format!("set {}", f.label()),
            Show(None) => "show task".into(),
            Show(Some(id)) => format!("show task {id}"),
            Selected => "the selected task".into(),
            Sticky => "sticky".into(),
            Key(c) => format!("press {c}"),
            Named(k) => format!("press {}", k.label()),
            NamedTimes(k, n) => format!("press {} {n} times", k.label()),
            Dashboard => "show all boards".into(),
            NextBoard => "next board".into(),
            PrevBoard => "previous board".into(),
            Search { archive: false } => "search".into(),
            Search { archive: true } => "search, archive included".into(),
            Ping => "scanner check".into(),
            Undo => "undo the last change".into(),
            Insert => "insert".into(),
            Replace => "replace".into(),
            Value(v) => v.label(),
            ShowBoard(id) => format!("show board {id}"),
            Analytics(id) => format!("analytics of board {id}"),
            StartProgram(id) => format!("start program {id}"),
            StartProgramNow(id) => format!("start program {id} now"),
            NewFromTemplate(id, c) => format!("new task from template {id} in {}", c.label()),
            NewFromTemplateAsk(id, c) => format!("new task from template {id} in {}, ask first", c.label()),
        }
    }

    /// Whether this command wants a value after it to do anything.
    pub fn wants_value(&self) -> bool {
        matches!(self, Control::Search { .. } | Control::Insert | Control::Replace)
    }

    /// Keys and free text carry lowercase and punctuation, which CODE39
    /// cannot; they print in CODE128 whatever the width.
    fn needs_code128(&self) -> bool {
        match self {
            Control::Key(c) => !is_code39_char(*c),
            Control::Value(Value::Text(t)) => !t.chars().all(is_code39_char),
            _ => false,
        }
    }

    fn parse_one(token: &str) -> Result<Control, ControlError> {
        use Control::*;
        let bad = || ControlError::UnknownCommand(token.to_string());
        let upper = token.to_ascii_uppercase();
        let t = upper.as_str();
        // Free text and keys keep their case; everything else is compared
        // uppercased so a plain CODE39 scanner and a CODE128 one agree.
        if let Some(text) = token.strip_prefix(TEXT) {
            return Ok(Value(self::Value::Text(text.to_string())));
        }
        let mut it = token.chars();
        if let (Some('K' | 'k'), Some(c), None) = (it.next(), it.next(), it.next()) {
            return Ok(Key(c));
        }
        let id_after = |prefix: &str| -> Result<ShortId, ControlError> {
            let rest = t.strip_prefix(prefix).ok_or_else(bad)?;
            ShortId::parse(rest).map_err(|_| bad())
        };
        Ok(match t {
            "ML" => MoveLeft,
            "MR" => MoveRight,
            "MU" => MoveUp,
            "MD" => MoveDown,
            "MT" => MoveTop,
            "MB" => MoveBottom,
            "MC" => MoveTo(None),
            "DEL" => Delete,
            "AM" => AssignMe,
            "AU" => UnassignMe,
            "AT" => ToggleAssign,
            "SH" => Show(None),
            "SEL" => Selected,
            "STK" => Sticky,
            "GD" => Dashboard,
            "GN" => NextBoard,
            "GP" => PrevBoard,
            "Q" => Search { archive: false },
            "QA" => Search { archive: true },
            "PING" => Ping,
            "UNDO" => Undo,
            "I" => Insert,
            "R" => Replace,
            "N" => Value(self::Value::Now),
            "TD" => Value(self::Value::Today),
            "TM" => Value(self::Value::Time),
            "ME" => Value(self::Value::Me),
            "X" => Value(self::Value::Clear),
            "U" => Value(self::Value::Split),
            _ => {
                let mut chars = t.chars();
                let first = chars.next().ok_or_else(bad)?;
                let second = chars.next();
                match (first, second) {
                    ('M', Some('C')) if t.len() == 3 => {
                        MoveTo(Some(ColumnRef::from_code(t.chars().nth(2).unwrap()).ok_or_else(bad)?))
                    }
                    ('P', Some(c)) if t.len() == 2 && SlipChoice::from_code(c).is_some() => {
                        Print(SlipChoice::from_code(c).unwrap())
                    }
                    ('P' | 'M', Some(d)) if d.is_ascii_digit() => parse_offset(t).ok_or_else(bad)?,
                    ('S', Some(_)) if t.len() <= 3 => Set(Field::from_code(&t[1..]).ok_or_else(bad)?),
                    ('S', Some('H')) => Show(Some(id_after("SH")?)),
                    ('X', Some(_)) if t.len() >= 2 => {
                        let rest = &t[1..];
                        match NamedKey::from_code(rest) {
                            Some(k) => Named(k),
                            None if rest.is_char_boundary(1) => {
                                // `T3`: a key letter and how many times.
                                let (k, n) = rest.split_at(1);
                                let n: u8 = n.parse().ok().filter(|n| (1..=99).contains(n)).ok_or_else(bad)?;
                                let k = NamedKey::from_code(k).ok_or_else(bad)?;
                                NamedTimes(k, n)
                            }
                            None => return Err(bad()),
                        }
                    }
                    ('G', Some('B')) => ShowBoard(id_after("GB")?),
                    ('G', Some('A')) => Analytics(id_after("GA")?),
                    ('R', Some('P')) => StartProgram(id_after("RP")?),
                    ('R', Some('N')) => StartProgramNow(id_after("RN")?),
                    ('N', Some('T' | 'A')) => {
                        let (id, col) = t[2..].split_at(t[2..].len().saturating_sub(1));
                        let id = ShortId::parse(id).map_err(|_| bad())?;
                        let col = ColumnRef::from_code(col.chars().next().ok_or_else(bad)?).ok_or_else(bad)?;
                        if second == Some('T') {
                            NewFromTemplate(id, col)
                        } else {
                            NewFromTemplateAsk(id, col)
                        }
                    }
                    _ => return Err(bad()),
                }
            }
        })
    }
}

/// `P2H`, `M30`, `P1MO`: a sign, digits, a unit code.
fn parse_offset(t: &str) -> Option<Control> {
    let (sign, rest) = t.split_at(1);
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let n: u32 = digits.parse().ok()?;
    let unit = Unit::from_code(&rest[digits.len()..])?;
    Some(Control::Value(match sign {
        "P" => Value::Plus(n, unit),
        _ => Value::Minus(n, unit),
    }))
}

impl fmt::Display for Control {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ControlError {
    #[error("empty control code")]
    Empty,
    #[error("unknown control command {0:?}")]
    UnknownCommand(String),
    #[error("free text has to be the last thing in a code")]
    TextNotLast,
}

/// The whole printed payload for these commands: frame, version, commands
/// joined, frame.
pub fn encode(commands: &[Control]) -> String {
    let body: Vec<String> = commands.iter().map(Control::encode).collect();
    format!("{FRAME}{VERSION_1}{}{FRAME}", body.join(&JOIN.to_string()))
}

/// The commands of a frame body, what sits between `--1` and `--`. A text
/// value takes everything after `V.`, slashes included, so it has to come
/// last.
pub fn parse(body: &str) -> Result<Vec<Control>, ControlError> {
    if body.trim().is_empty() {
        return Err(ControlError::Empty);
    }
    let mut out = Vec::new();
    let mut rest = body;
    loop {
        if rest.starts_with(TEXT) {
            out.push(Control::parse_one(rest)?);
            return Ok(out);
        }
        match rest.find(JOIN) {
            Some(i) => {
                out.push(Control::parse_one(&rest[..i])?);
                rest = &rest[i + 1..];
            }
            None => {
                out.push(Control::parse_one(rest)?);
                return Ok(out);
            }
        }
    }
}

/// One line for a whole code, the way the slip labels it.
pub fn label(commands: &[Control]) -> String {
    commands
        .iter()
        .map(Control::label)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The characters CODE39 can carry.
pub fn is_code39_char(c: char) -> bool {
    c.is_ascii_digit() || c.is_ascii_uppercase() || matches!(c, ' ' | '-' | '.' | '$' | '/' | '+' | '%')
}

/// Modules a CODE39 of `chars` characters takes: 13 per character
/// including the gap after it, start and stop characters included.
pub fn code39_modules(chars: usize) -> usize {
    (chars + 2) * 13 - 1
}

/// Modules a CODE128 of `chars` characters takes: 11 per character, plus
/// start, check and a 13 module stop.
pub fn code128_modules(chars: usize) -> usize {
    (chars + 2) * 11 + 13
}

/// The most characters a payload may have to print at two dots per module
/// on paper `dots` wide, in each symbology.
pub fn fits(chars: usize, symbology: Symbology, dots: usize) -> bool {
    let modules = match symbology {
        Symbology::Code39 => code39_modules(chars),
        _ => code128_modules(chars),
    };
    modules * 2 <= dots
}

/// Which symbology a code prints in on paper `dots` wide: CODE39 when the
/// payload is all CODE39 characters and fits at the usual bar width,
/// CODE128 otherwise. Keys and free text are CODE128 regardless.
pub fn symbology_for(commands: &[Control], dots: usize) -> Symbology {
    let payload = encode(commands);
    let len = payload.chars().count();
    if commands.iter().any(Control::needs_code128) || !payload.chars().all(is_code39_char) {
        return Symbology::Code128;
    }
    if fits(len, Symbology::Code39, dots) {
        Symbology::Code39
    } else {
        Symbology::Code128
    }
}

/// Split a payload that does not fit even in CODE128 at one dot per module
/// into pieces that do, for one barcode each. The detector stitches them:
/// only the last piece carries the closing frame, and Enter between pieces
/// is dropped inside an open frame. A payload that fits comes back whole.
pub fn chunks(payload: &str, dots: usize) -> Vec<String> {
    // One dot per module is the floor; how many characters that allows.
    let max = (1..=payload.chars().count())
        .rev()
        .find(|n| code128_modules(*n) <= dots)
        .unwrap_or(1);
    let chars: Vec<char> = payload.chars().collect();
    if chars.len() <= max {
        return vec![payload.to_string()];
    }
    chars.chunks(max.max(3)).map(|c| c.iter().collect()).collect()
}

/// The categories of the Print codes menu, in the order shown.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    NextScan,
    Navigation,
    DataEntry,
    Values,
    Dates,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::NextScan,
        Category::Navigation,
        Category::DataEntry,
        Category::Values,
        Category::Dates,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::NextScan => "Next scanned task",
            Category::Navigation => "Navigation and keys",
            Category::DataEntry => "Data entry",
            Category::Values => "Dates, times and values",
            Category::Dates => "Start and due dates",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Category::NextScan => "scan one of these, then a task's code: it does this instead of starting or finishing",
            Category::Navigation => "keys and screens, for a terminal without a keyboard",
            Category::DataEntry => "type into whatever field has the focus",
            Category::Values => "what a waiting command takes: combinable, plus and minus stack",
            Category::Dates => "scan one, then a task's code: sets its start or due date to now, today or an offset",
        }
    }
}

/// What the menu asks for before it can print an entry.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Asks {
    Nothing,
    /// A column, for "move to".
    Column,
    /// A number and a unit, for plus and minus.
    Offset,
    /// One key.
    Key,
    /// Free text.
    Text,
    /// How many times, for a key pressed repeatedly.
    Count,
    /// Two offsets, one for the start date and one for the due date.
    OffsetPair,
}

/// One printable line of the menu: a label, the commands, and what has to
/// be asked first. The client turns the answer into the finished commands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub label: &'static str,
    pub commands: Vec<Control>,
    pub asks: Asks,
    /// Whether the override may be printed sticky.
    pub sticky: bool,
}

impl Entry {
    fn plain(label: &'static str, commands: Vec<Control>) -> Self {
        Self {
            label,
            commands,
            asks: Asks::Nothing,
            sticky: false,
        }
    }

    fn armed(label: &'static str, commands: Vec<Control>) -> Self {
        Self {
            sticky: true,
            ..Self::plain(label, commands)
        }
    }

    fn asking(label: &'static str, commands: Vec<Control>, asks: Asks) -> Self {
        Self {
            asks,
            ..Self::plain(label, commands)
        }
    }
}

/// The entries of one category.
pub fn entries(category: Category) -> Vec<Entry> {
    use Control::*;
    match category {
        Category::NextScan => {
            let mut v = vec![
                Entry::armed("Move left", vec![MoveLeft]),
                Entry::armed("Move right", vec![MoveRight]),
                Entry::armed("Move up", vec![MoveUp]),
                Entry::armed("Move down", vec![MoveDown]),
                Entry::armed("Move to the top", vec![MoveTop]),
                Entry::armed("Move to the bottom", vec![MoveBottom]),
                Entry::armed("Move to todo", vec![MoveTo(Some(ColumnRef::Todo))]),
                Entry::armed("Move to paused", vec![MoveTo(Some(ColumnRef::Paused))]),
                Entry::armed("Move to doing", vec![MoveTo(Some(ColumnRef::Doing))]),
                Entry::armed("Move to done", vec![MoveTo(Some(ColumnRef::Finished))]),
                Entry {
                    asks: Asks::Column,
                    ..Entry::armed("Move to a column", vec![MoveTo(None)])
                },
                Entry::armed("Delete (archive)", vec![Delete]),
                Entry::armed("Print the reminder slip", vec![Print(SlipChoice::Reminder)]),
                Entry::armed("Print the finish slip", vec![Print(SlipChoice::Finish)]),
                Entry::armed("Print the pause card", vec![Print(SlipChoice::Pause)]),
                Entry::armed("Print both slips", vec![Print(SlipChoice::Combo)]),
                Entry::armed("Assign to me", vec![AssignMe]),
                Entry::armed("Unassign me", vec![UnassignMe]),
                Entry::armed("Toggle assigning to me", vec![ToggleAssign]),
                Entry::armed("Show the task", vec![Show(None)]),
            ];
            for f in [
                Field::Title,
                Field::Description,
                Field::Start,
                Field::Due,
                Field::StartAndDue,
                Field::RemindStart,
                Field::RemindDue,
                Field::Assign,
                Field::Checklist,
                Field::ExcludeFromStats,
            ] {
                v.push(Entry::armed(field_entry_label(f), vec![Set(f)]));
            }
            v.push(Entry::plain("The selected task", vec![Selected]));
            v
        }
        Category::Navigation => {
            let mut v = vec![
                Entry::plain("Show all boards", vec![Dashboard]),
                Entry::plain("Next board", vec![NextBoard]),
                Entry::plain("Previous board", vec![PrevBoard]),
            ];
            for k in NamedKey::ALL {
                v.push(Entry::plain(named_key_entry_label(k), vec![Named(k)]));
            }
            v.push(Entry::asking("Tab several times", vec![Named(NamedKey::Tab)], Asks::Count));
            v.push(Entry::asking("Shift+Tab several times", vec![Named(NamedKey::BackTab)], Asks::Count));
            v.push(Entry::asking("Press a key", vec![], Asks::Key));
            v.push(Entry::asking("Search", vec![Search { archive: false }], Asks::Text));
            v.push(Entry::asking("Search, archive included", vec![Search { archive: true }], Asks::Text));
            v.push(Entry::plain("Scanner check", vec![Ping]));
            v.push(Entry::plain("Undo the last change", vec![Undo]));
            v
        }
        Category::DataEntry => vec![
            Entry::plain("Insert now", vec![Insert, Value(self::Value::Now)]),
            Entry::plain("Insert today", vec![Insert, Value(self::Value::Today)]),
            Entry::plain("Insert this time", vec![Insert, Value(self::Value::Time)]),
            Entry::plain("Insert my name", vec![Insert, Value(self::Value::Me)]),
            Entry::asking("Insert text", vec![Insert], Asks::Text),
            Entry::plain("Replace with now", vec![Replace, Value(self::Value::Now)]),
            Entry::plain("Replace with today", vec![Replace, Value(self::Value::Today)]),
            Entry::plain("Replace with my name", vec![Replace, Value(self::Value::Me)]),
            Entry::asking("Replace with text", vec![Replace], Asks::Text),
        ],
        Category::Values => vec![
            Entry::plain("Now", vec![Value(self::Value::Now)]),
            Entry::plain("Today", vec![Value(self::Value::Today)]),
            Entry::plain("This time", vec![Value(self::Value::Time)]),
            Entry::asking("Plus so much", vec![], Asks::Offset),
            Entry::asking("Minus so much", vec![], Asks::Offset),
            Entry::plain("My name", vec![Value(self::Value::Me)]),
            Entry::plain("Clear", vec![Value(self::Value::Clear)]),
            Entry::plain("Then the due date", vec![Value(self::Value::Split)]),
            Entry::asking("Text", vec![], Asks::Text),
        ],
        Category::Dates => {
            let mut v = Vec::new();
            for f in [Field::Start, Field::Due, Field::StartAndDue] {
                let (now, today, plus, minus, clear) = if f == Field::StartAndDue {
                    (
                        "Set start and due to now",
                        "Set start and due to today",
                        "Set start and due, plus so much",
                        "Set start and due, minus so much",
                        "Clear start and due",
                    )
                } else if f == Field::Start {
                    (
                        "Set the start date to now",
                        "Set the start date to today",
                        "Set the start date, plus so much",
                        "Set the start date, minus so much",
                        "Clear the start date",
                    )
                } else {
                    (
                        "Set the due date to now",
                        "Set the due date to today",
                        "Set the due date, plus so much",
                        "Set the due date, minus so much",
                        "Clear the due date",
                    )
                };
                v.push(Entry::armed(now, vec![Set(f), Value(self::Value::Now)]));
                v.push(Entry::armed(today, vec![Set(f), Value(self::Value::Today)]));
                v.push(Entry { asks: Asks::Offset, ..Entry::armed(plus, vec![Set(f)]) });
                v.push(Entry { asks: Asks::Offset, ..Entry::armed(minus, vec![Set(f)]) });
                if f == Field::StartAndDue {
                    for (label, base) in [
                        ("Set start and due, start plus, due plus", vec![Set(f)]),
                        ("Set start and due, start plus, due minus", vec![Set(f)]),
                        ("Set start and due, start minus, due plus", vec![Set(f)]),
                        ("Set start and due, start minus, due minus", vec![Set(f)]),
                    ] {
                        v.push(Entry { asks: Asks::OffsetPair, ..Entry::armed(label, base) });
                    }
                }
                v.push(Entry::armed(clear, vec![Set(f), Value(self::Value::Clear)]));
            }
            v
        }
    }
}

fn field_entry_label(f: Field) -> &'static str {
    match f {
        Field::Title => "Set the title",
        Field::Description => "Set the description",
        Field::Start => "Set the start date",
        Field::Due => "Set the due date",
        Field::RemindStart => "Set the reminder before start",
        Field::RemindDue => "Set the reminder before due",
        Field::Assign => "Set the assignee",
        Field::Checklist => "Add a checklist item",
        Field::ChecklistItem(_) => "Tick a checklist item",
        Field::ExcludeFromStats => "Toggle exclude from stats",
        Field::StartAndDue => "Set start and due dates",
    }
}

fn named_key_entry_label(k: NamedKey) -> &'static str {
    match k {
        NamedKey::F(n) => {
            const NAMES: [&str; 12] = ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"];
            NAMES[usize::from(n.clamp(1, 12) - 1)]
        }
        NamedKey::Up => "Up",
        NamedKey::Down => "Down",
        NamedKey::Left => "Left",
        NamedKey::Right => "Right",
        NamedKey::Esc => "Esc (also cancels what is armed)",
        NamedKey::Enter => "Enter",
        NamedKey::Tab => "Tab",
        NamedKey::BackTab => "Shift+Tab",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> ShortId {
        ShortId::parse(s).unwrap()
    }

    #[test]
    fn every_command_round_trips_through_its_payload() {
        use Control::*;
        let all = vec![
            MoveLeft, MoveRight, MoveUp, MoveDown, MoveTop, MoveBottom,
            MoveTo(None), MoveTo(Some(ColumnRef::Todo)), MoveTo(Some(ColumnRef::Index(3))), MoveTo(Some(ColumnRef::Last)),
            Delete, Print(SlipChoice::Pause), AssignMe, UnassignMe, ToggleAssign,
            Set(Field::Title), Set(Field::RemindDue), Set(Field::StartAndDue), Set(Field::ChecklistItem(4)), Set(Field::ExcludeFromStats),
            Show(None), Show(Some(id("K4M9Q2"))), Selected, Sticky,
            Key('T'), Key('n'), Key('?'), Named(NamedKey::BackTab), Named(NamedKey::F(1)), Named(NamedKey::F(12)), NamedTimes(NamedKey::Tab, 3), NamedTimes(NamedKey::BackTab, 12), Dashboard, NextBoard, PrevBoard,
            Search { archive: false }, Search { archive: true }, Ping, Undo, Insert, Replace,
            Value(self::Value::Now), Value(self::Value::Today), Value(self::Value::Time),
            Value(self::Value::Plus(2, Unit::Hours)), Value(self::Value::Minus(30, Unit::Minutes)),
            Value(self::Value::Plus(1, Unit::Months)), Value(self::Value::Split), Value(self::Value::Me), Value(self::Value::Clear),
            ShowBoard(id("000001")), Analytics(id("ABCDEF")), StartProgram(id("K4M9Q2")), StartProgramNow(id("K4M9Q2")),
            NewFromTemplate(id("K4M9Q2"), ColumnRef::Index(1)), NewFromTemplateAsk(id("K4M9Q2"), ColumnRef::Finished),
        ];
        for c in &all {
            let body = c.encode();
            assert_eq!(parse(&body).unwrap(), vec![c.clone()], "{body}");
        }
        // And joined, in one frame.
        let frame = encode(&all[..6]);
        assert_eq!(frame, "--1ML/MR/MU/MD/MT/MB--");
        let body = &frame[3..frame.len() - 2];
        assert_eq!(parse(body).unwrap(), all[..6].to_vec());
    }

    #[test]
    fn free_text_runs_to_the_end_and_keeps_its_case() {
        let cmds = vec![Control::Insert, Control::Value(Value::Text("a/b.c, x".into()))];
        let frame = encode(&cmds);
        assert_eq!(frame, "--1I/V.a/b.c, x--");
        assert_eq!(parse(&frame[3..frame.len() - 2]).unwrap(), cmds);
        // Verbs are read uppercased, text and keys as printed.
        assert_eq!(parse("ml/kt").unwrap(), vec![Control::MoveLeft, Control::Key('t')]);
        assert_eq!(parse("V.Hello").unwrap(), vec![Control::Value(Value::Text("Hello".into()))]);
    }

    #[test]
    fn what_is_not_a_command_says_so() {
        assert_eq!(parse(""), Err(ControlError::Empty));
        assert!(matches!(parse("MW"), Err(ControlError::UnknownCommand(_))));
        assert!(matches!(parse("MCZ"), Err(ControlError::UnknownCommand(_))));
        assert!(matches!(parse("P2Q"), Err(ControlError::UnknownCommand(_))));
        assert!(matches!(parse("GBNOPE"), Err(ControlError::UnknownCommand(_))));
        assert!(matches!(parse("ML//MR"), Err(ControlError::UnknownCommand(_))), "an empty command");
    }

    #[test]
    fn the_everyday_codes_fit_58mm_in_code39_and_the_rest_fall_back() {
        let mm58 = 384;
        let mm80 = 576;
        // Every entry of the next scan and navigation categories, once
        // filled in, prints in CODE39 on 58 mm: that was the point of the
        // terse spelling.
        for c in [
            Control::MoveTo(Some(ColumnRef::Finished)),
            Control::Delete,
            Control::Set(Field::RemindStart),
            Control::Selected,
            Control::Named(NamedKey::Esc),
            Control::Dashboard,
            Control::Ping,
        ] {
            assert_eq!(symbology_for(std::slice::from_ref(&c), mm58), Symbology::Code39, "{}", encode(&[c]));
        }
        // Keys and lowercase text are CODE128 however short.
        assert_eq!(symbology_for(&[Control::Key('t')], mm58), Symbology::Code128);
        assert_eq!(symbology_for(&[Control::Key('T')], mm58), Symbology::Code39, "an uppercase key is plain CODE39");
        // Due in two hours: 13 characters, CODE128 on 58 mm, CODE39 on 80.
        let due = [
            Control::Set(Field::Due),
            Control::Value(Value::Now),
            Control::Value(Value::Plus(2, Unit::Hours)),
        ];
        assert_eq!(encode(&due), "--1SU/N/P2H--");
        assert_eq!(symbology_for(&due, mm58), Symbology::Code128);
        assert_eq!(symbology_for(&due, mm80), Symbology::Code39);
        // A wall of text splits, with the closing frame on the last piece only.
        let long = encode(&[Control::Insert, Control::Value(Value::Text("x".repeat(80)))]);
        let pieces = chunks(&long, mm58);
        assert!(pieces.len() > 1);
        assert_eq!(pieces.concat(), long);
        assert!(pieces.last().unwrap().ends_with(FRAME));
        assert!(!pieces[0].ends_with(FRAME));
        assert_eq!(chunks("--1ML--", mm58), vec!["--1ML--".to_string()]);
    }

    #[test]
    fn the_menu_covers_every_category_and_labels_read() {
        for cat in Category::ALL {
            let es = entries(cat);
            assert!(!es.is_empty(), "{cat:?}");
            for e in &es {
                // Entries that ask for something have their commands finished
                // by the client; the rest print as they are.
                if e.asks == Asks::Nothing {
                    assert!(!e.commands.is_empty(), "{}", e.label);
                    let body = encode(&e.commands);
                    assert!(parse(&body[3..body.len() - 2]).is_ok(), "{}", e.label);
                }
            }
        }
        assert_eq!(label(&[Control::Set(Field::Due), Control::Value(Value::Now)]), "set due, now");
        assert_eq!(Control::MoveTo(Some(ColumnRef::Index(2))).label(), "move to column 2");
        assert_eq!(Value::Plus(1, Unit::Days).label(), "plus 1 day");
    }
}
