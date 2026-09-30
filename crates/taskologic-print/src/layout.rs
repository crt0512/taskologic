//! What goes on the slip, in what order, without deciding how it is drawn.
//!
//! One `plan` feeds every output mode. ESC/POS, a bitmap and plain text
//! disagree about almost everything else, but they agree on what a task slip
//! says, so the wrapping and the ordering live here once instead of three
//! times drifting apart.
//!
//! What actually appears, in what order and set how, is the printer's
//! [`SlipLayout`]. Nothing else has a say: the job carries everything the
//! task has and this decides what reaches the paper.

use chrono::{DateTime, Utc};
use taskologic_core::barcode::ScanAction;
use taskologic_core::print::{Barcode, PrintJob, PrintJobKind};

use crate::slip::{SlipLayout, SlipRow, SlipSection};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// One line of text, already wrapped to the column count it was planned for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextLine {
    pub text: String,
    pub align: Align,
    pub bold: bool,
    /// 1 for body text, 2 for a section set large.
    pub scale: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Line(TextLine),
    Blank,
    Barcode {
        code: Barcode,
        /// What the line above the code says to do with it.
        label: String,
    },
    /// The end of the slip.
    End,
}

impl SlipRow {
    fn align(&self) -> Align {
        if self.centered { Align::Center } else { Align::Left }
    }

    fn scale(&self) -> u8 {
        if self.large { 2 } else { 1 }
    }

    /// Columns this row's text has, which halves when it is set large.
    fn columns(&self, cols: usize) -> usize {
        (cols / self.scale() as usize).max(1)
    }

    fn line(&self, text: impl Into<String>) -> Op {
        Op::Line(TextLine {
            text: text.into(),
            align: self.align(),
            bold: self.bold,
            scale: self.scale(),
        })
    }

    /// Wrapped to this row's width, one Op a line.
    fn lines(&self, text: &str, cols: usize) -> Vec<Op> {
        wrap(text, self.columns(cols)).into_iter().map(|l| self.line(l)).collect()
    }
}

/// Lay a job out for a printer this many characters wide, in the order and
/// with the emphasis this slip layout asks for. The caller picks the layout
/// that suits the kind of slip; by here the choice is already made.
pub fn plan(job: &PrintJob, cols: usize, layout: &SlipLayout) -> Vec<Op> {
    let cols = cols.max(1);
    if job.kind == PrintJobKind::Codes {
        return codes_card(job, cols);
    }
    let mut ops: Vec<Op> = Vec::new();
    for row in &layout.rows {
        if !row.enabled {
            continue;
        }
        let at = ops.len();
        section(&mut ops, row, job, cols);
        // A blank line between sections, but only between ones that actually
        // put something on the paper. A section the task has nothing for
        // leaves no gap behind it.
        if ops.len() > at && at > 0 {
            ops.insert(at, Op::Blank);
        }
    }
    ops.push(Op::End);
    ops
}

