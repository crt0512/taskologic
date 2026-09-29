//! Starting a program: when its root task starts. The date is a plain
//! field, with a helper that fills it from "in N minutes" so nobody has to
//! do clock arithmetic. Nothing runs until somebody moves the root to the
//! started column or scans its start code; the date is what the start slip
//! and the reminders count from.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Clear, Paragraph};
use taskologic_core::ids::{ColumnId, ProgramId};
use taskologic_core::offset::{Offset, OffsetUnit};
use taskologic_core::program::Program;

use super::{Row, button_h, button_row, frame_block, label, popup, split_label};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, CheckboxState, ChoiceState, Focus, FocusBuilder, HandleEvent,
    HasFocus, HasScreenCursor, Outcome, Regular, TextInputState, checkbox_at, dropdown,
    dropdown_marker, dropdown_popup_hover, field, render_button,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum StartOutcome {
    Continue,
    Changed,
    Cancel,
    Start {
        program_id: ProgramId,
        column_id: ColumnId,
        start_at: Option<DateTime<Utc>>,
        /// The entries left ticked for each step that fans out.
        fan_out: Vec<(String, Vec<String>)>,
    },
}

/// One step that fans out, with its entries to keep or drop for this run.
struct FanStep {
    key: String,
    title: String,
    entries: Vec<(String, CheckboxState)>,
}

pub struct StartProgramForm {
    program: Program,
    column: ColumnId,
    column_name: String,
    tz: Tz,
    start: TextInputState,
    in_amount: TextInputState,
    in_unit: ChoiceState<OffsetUnit>,
    fan: Vec<FanStep>,
    set_btn: ButtonState,
    start_btn: ButtonState,
    cancel_btn: ButtonState,
    pub error: Option<String>,
    pub saving: bool,
}

impl StartProgramForm {
    pub fn new(program: Program, column: ColumnId, column_name: String, tz: Tz) -> Self {
        let mut start = TextInputState::named("start");
        start.set_text(stamp(Utc::now(), tz));
        start.focus().set(true);
        let mut in_amount = TextInputState::named("in_amount");
        in_amount.set_text("30");
        let mut in_unit = ChoiceState::named("in_unit");
        in_unit.set_value(OffsetUnit::Minutes);
        // Every entry starts ticked: the program's own list is the default.
        let fan = program
            .steps
            .iter()
            .filter_map(|s| {
                let entries = s.fan_out.as_ref()?;
                Some(FanStep {
                    key: s.key.clone(),
                    title: s.title.clone(),
                    entries: entries
                        .iter()
                        .map(|e| {
                            let mut c = CheckboxState::named(e);
                            c.set_checked(true);
                            (e.clone(), c)
                        })
                        .collect(),
                })
            })
            .collect();
        Self {
            program,
            column,
            column_name,
            tz,
            start,
            in_amount,
            in_unit,
            fan,
            set_btn: ButtonState::new(),
            start_btn: ButtonState::new(),
            cancel_btn: ButtonState::new(),
            error: None,
            saving: false,
        }
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        b.widget(&self.start)
            .widget(&self.in_amount)
            .widget(&self.in_unit)
            .widget(&self.set_btn);
        for step in &self.fan {
            for (_, c) in &step.entries {
                b.widget(c);
            }
        }
        b.widget(&self.start_btn).widget(&self.cancel_btn);
        b.build()
    }

