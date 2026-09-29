//! The runs list: every program started on the board, how far each got,
//! and the two ways to stop one. Cancelling keeps the tasks the run made
//! unless asked to delete the open ones; finished ones always stay.

use std::collections::HashMap;

use chrono_tz::Tz;
use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Clear, ListItem, Paragraph};
use taskologic_core::ids::{BoardId, RunId, Uid};
use taskologic_core::task::Task;
use taskologic_proto::RunEntry;

use super::{button_bar, button_h, frame_block, popup};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, Focus, FocusBuilder, HandleEvent, HasFocus, ListState, Outcome,
    Regular, list, render_button,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum RunsOutcome {
    Changed,
    Cancel,
    /// Open the run's root task.
    View(Box<Task>),
    CancelRun { run_id: RunId, delete_open: bool },
}

pub struct RunsPanel {
    pub board_id: BoardId,
    tz: Tz,
    names: HashMap<Uid, String>,
    entries: Vec<RunEntry>,
    list: ListState,
    view_btn: ButtonState,
    keep_btn: ButtonState,
    delete_btn: ButtonState,
    close_btn: ButtonState,
    pub error: Option<String>,
}

impl RunsPanel {
    pub fn new(board_id: BoardId, tz: Tz, names: HashMap<Uid, String>) -> Self {
        let list = ListState::named("runs");
        list.focus().set(true);
        Self {
            board_id,
            tz,
            names,
            entries: Vec::new(),
            list,
            view_btn: ButtonState::new(),
            keep_btn: ButtonState::new(),
            delete_btn: ButtonState::new(),
            close_btn: ButtonState::new(),
            error: None,
        }
    }

    pub fn set_entries(&mut self, entries: Vec<RunEntry>) {
        self.entries = entries;
        let sel = self.list.selected().unwrap_or(0);
        self.list.select(if self.entries.is_empty() {
            None
        } else {
            Some(sel.min(self.entries.len() - 1))
        });
    }

    fn selected(&self) -> Option<&RunEntry> {
        self.list.selected().and_then(|i| self.entries.get(i))
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        b.widget(&self.list)
            .widget(&self.view_btn)
            .widget(&self.keep_btn)
            .widget(&self.delete_btn)
            .widget(&self.close_btn);
        b.build()
    }

    /// The selected run, if it can still be cancelled.
    fn cancellable(&mut self) -> Option<RunId> {
        let e = self.selected()?;
        if e.finished_at.is_some() || e.cancelled_at.is_some() {
            self.error = Some("this run is already over".into());
            return None;
        }
        Some(e.id)
    }

    pub fn handle(&mut self, ev: &Event) -> RunsOutcome {
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        if matches!(key, Some(KeyCode::Esc | KeyCode::Char('q'))) {
            return RunsOutcome::Cancel;
        }
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return RunsOutcome::Changed;
        }
        if self.close_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return RunsOutcome::Cancel;
        }
        let view = self.view_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || (key == Some(KeyCode::Enter) && self.list.is_focused());
        if view {
            return match self.selected() {
                Some(e) => RunsOutcome::View(Box::new(e.root.clone())),
                None => RunsOutcome::Changed,
            };
        }
        if self.keep_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return match self.cancellable() {
                Some(run_id) => RunsOutcome::CancelRun {
                    run_id,
                    delete_open: false,
                },
                None => RunsOutcome::Changed,
            };
        }
        if self.delete_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return match self.cancellable() {
                Some(run_id) => RunsOutcome::CancelRun {
                    run_id,
                    delete_open: true,
                },
                None => RunsOutcome::Changed,
            };
        }
        self.list.handle(ev, Regular);
        RunsOutcome::Changed
    }

    fn line(&self, e: &RunEntry) -> String {
        let state = if e.cancelled_at.is_some() {
            "cancelled"
        } else if e.finished_at.is_some() {
            "finished"
        } else if e.started_at.is_some() {
            "running"
        } else {
            "waiting"
        };
        let who = self
            .names
            .get(&e.started_by)
            .cloned()
            .unwrap_or_else(|| format!("uid {}", e.started_by));
        let when = e
            .created_at
            .with_timezone(&self.tz)
            .format("%Y-%m-%d %H:%M");
        format!(
            "{state:<9} {:<20} {}/{} steps  by {who}, {when}",
            clip(&e.program_name, 20),
            e.done_steps,
            e.done_steps + e.open_steps
        )
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let p = popup(area, 78, 18);
        f.render_widget(Clear, p);
        let block = frame_block(" Program runs ", " Enter opens the root task   Esc closes ", t);
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
            vec![ListItem::new("no program has been started on this board").style(t.surface_dim())]
        } else {
            self.entries
                .iter()
                .map(|e| ListItem::new(self.line(e)))
                .collect()
        };
        f.render_stateful_widget(list(items, t), l, &mut self.list);
        f.render_widget(
            Paragraph::new("cancelling stops the run; finished tasks always stay on the board")
                .style(t.surface_dim()),
            note,
        );
        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), err);
        }
        let labels = [
            " Open root ",
            " Cancel, keep tasks ",
            " Cancel, delete open tasks ",
            " Close ",
        ];
        let rects = button_bar(buttons, &labels, t);
        let states = [
            &mut self.view_btn,
            &mut self.keep_btn,
            &mut self.delete_btn,
            &mut self.close_btn,
        ];
        for ((r, label), state) in rects.iter().zip(labels).zip(states) {
            render_button(f, *r, label, state, t);
        }
    }
}

fn clip(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('~');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use taskologic_core::board::test_support::board_with_members;
    use taskologic_core::task::test_support::task_on;

    #[test]
    fn rows_say_where_a_run_stands_and_over_runs_cannot_be_cancelled() {
        let board = board_with_members(1, &[1]);
        let root = task_on(&board, 1);
        let entry = |started: bool, finished: bool| RunEntry {
            id: RunId(1),
            program_name: "Clean up".into(),
            root: root.clone(),
            started_by: 1,
            created_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
            started_at: started.then(|| chrono::DateTime::from_timestamp(60, 0).unwrap()),
            finished_at: finished.then(|| chrono::DateTime::from_timestamp(120, 0).unwrap()),
            cancelled_at: None,
            open_steps: 2,
            done_steps: 1,
        };
        let mut p = RunsPanel::new(board.id, chrono_tz::UTC, HashMap::from([(1, "alice".to_string())]));
        p.set_entries(vec![entry(false, false), entry(true, false), entry(true, true)]);
        assert!(p.line(&p.entries[0]).starts_with("waiting"));
        assert!(p.line(&p.entries[1]).starts_with("running"));
        let done = p.line(&p.entries[2]);
        assert!(done.starts_with("finished") && done.contains("1/3 steps") && done.contains("by alice"), "{done}");
        p.list.select(Some(2));
        assert_eq!(p.cancellable(), None);
        assert!(p.error.is_some());
        p.list.select(Some(1));
        assert_eq!(p.cancellable(), Some(RunId(1)));
    }
}