fn section(out: &mut Vec<Op>, row: &SlipRow, job: &PrintJob, cols: usize) {
    match row.section {
        SlipSection::Custom => {
            if !row.text.trim().is_empty() {
                out.extend(row.lines(&row.text, cols));
            }
        }
        SlipSection::BoardTitle => out.push(row.line(fit(&job.board_name, row.columns(cols)))),
        SlipSection::TaskTitle => {
            out.extend(row.lines(job.title.trim(), cols));
            // The short id is what a person reads back to find the task, so
            // it stays with the title and stays small.
            out.push(Op::Line(TextLine {
                text: format!("#{}", job.short_id),
                align: row.align(),
                bold: false,
                scale: 1,
            }));
        }
        SlipSection::BarcodeStart => barcodes(out, job, false),
        // Every code that finishes the task: the plain one, or one per
        // answer when a program gave the task a question.
        SlipSection::BarcodeFinish => barcodes(out, job, true),
        SlipSection::Description => {
            let Some(desc) = &job.description else { return };
            for para in desc.lines() {
                if para.trim().is_empty() {
                    out.push(Op::Blank);
                } else {
                    out.extend(row.lines(para, cols));
                }
            }
        }
        SlipSection::Checklist => {
            for item in &job.checklist {
                let mark = if item.done { "[x]" } else { "[ ]" };
                out.extend(row.lines(&format!("{mark} {}", item.text), cols));
            }
        }
        SlipSection::StartDate => {
            if let Some(start) = job.start_at {
                dated(out, row, "Start", start, job, cols);
            }
        }
        SlipSection::DueDate => {
            if let Some(due) = job.due_at {
                dated(out, row, "Due", due, job, cols);
            }
        }
        SlipSection::Creator => {
            if let Some(c) = &job.created_by {
                out.extend(row.lines(&format!("By: {c}"), cols));
            }
        }
        SlipSection::Assignee => {
            if let Some(a) = &job.assignees {
                out.extend(row.lines(&format!("For: {}", a.join(", ")), cols));
            }
        }
        SlipSection::Dependencies => {
            let Some(deps) = &job.dependencies else { return };
            out.push(row.line("Depends on:"));
            for d in deps {
                let mark = if d.done { "[x]" } else { "[ ]" };
                let line = format!("{mark} {} {}", d.short_id, d.title);
                out.push(row.line(fit(&line, row.columns(cols))));
            }
        }
        SlipSection::Sheet => {
            for entry in &job.sheet {
                out.push(row.line(fit(
                    &format!("#{} {}", entry.short_id, entry.title),
                    row.columns(cols),
                )));
                let label = entry
                    .barcode
                    .label
                    .clone()
                    .unwrap_or_else(|| "scan to start".to_string());
                out.push(Op::Barcode {
                    code: entry.barcode.clone(),
                    label,
                });
            }
        }
        SlipSection::Timestamp => out.push(row.line(local(job.created_at, job))),
    }
}

/// A codes card is not shaped by the printer's layout: a heading, then
/// every code with its label above and its payload below, which is what
/// the barcode op draws anyway. Nothing on it depends on a task.
fn codes_card(job: &PrintJob, cols: usize) -> Vec<Op> {
    let mut ops = Vec::new();
    // A card with one code whose label is the heading would say it twice;
    // the label above the code is enough then. A strip keeps its heading.
    let heading_says_it_all = job.codes.len() == 1
        && job.codes[0].label.trim().eq_ignore_ascii_case(job.title.trim());
    if !heading_says_it_all {
        ops.push(Op::Line(TextLine {
            text: fit(&job.title, cols),
            align: Align::Center,
            bold: true,
            scale: 1,
        }));
    }
    for (i, line) in job.codes.iter().enumerate() {
        if i > 0 || !ops.is_empty() {
            ops.push(Op::Blank);
        }
        ops.push(Op::Barcode {
            code: line.barcode(),
            label: line.label.clone(),
        });
    }
    ops.push(Op::End);
    ops
}

fn barcodes(out: &mut Vec<Op>, job: &PrintJob, finishing: bool) {
    for code in job.barcodes.iter().filter(|b| b.action.finishes() == finishing) {
        let label = code.label.clone().unwrap_or_else(|| {
            match code.action {
                ScanAction::StartPause => "scan to start / pause".to_string(),
                ScanAction::Finish => "scan to finish".to_string(),
                ScanAction::Yes => "scan to finish: yes".to_string(),
                ScanAction::No => "scan to finish: no".to_string(),
                ScanAction::Choice(n) => format!("scan to finish: answer {n}"),
                ScanAction::FinishChildren => "scan to finish what is running".to_string(),
            }
        });
        out.push(Op::Barcode {
            code: code.clone(),
            label,
        });
    }
}

