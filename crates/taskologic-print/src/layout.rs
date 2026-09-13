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
use taskologic_core::print::{Barcode, PrintJob};

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
        SlipSection::BarcodeStart => barcode(out, job, ScanAction::StartPause),
        SlipSection::BarcodeFinish => barcode(out, job, ScanAction::Finish),
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
                out.push(row.line(format!("Start: {}", local(start, job))));
            }
        }
        SlipSection::DueDate => {
            if let Some(due) = job.due_at {
                out.push(row.line(format!("Due: {}", local(due, job))));
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
        SlipSection::Timestamp => out.push(row.line(local(job.created_at, job))),
    }
}

fn barcode(out: &mut Vec<Op>, job: &PrintJob, action: ScanAction) {
    let Some(code) = job.barcodes.iter().find(|b| b.action == action) else {
        return;
    };
    let label = code.label.clone().unwrap_or_else(|| {
        match action {
            ScanAction::StartPause => "scan to start / pause",
            ScanAction::Finish => "scan to finish",
        }
        .to_string()
    });
    out.push(Op::Barcode { code: code.clone(), label });
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
                },
                Barcode {
                    action: ScanAction::Finish,
                    payload: "..1F".into(),
                    symbology: taskologic_core::print::Symbology::Code39,
                    label: None,
                },
            ],
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
