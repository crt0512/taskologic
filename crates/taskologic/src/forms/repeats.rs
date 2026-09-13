//! The repeating tasks list: every active repetition on the board, its rule
//! and when it fires next, with a button to stop it. Any member may stop
//! one; the daemon records who did.

use chrono_tz::Tz;
use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Clear, ListItem, Paragraph};
use taskologic_core::ids::{BoardId, TaskId};
use taskologic_proto::RepeatEntry;

use super::{button_bar, button_h, frame_block, popup};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, Focus, FocusBuilder, HandleEvent, HasFocus, ListState, Outcome,
    Regular, list, render_button,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum RepeatsOutcome {
    Changed,
    Cancel,
    Stop(TaskId),
}

pub struct RepeatsPanel {
    pub board_id: BoardId,
    tz: Tz,
    entries: Vec<RepeatEntry>,
    list: ListState,
    stop_btn: ButtonState,
    close_btn: ButtonState,
    pub error: Option<String>,
}

impl RepeatsPanel {
    pub fn new(board_id: BoardId, tz: Tz) -> Self {
        let list = ListState::named("repeats");
        list.focus().set(true);
        Self {
            board_id,
            tz,
            entries: Vec::new(),
            list,
            stop_btn: ButtonState::new(),
            close_btn: ButtonState::new(),
            error: None,
        }
    }

    pub fn set_entries(&mut self, entries: Vec<RepeatEntry>) {
        self.entries = entries;
        self.clamp();
    }

    /// A repetition was stopped, drop its row.
    pub fn remove(&mut self, id: TaskId) {
        self.entries.retain(|e| e.task.id != id);
        self.clamp();
    }

    fn clamp(&mut self) {
        let sel = self.list.selected().unwrap_or(0);
        self.list.select(if self.entries.is_empty() {
            None
        } else {
            Some(sel.min(self.entries.len() - 1))
        });
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        b.widget(&self.list)
            .widget(&self.stop_btn)
            .widget(&self.close_btn);
        b.build()
    }

    pub fn handle(&mut self, ev: &Event) -> RepeatsOutcome {
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        if matches!(key, Some(KeyCode::Esc | KeyCode::Char('q'))) {
            return RepeatsOutcome::Cancel;
        }
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return RepeatsOutcome::Changed;
        }
        if self.close_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return RepeatsOutcome::Cancel;
        }
        let stop = self.stop_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || (key == Some(KeyCode::Enter) && self.list.is_focused());
        if stop {
            return match self.list.selected().and_then(|i| self.entries.get(i)) {
                Some(e) => RepeatsOutcome::Stop(e.task.id),
                None => RepeatsOutcome::Changed,
            };
        }
        self.list.handle(ev, Regular);
        RepeatsOutcome::Changed
    }

    fn line(&self, e: &RepeatEntry) -> String {
        let rule = e
            .task
            .repeat
            .as_ref()
            .map(|r| r.summary())
            .unwrap_or_default();
        let next = match e.next_fire_at {
            Some(n) => format!(
                "next {}",
                n.with_timezone(&self.tz).format("%Y-%m-%d %H:%M")
            ),
            None => "nothing scheduled".to_string(),
        };
        format!("{:<24} {rule}, {next}", e.task.title)
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let p = popup(area, 74, 16);
        f.render_widget(Clear, p);
        let block = frame_block(
            " Repeating tasks ",
            " Enter stops the selected one   Esc closes ",
            t,
        );
        let inner = block.inner(p);
        f.render_widget(block, p);
        let [l, note, err, buttons] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(button_h(t)),
        ])
        .areas(inner);
        let items: Vec<ListItem> = if self.entries.is_empty() {
            vec![ListItem::new("no repeating tasks on this board").style(t.surface_dim())]
        } else {
            self.entries
                .iter()
                .map(|e| ListItem::new(self.line(e)))
                .collect()
        };
        f.render_stateful_widget(list(items, t), l, &mut self.list);
        f.render_widget(
            Paragraph::new("a repeat only spawns the next copy once the current one is finished")
                .style(t.surface_dim()),
            note,
        );
        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), err);
        }
        let labels = [" Stop repetition ", " Close "];
        let rects = button_bar(buttons, &labels, t);
        if let Some(r) = rects.first() {
            render_button(f, *r, labels[0], &mut self.stop_btn, t);
        }
        if let Some(r) = rects.get(1) {
            render_button(f, *r, labels[1], &mut self.close_btn, t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};
    use taskologic_core::board::test_support::board_with_members;
    use taskologic_core::repeat::{Repeat, RepeatSpec};
    use taskologic_core::task::test_support::task_on;

    #[test]
    fn rows_show_the_rule_and_the_local_next_fire_time() {
        let board = board_with_members(1, &[1]);
        let mut task = task_on(&board, 1);
        task.title = "Water plants".into();
        task.repeat = Some(RepeatSpec {
            rule: Repeat::EveryDays {
                every: 2,
                from: NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
            },
            at: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            tz: chrono_tz::Europe::Berlin,
            start_rule: None,
            due_rule: None,
        });
        let mut p = RepeatsPanel::new(board.id, chrono_tz::Europe::Berlin);
        let next = Some(Utc.with_ymd_and_hms(2026, 9, 7, 7, 0, 0).unwrap());
        let id = task.id;
        p.set_entries(vec![RepeatEntry {
            task,
            next_fire_at: next,
        }]);
        let line = p.line(&p.entries[0]);
        assert!(line.contains("every 2 days at 09:00"), "{line}");
        assert!(
            line.contains("next 2026-09-07 09:00"),
            "local time, not UTC: {line}"
        );
        p.remove(id);
        assert!(p.entries.is_empty());
    }
}
