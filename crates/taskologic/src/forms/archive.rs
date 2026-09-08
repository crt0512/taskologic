//! The archive: finished tasks waiting to age out, and deleted ones waiting
//! to be purged. Restoring is open to anyone on the board, purging is not.

use chrono_tz::Tz;
use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Clear, ListItem, Paragraph};
use taskologic_core::ids::TaskId;
use taskologic_core::task::Task;

use super::{button_bar, button_h, frame_block, popup};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, Focus, FocusBuilder, HandleEvent, HasFocus, ListState, Outcome,
    Regular, list, render_button,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum ArchiveOutcome {
    Changed,
    Cancel,
    Restore(TaskId),
    View(Box<Task>),
    Purge(Box<Task>),
}

pub struct ArchivePanel {
    tasks: Vec<Task>,
    privileged: bool,
    tz: Tz,
    list: ListState,
    restore_btn: ButtonState,
    view_btn: ButtonState,
    purge_btn: ButtonState,
    close_btn: ButtonState,
}

impl ArchivePanel {
    pub fn new(tasks: Vec<Task>, privileged: bool, tz: Tz) -> Self {
        let list = ListState::named("archive");
        list.focus().set(true);
        let mut panel = Self {
            tasks,
            privileged,
            tz,
            list,
            restore_btn: ButtonState::new(),
            view_btn: ButtonState::new(),
            purge_btn: ButtonState::new(),
            close_btn: ButtonState::new(),
        };
        panel.clamp();
        panel
    }

    fn clamp(&mut self) {
        let sel = self.list.selected().unwrap_or(0);
        self.list.select(if self.tasks.is_empty() {
            None
        } else {
            Some(sel.min(self.tasks.len() - 1))
        });
    }

    /// An event elsewhere changed the archive under us.
    pub fn upsert(&mut self, task: Task) {
        match self.tasks.iter_mut().find(|t| t.id == task.id) {
            Some(slot) => *slot = task,
            None => self.tasks.insert(0, task),
        }
        self.clamp();
    }

    pub fn remove(&mut self, id: TaskId) {
        self.tasks.retain(|t| t.id != id);
        self.clamp();
    }

    fn selected(&self) -> Option<&Task> {
        self.list.selected().and_then(|i| self.tasks.get(i))
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        b.widget(&self.list)
            .widget(&self.restore_btn)
            .widget(&self.view_btn);
        if self.privileged {
            b.widget(&self.purge_btn);
        }
        b.widget(&self.close_btn);
        b.build()
    }

    fn labels(&self) -> Vec<&'static str> {
        if self.privileged {
            vec![" Restore ", " View ", " Delete for good ", " Close "]
        } else {
            vec![" Restore ", " View ", " Close "]
        }
    }

    pub fn handle(&mut self, ev: &Event) -> ArchiveOutcome {
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        if matches!(key, Some(KeyCode::Esc | KeyCode::Char('q'))) {
            return ArchiveOutcome::Cancel;
        }
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return ArchiveOutcome::Changed;
        }
        if self.close_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return ArchiveOutcome::Cancel;
        }
        let restore = self.restore_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || (key == Some(KeyCode::Enter) && self.list.is_focused());
        if restore {
            return match self.selected() {
                Some(t) => ArchiveOutcome::Restore(t.id),
                None => ArchiveOutcome::Changed,
            };
        }
        let view = self.view_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || key == Some(KeyCode::Char('v'));
        if view {
            return match self.selected() {
                Some(t) => ArchiveOutcome::View(Box::new(t.clone())),
                None => ArchiveOutcome::Changed,
            };
        }
        let purge = self.purge_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || key == Some(KeyCode::Char('D'));
        if purge && self.privileged {
            return match self.selected() {
                Some(t) => ArchiveOutcome::Purge(Box::new(t.clone())),
                None => ArchiveOutcome::Changed,
            };
        }
        self.list.handle(ev, Regular);
        ArchiveOutcome::Changed
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let p = popup(area, 72, 18);
        f.render_widget(Clear, p);
        let block = frame_block(" Archive ", " Enter restores   v views   Esc closes ", t);
        let inner = block.inner(p);
        f.render_widget(block, p);
        let [l, buttons] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(button_h(t))]).areas(inner);

        if self.tasks.is_empty() {
            f.render_widget(
                Paragraph::new("Nothing archived.").style(t.surface_dim()),
                l,
            );
        }
        let tz = self.tz;
        let items: Vec<ListItem> = self
            .tasks
            .iter()
            .map(|task| {
                let when = task
                    .archived_at
                    .map(|a| a.with_timezone(&tz).format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                let how = if task.is_deleted() {
                    "deleted "
                } else {
                    "finished"
                };
                ListItem::new(format!(" {when} {how}  {}", task.title))
            })
            .collect();
        f.render_stateful_widget(list(items, t), l, &mut self.list);

        let labels = self.labels();
        let rects = button_bar(buttons, &labels, t);
        let mut states: Vec<&mut ButtonState> = if self.privileged {
            vec![
                &mut self.restore_btn,
                &mut self.view_btn,
                &mut self.purge_btn,
                &mut self.close_btn,
            ]
        } else {
            vec![
                &mut self.restore_btn,
                &mut self.view_btn,
                &mut self.close_btn,
            ]
        };
        for ((rect, label), state) in rects.iter().zip(labels.iter()).zip(states.iter_mut()) {
            render_button(f, *rect, label, state, t);
        }
    }
}