    pub fn handle(&mut self, ev: &Event) -> StartOutcome {
        if self.saving {
            return StartOutcome::Continue;
        }
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        match key {
            Some(KeyCode::Esc) if !self.in_unit.is_popup_active() => return StartOutcome::Cancel,
            Some(KeyCode::F(2)) => return self.try_start(),
            _ => {}
        }
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return StartOutcome::Changed;
        }
        if self.start_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || (key == Some(KeyCode::Enter) && self.start.is_focused())
        {
            return self.try_start();
        }
        if self.cancel_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return StartOutcome::Cancel;
        }
        if self.set_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || (key == Some(KeyCode::Enter) && self.in_amount.is_focused())
        {
            match self.in_amount.text().trim().parse::<u32>() {
                Ok(amount) => {
                    let at = Offset {
                        amount,
                        unit: self.in_unit.value(),
                    }
                    .after(Utc::now());
                    if let Some(at) = at {
                        self.start.set_text(stamp(at, self.tz));
                        self.error = None;
                    }
                }
                Err(_) => self.error = Some("\"in\" needs a whole number".into()),
            }
            return StartOutcome::Changed;
        }
        self.start.handle(ev, Regular);
        self.in_amount.handle(ev, Regular);
        self.in_unit.handle(ev, Regular);
        for step in &mut self.fan {
            for (_, c) in &mut step.entries {
                c.handle(ev, Regular);
            }
        }
        StartOutcome::Changed
    }

    /// The fan-out lists as ticked. A step with everything unticked would
    /// make nothing, which is not what dropping an entry means.
    fn fan_out(&self) -> Result<Vec<(String, Vec<String>)>, String> {
        let mut out = Vec::new();
        for step in &self.fan {
            let kept: Vec<String> = step
                .entries
                .iter()
                .filter(|(_, c)| c.checked())
                .map(|(e, _)| e.clone())
                .collect();
            if kept.is_empty() {
                return Err(format!("step {} needs at least one entry ticked", step.key));
            }
            out.push((step.key.clone(), kept));
        }
        Ok(out)
    }

    fn try_start(&mut self) -> StartOutcome {
        let fan_out = match self.fan_out() {
            Ok(f) => f,
            Err(e) => {
                self.error = Some(e);
                return StartOutcome::Changed;
            }
        };
        match parse_when(self.start.text().trim(), self.tz) {
            Ok(start_at) => {
                self.error = None;
                StartOutcome::Start {
                    program_id: self.program.id,
                    column_id: self.column,
                    start_at,
                    fan_out,
                }
            }
            Err(e) => {
                self.error = Some(e);
                StartOutcome::Changed
            }
        }
    }

    /// Rows the fan-out lists take at this width: a heading per step and
    /// as many rows of checkboxes as its entries need.
    fn fan_rows(&self, width: usize) -> u16 {
        self.fan
            .iter()
            .map(|s| {
                let mut rows = 1;
                let mut x = 0;
                for (e, _) in &s.entries {
                    let cw = e.chars().count() + 5;
                    if x + cw > width && x > 0 {
                        rows += 1;
                        x = 0;
                    }
                    x += cw;
                }
                rows + 1
            })
            .sum()
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let bh = button_h(t);
        let pad = if t.touch { 2 } else { 0 };
        let lw = 10u16;
        let fan_h = self.fan_rows(68usize.saturating_sub(lw as usize));
        let p = popup(area, 70, 11 + bh + fan_h);
        f.render_widget(Clear, p);
        let hint = if self.saving {
            " starting... "
        } else {
            " Enter starts   Esc cancels "
        };
        let block = frame_block(" Start program ", hint, t);
        let inner = block.inner(p);
        f.render_widget(block, p);
        let [name, desc, _, start_row, in_row, fan_area, _, note, err, buttons] =
            Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(fan_h),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(bh),
            ])
            .areas(inner);
        // The fan-out lists, one heading and a row of boxes per step.
        let mut y = fan_area.y;
        for step in &mut self.fan {
            if y >= fan_area.bottom() {
                break;
            }
            let (l, w) = split_label(Rect::new(fan_area.x, y, fan_area.width, 1), lw);
            label(f, l, &step.key, t);
            f.render_widget(
                Paragraph::new(format!("{}: untick what this run leaves out", step.title))
                    .style(t.surface_dim()),
                w,
            );
            y += 1;
            let mut x = w.x;
            for (e, c) in &mut step.entries {
                let cw = super::check_w(e) + 1;
                if x + cw > w.right() && x > w.x {
                    x = w.x;
                    y += 1;
                }
                if y >= fan_area.bottom() {
                    break;
                }
                let cb = Rect::new(x, y, cw.min(w.right().saturating_sub(x)), 1);
                f.render_stateful_widget(checkbox_at(e.clone(), cb, t), cb, c);
                x += cw;
            }
            y += 1;
        }
        f.render_widget(
            Paragraph::new(format!(
                "{}, into {}",
                self.program.name, self.column_name
            ))
            .style(t.surface()),
            name,
        );
        f.render_widget(
            Paragraph::new(self.program.description.clone()).style(t.surface_dim()),
            desc,
        );
        let (l, w) = split_label(start_row, lw);
        label(f, l, "Start at", t);
        let mut r = Row::new(w);
        f.render_stateful_widget(field(t), r.take(17), &mut self.start);
        f.render_widget(
            Paragraph::new("YYYY-MM-DD HH:MM, empty for no date").style(t.surface_dim()),
            r.rest(),
        );
        let (l, w) = split_label(in_row, lw);
        label(f, l, "or in", t);
        let mut r = Row::new(w);
        f.render_stateful_widget(field(t), r.take(6), &mut self.in_amount);
        let unit_area = r.take(11);
        let (unit_w, unit_popup) = dropdown(OffsetUnit::ALL.map(|u| (u, u.label())), unit_area, t);
        f.render_stateful_widget(unit_w, unit_area, &mut self.in_unit);
        dropdown_marker(f, &self.in_unit, t);
        let sb = r.take(super::button_w(" Set ") + pad);
        render_button(f, sb, " Set ", &mut self.set_btn, t);
        f.render_widget(
            Paragraph::new("nothing runs until the root is moved to doing or its start code is scanned")
                .style(t.surface_dim()),
            note,
        );
        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), err);
        }
        let (start, cancel) = button_row(buttons, " Start ", " Cancel ", t);
        render_button(f, start, " Start ", &mut self.start_btn, t);
        render_button(f, cancel, " Cancel ", &mut self.cancel_btn, t);
        f.render_stateful_widget(unit_popup, unit_area, &mut self.in_unit);
        dropdown_popup_hover(f, &self.in_unit, t);
        if let Some(pos) = [self.start.screen_cursor(), self.in_amount.screen_cursor()]
            .into_iter()
            .flatten()
            .next()
        {
            f.set_cursor_position(pos);
        }
    }
}