/// "Due: 2027-01-15 09:00" on one line while it fits at the row's size. Set
/// large it needs twice the columns, which a 58 mm printer and an 80 mm
/// bitmap one do not have, and the end of the line was simply cut off. Then
/// the time goes on a line of its own ("At: 09:00") and neither line is bold; a paper too
/// narrow even for the date drops to body size.
fn dated(out: &mut Vec<Op>, row: &SlipRow, label: &str, when: DateTime<Utc>, job: &PrintJob, cols: usize) {
    let stamp = local(when, job);
    let whole = format!("{label}: {stamp}");
    if whole.chars().count() <= row.columns(cols) {
        out.push(row.line(whole));
        return;
    }
    let (date, time) = stamp.split_once(' ').unwrap_or((stamp.as_str(), ""));
    let mut plain = row.clone();
    plain.bold = false;
    let first = format!("{label}: {date}");
    if first.chars().count() > plain.columns(cols) {
        plain.large = false;
    }
    out.push(plain.line(first));
    if !time.is_empty() {
        out.push(plain.line(format!("At: {time}")));
    }
}

pub fn local(t: DateTime<Utc>, job: &PrintJob) -> String {
    t.with_timezone(&job.timezone).format("%Y-%m-%d %H:%M").to_string()
}

pub fn fit(s: &str, cols: usize) -> String {
    let mut out: String = s.chars().take(cols).collect();
    if s.chars().count() > cols && cols > 1 {
        out.pop();
        out.push('~');
    }
    out
}