fn stamp(at: DateTime<Utc>, tz: Tz) -> String {
    at.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string()
}

/// The same reading the task form gives a date: with a time, or a bare
/// date at nine, in the user's zone; empty is no date.
fn parse_when(text: &str, tz: Tz) -> Result<Option<DateTime<Utc>>, String> {
    if text.is_empty() {
        return Ok(None);
    }
    let naive = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M")
        .or_else(|_| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .map(|d| d.and_hms_opt(9, 0, 0).unwrap_or_default())
        })
        .map_err(|_| "the start must look like 2026-09-07 14:30".to_string())?;
    let local = tz
        .from_local_datetime(&naive)
        .earliest()
        .ok_or_else(|| "that time does not exist in your timezone".to_string())?;
    Ok(Some(local.with_timezone(&Utc)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use taskologic_core::ids::BoardId;

    fn program() -> Program {
        Program {
            min_samples: 3,
            id: ProgramId(3),
            board_id: BoardId(1),
            owner_uid: 1,
            name: "Clean up".into(),
            description: String::new(),
            steps: Vec::new(),
        }
    }

    #[test]
    fn the_helper_fills_the_date_and_enter_starts_with_it() {
        let mut form = StartProgramForm::new(program(), ColumnId(1), "Todo".into(), chrono_tz::UTC);
        // Tab from the start field: the amount, then the unit, then Set.
        for _ in 0..3 {
            form.handle(&Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        }
        assert!(form.set_btn.is_focused());
        form.handle(&Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        match form.try_start() {
            StartOutcome::Start {
                program_id,
                column_id,
                start_at,
                fan_out,
            } => {
                assert_eq!((program_id, column_id), (ProgramId(3), ColumnId(1)));
                let ahead = (start_at.unwrap() - Utc::now()).num_minutes();
                assert!((29..=30).contains(&ahead), "{ahead} minutes ahead");
                assert!(fan_out.is_empty(), "nothing fans out here");
            }
            other => panic!("{other:?}"),
        }
        form.start.set_text(String::new());
        assert!(matches!(form.try_start(), StartOutcome::Start { start_at: None, .. }));
        form.start.set_text("soon".to_string());
        assert!(matches!(form.try_start(), StartOutcome::Changed));
        assert!(form.error.is_some());
    }

    #[test]
    fn the_fan_out_lists_start_ticked_and_cannot_all_be_dropped() {
        use taskologic_core::program::Step;
        let mut p = program();
        p.steps = vec![Step {
            key: "3".into(),
            title: "Rooms".into(),
            fan_out: Some(vec!["bedroom".into(), "kitchen".into()]),
            ..Default::default()
        }];
        let mut form = StartProgramForm::new(p, ColumnId(1), "Todo".into(), chrono_tz::UTC);
        assert_eq!(
            form.fan_out().unwrap(),
            vec![("3".to_string(), vec!["bedroom".to_string(), "kitchen".to_string()])]
        );
        form.fan[0].entries[0].1.set_checked(false);
        assert_eq!(form.fan_out().unwrap()[0].1, vec!["kitchen".to_string()]);
        form.fan[0].entries[1].1.set_checked(false);
        assert!(form.fan_out().unwrap_err().contains("at least one entry"));
        assert!(form.fan_rows(60) >= 2, "a heading and a row of boxes");
    }
}