/// Greedy word wrap. Words longer than a line are split hard.
pub fn wrap(s: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0;
    for word in s.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        while word.len() > cols {
            if cur_len > 0 {
                lines.push(std::mem::take(&mut cur));
                cur_len = 0;
            }
            lines.push(word.drain(..cols).collect());
        }
        let wl = word.len();
        if cur_len > 0 && cur_len + 1 + wl > cols {
            lines.push(std::mem::take(&mut cur));
            cur_len = 0;
        }
        if cur_len > 0 {
            cur.push(' ');
            cur_len += 1;
        }
        cur.extend(word);
        cur_len += wl;
    }
    if cur_len > 0 {
        lines.push(cur);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slip::{SlipSection, SlipSet};
    use taskologic_core::ids::{ShortId, TaskId};
    use taskologic_core::print::{Barcode, DepLine, PrintJobKind};
    use taskologic_core::task::ChecklistItem;

    fn job(kind: PrintJobKind) -> PrintJob {
        PrintJob {
            kind,
            task_id: TaskId(1),
            short_id: ShortId::from_index(0),
            board_name: "Kitchen".into(),
            title: "Water".into(),
            description: Some("Balcony".into()),
            start_at: Some(DateTime::from_timestamp(1_799_996_400, 0).unwrap()),
            due_at: Some(DateTime::from_timestamp(1_800_000_000, 0).unwrap()),
            dependencies: Some(vec![DepLine {
                short_id: ShortId::from_index(1),
                title: "Buy a can".into(),
                done: false,
            }]),
            created_by: Some("alice".into()),
            assignees: Some(vec!["bob".into()]),
            checklist: vec![
                ChecklistItem { text: "Balcony".into(), done: true },
                ChecklistItem { text: "Kitchen".into(), done: false },
            ],
            barcodes: vec![
                Barcode {
                    action: ScanAction::StartPause,
                    payload: "..1S".into(),
                    symbology: taskologic_core::print::Symbology::Code39,
                    label: None,
                    narrow: false,
                },
                Barcode {
                    action: ScanAction::Finish,
                    payload: "..1F".into(),
                    symbology: taskologic_core::print::Symbology::Code39,
                    label: None,
                    narrow: false,
                },
            ],
            sheet: Vec::new(),
            codes: Vec::new(),
            timezone: chrono_tz::UTC,
            created_at: DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
        }
    }

    fn texts(ops: &[Op]) -> Vec<String> {
        ops.iter()
            .filter_map(|o| match o {
                Op::Line(l) => Some(l.text.clone()),
                Op::Barcode { label, .. } => Some(format!("<barcode {label}>")),
                _ => None,
            })
            .collect()
    }

    fn line<'a>(ops: &'a [Op], starts: &str) -> &'a TextLine {
        ops.iter()
            .find_map(|o| match o {
                Op::Line(l) if l.text.starts_with(starts) => Some(l),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no line starting {starts:?} in {:?}", texts(ops)))
    }

    #[test]
    fn the_default_layout_puts_a_task_slip_in_the_documented_order() {
        let ops = plan(&job(PrintJobKind::Task), 32, &SlipLayout::task());
        let t = texts(&ops);
        let at = |s: &str| t.iter().position(|x| x.starts_with(s));
        assert!(at("Taskologic").is_none(), "the custom line is off by default");
        assert!(at("Kitchen") < at("Water"), "board title above task title");
        assert!(at("Water") < at("Balcony"), "title above description");
        assert!(at("Balcony") < at("[x] Balcony"), "description above checklist");
        assert!(at("[ ] Kitchen") < at("Due:"), "checklist above the due date");
        assert!(at("Due:") < at("Depends on:"), "due date above dependencies");
        assert!(at("Depends on:") < at("<barcode scan to finish>"));
        assert!(at("<barcode scan to finish>") < at("2027-"), "the timestamp is last");
        // Off by default.
        assert!(at("By: ").is_none(), "creator");
        assert!(at("For: ").is_none(), "assignees");
        assert!(at("<barcode scan to start / pause>").is_none(), "reminders only");
    }

    #[test]
    fn a_checklist_reaches_the_paper_with_its_boxes_ticked() {
        let ops = plan(&job(PrintJobKind::Task), 32, &SlipLayout::task());
        let t = texts(&ops);
        assert!(t.contains(&"[x] Balcony".to_string()), "{t:?}");
        assert!(t.contains(&"[ ] Kitchen".to_string()), "{t:?}");
    }

    #[test]
    fn the_start_barcode_is_for_reminders_and_the_finish_one_is_not() {
        let set = SlipSet::default();
        let kind = PrintJobKind::Task;
        let task = texts(&plan(&job(kind), 32, set.for_kind(kind)));
        let kind = PrintJobKind::Reminder;
        let rem = texts(&plan(&job(kind), 32, set.for_kind(kind)));
        assert!(task.iter().any(|l| l.contains("scan to finish")));
        assert!(!task.iter().any(|l| l.contains("start / pause")));
        assert!(rem.iter().any(|l| l.contains("start / pause")));
        assert!(!rem.iter().any(|l| l.contains("scan to finish")));
    }

    #[test]
    fn moving_a_row_moves_what_it_prints() {
        let mut l = SlipLayout::task();
        let before = texts(&plan(&job(PrintJobKind::Task), 32, &l));
        let stamp = l.rows.iter().position(|r| r.section == SlipSection::Timestamp).unwrap();
        // Walk the timestamp to the top of the slip.
        for i in (1..=stamp).rev() {
            assert!(l.move_up(i));
        }
        let after = texts(&plan(&job(PrintJobKind::Task), 32, &l));
        assert!(before.last().unwrap().starts_with("2027-"), "{before:?}");
        assert!(after.first().unwrap().starts_with("2027-"), "{after:?}");
        assert_eq!(before.len(), after.len(), "the same things, in a different order");
    }

    #[test]
    fn a_due_date_too_wide_for_large_type_puts_the_time_on_its_own_line() {
        let due_lines = |cols: usize, bold: bool| {
            let mut l = SlipLayout::task();
            for r in &mut l.rows {
                if r.section == SlipSection::DueDate {
                    r.bold = bold;
                }
            }
            let ops = plan(&job(PrintJobKind::Task), cols, &l);
            let at = ops
                .iter()
                .position(|o| matches!(o, Op::Line(t) if t.text.starts_with("Due:")))
                .expect("a due line");
            let Op::Line(first) = ops[at].clone() else { unreachable!() };
            let next = match &ops[at + 1] {
                Op::Line(t) => Some(t.clone()),
                _ => None,
            };
            (first, next)
        };
        // 48 columns at double size holds all 21 characters: one line, as ever.
        let (one, next) = due_lines(48, false);
        assert!(one.text.len() > "Due: 2027-01-15".len() && one.scale == 2, "{one:?}");
        assert!(next.is_none_or(|n| !n.text.contains(':') || n.text.starts_with("Depends")), "nothing split off");
        // 35 (80 mm bitmap) and 32 (58 mm): the date, then the time by itself,
        // still large and not bold even when the row asked for bold.
        for cols in [35, 32] {
            let (date, time) = due_lines(cols, true);
            assert!(date.text.starts_with("Due: 2027-01-") && date.text.len() == 15, "{cols}: {date:?}");
            assert!(!date.bold && date.scale == 2 && date.align == Align::Center, "{cols}: {date:?}");
            let time = time.expect("the time on a new line");
            assert!(time.text.starts_with("At: ") && time.text.len() == 9, "{cols}: {time:?}");
            assert!(time.text.contains(':') && !time.bold && time.scale == 2 && time.align == Align::Center, "{cols}: {time:?}");
        }
        // Too narrow even for the date at double size: body size.
        let (date, time) = due_lines(20, false);
        assert_eq!(date.scale, 1, "{date:?}");
        assert_eq!(time.unwrap().scale, 1);
    }

    #[test]
    fn the_start_date_splits_the_same_way() {
        let mut l = SlipLayout::task();
        for r in &mut l.rows {
            if r.section == SlipSection::StartDate {
                r.enabled = true;
                r.large = true;
            }
        }
        let mut j = job(PrintJobKind::Task);
        j.start_at = j.due_at;
        let lines = texts(&plan(&j, 32, &l));
        let at = lines.iter().position(|t| t.starts_with("Start:")).expect("a start line");
        assert_eq!(lines[at].len(), "Start: 2027-01-15".len(), "{lines:?}");
        assert_eq!(lines[at + 1].len(), "At: 09:00".len(), "the time by itself: {lines:?}");
        assert!(lines[at + 1].starts_with("At: "), "{lines:?}");
    }

    #[test]
    fn each_row_carries_its_own_emphasis() {
        let mut l = SlipLayout::task();
        for r in &mut l.rows {
            r.enabled = true;
        }
        let ops = plan(&job(PrintJobKind::Task), 32, &l);
        let board = line(&ops, "Kitchen");
        assert!(board.bold && board.align == Align::Center && board.scale == 1);
        let title = line(&ops, "Water");
        assert!(title.bold && title.scale == 2 && title.align == Align::Left);
        let due = line(&ops, "Due:");
        assert!(!due.bold && due.scale == 2 && due.align == Align::Center);
        let by = line(&ops, "By: ");
        assert!(!by.bold && by.scale == 1 && by.align == Align::Left);
    }

    #[test]
    fn a_custom_line_prints_its_own_words_when_it_is_turned_on() {
        let mut l = SlipLayout::task();
        let row = l.rows.iter_mut().find(|r| r.section == SlipSection::Custom).unwrap();
        row.enabled = true;
        row.text = "Bay 4".into();
        let t = texts(&plan(&job(PrintJobKind::Task), 32, &l));
        assert_eq!(t.first().map(String::as_str), Some("Bay 4"), "{t:?}");
    }

    #[test]
    fn turning_everything_off_still_leaves_a_slip_that_ends() {
        let mut l = SlipLayout::task();
        for r in &mut l.rows {
            r.enabled = false;
        }
        let ops = plan(&job(PrintJobKind::Task), 32, &l);
        assert_eq!(ops, vec![Op::End], "nothing but the cut");
    }

    #[test]
    fn a_section_the_task_has_nothing_for_leaves_no_gap() {
        let mut j = job(PrintJobKind::Task);
        j.description = None;
        j.checklist.clear();
        j.dependencies = None;
        let ops = plan(&j, 32, &SlipLayout::task());
        // No two blanks in a row, which is what an empty section would leave.
        assert!(
            !ops.windows(2).any(|w| w == [Op::Blank, Op::Blank]),
            "{:?}",
            texts(&ops)
        );
    }


    #[test]
    fn wrapping() {
        assert_eq!(wrap("a bb ccc dddd", 6), vec!["a bb", "ccc", "dddd"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap("   ", 4), Vec::<String>::new());
        assert_eq!(wrap("x abcdefgh y", 4), vec!["x", "abcd", "efgh", "y"]);
    }

    #[test]
    fn a_long_word_is_cut_rather_than_dropped() {
        assert_eq!(fit("abcdef", 4), "abc~");
        assert_eq!(fit("abc", 4), "abc");
    }
}
