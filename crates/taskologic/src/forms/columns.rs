//! Column management for one board: add, rename, reorder, assign roles,
//! remove. Removal needs answers (where do the tasks go, who takes over a
//! role), which the app collects after this panel says `Remove`.

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Clear, ListItem, Paragraph};
use taskologic_core::board::{Board, Column, ColumnRole};
use taskologic_core::ids::ColumnId;

use super::{button_bar, button_h, frame_block, popup};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, Focus, FocusBuilder, HandleEvent, HasFocus, HasScreenCursor,
    ListState, Outcome, Regular, TextInputState, field, list, render_button,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum ColumnsOutcome {
    Changed,
    Cancel,
    Add(String),
    Rename(ColumnId, String),
    Reorder(Vec<ColumnId>),
    SetRole(ColumnRole, ColumnId),
    Remove(ColumnId),
}

enum Editing {
    Add,
    Rename(ColumnId),
}

pub struct ColumnsPanel {
    pub board: Board,
    list: ListState,
    input: TextInputState,
    editing: Option<Editing>,
    buttons: [ButtonState; 7],
    pub error: Option<String>,
}

impl ColumnsPanel {
    pub fn new(board: &Board) -> Self {
        let mut list = ListState::named("columns");
        list.focus().set(true);
        list.select(if board.columns.is_empty() {
            None
        } else {
            Some(0)
        });
        Self {
            board: board.clone(),
            list,
            input: TextInputState::named("column_name"),
            editing: None,
            buttons: std::array::from_fn(|_| ButtonState::new()),
            error: None,
        }
    }

    pub fn refresh(&mut self, board: &Board) {
        self.board = board.clone();
        let sel = self.list.selected().unwrap_or(0);
        self.list.select(if board.columns.is_empty() {
            None
        } else {
            Some(sel.min(board.columns.len() - 1))
        });
    }

    fn selected(&self) -> Option<&Column> {
        self.list.selected().and_then(|i| self.board.columns.get(i))
    }

    fn start_editing(&mut self, editing: Editing, text: &str) {
        self.input.set_text(text.to_string());
        self.editing = Some(editing);
        self.list.focus().set(false);
        self.input.focus().set(true);
        self.error = None;
    }

    fn stop_editing(&mut self) {
        self.editing = None;
        self.input.focus().set(false);
        self.list.focus().set(true);
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        b.widget(&self.list);
        if self.editing.is_some() {
            b.widget(&self.input);
        }
        for s in &self.buttons {
            b.widget(s);
        }
        b.build()
    }

    /// The button bar, in order. Each one does what its key does.
    const ACTIONS: [(&'static str, char); 7] = [
        (" Add ", 'a'),
        (" Rename ", 'r'),
        (" Remove ", 'd'),
        (" Up ", 'K'),
        (" Down ", 'J'),
        (" Role ", 's'),
        (" Close ", '\u{1b}'),
    ];

    pub fn handle(&mut self, ev: &Event) -> ColumnsOutcome {
        // The ring first: a button ignores the mouse until the ring marks it
        // as the thing under the pointer.
        let ring_moved = {
            let mut focus = self.focus();
            focus.handle(ev, Regular) == Outcome::Changed
        };
        // A button press stands in for its key, so both paths share one
        // implementation and cannot drift apart.
        let mut pressed = None;
        for (i, state) in self.buttons.iter_mut().enumerate() {
            if state.handle(ev, Regular) == ButtonOutcome::Pressed {
                pressed = Self::ACTIONS.get(i).map(|(_, k)| *k);
            }
        }
        if let Some(key) = pressed {
            if key == '\u{1b}' {
                return ColumnsOutcome::Cancel;
            }
            let modifiers = if key.is_uppercase() {
                KeyModifiers::SHIFT
            } else {
                KeyModifiers::NONE
            };
            let synthetic = Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char(key),
                modifiers,
            ));
            return self.handle(&synthetic);
        }
        if ring_moved && matches!(ev, Event::Key(_)) {
            return ColumnsOutcome::Changed;
        }
        let (code, shift) = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                (Some(k.code), k.modifiers.contains(KeyModifiers::SHIFT))
            }
            _ => (None, false),
        };
        if self.editing.is_some() {
            match code {
                Some(KeyCode::Esc) => {
                    self.stop_editing();
                    return ColumnsOutcome::Changed;
                }
                Some(KeyCode::Enter) => {
                    let name = self.input.text().trim().to_string();
                    if name.is_empty() {
                        self.error = Some("column names cannot be empty".into());
                        return ColumnsOutcome::Changed;
                    }
                    let editing = self.editing.take();
                    self.stop_editing();
                    return match editing {
                        Some(Editing::Add) => ColumnsOutcome::Add(name),
                        Some(Editing::Rename(id)) => ColumnsOutcome::Rename(id, name),
                        None => ColumnsOutcome::Changed,
                    };
                }
                _ => {
                    self.input.handle(ev, Regular);
                    return ColumnsOutcome::Changed;
                }
            }
        }
        let sel = self.selected().cloned();
        match code {
            Some(KeyCode::Esc) => return ColumnsOutcome::Cancel,
            Some(KeyCode::Char('a')) => {
                self.start_editing(Editing::Add, "");
                return ColumnsOutcome::Changed;
            }
            Some(KeyCode::Char('r')) => {
                if let Some(c) = sel {
                    self.start_editing(Editing::Rename(c.id), &c.name);
                }
                return ColumnsOutcome::Changed;
            }
            Some(KeyCode::Char('d')) => {
                if self.board.columns.len() <= 1 {
                    self.error = Some("a board needs at least one column".into());
                    return ColumnsOutcome::Changed;
                }
                if let Some(c) = sel {
                    return ColumnsOutcome::Remove(c.id);
                }
                return ColumnsOutcome::Changed;
            }
            Some(KeyCode::Char('s')) => {
                if let Some(c) = sel {
                    return ColumnsOutcome::SetRole(ColumnRole::Started, c.id);
                }
            }
            Some(KeyCode::Char('p')) => {
                if let Some(c) = sel {
                    return ColumnsOutcome::SetRole(ColumnRole::Paused, c.id);
                }
            }
            Some(KeyCode::Char('f')) => {
                if let Some(c) = sel {
                    return ColumnsOutcome::SetRole(ColumnRole::Finished, c.id);
                }
            }
            Some(KeyCode::Up | KeyCode::Char('K')) if shift || code == Some(KeyCode::Char('K')) => {
                if let Some(i) = self.list.selected()
                    && i > 0
                {
                    let mut order: Vec<ColumnId> =
                        self.board.columns.iter().map(|c| c.id).collect();
                    order.swap(i, i - 1);
                    self.list.select(Some(i - 1));
                    return ColumnsOutcome::Reorder(order);
                }
                return ColumnsOutcome::Changed;
            }
            Some(KeyCode::Down | KeyCode::Char('J'))
                if shift || code == Some(KeyCode::Char('J')) =>
            {
                if let Some(i) = self.list.selected()
                    && i + 1 < self.board.columns.len()
                {
                    let mut order: Vec<ColumnId> =
                        self.board.columns.iter().map(|c| c.id).collect();
                    order.swap(i, i + 1);
                    self.list.select(Some(i + 1));
                    return ColumnsOutcome::Reorder(order);
                }
                return ColumnsOutcome::Changed;
            }
            _ => {}
        }
        self.list.handle(ev, Regular);
        ColumnsOutcome::Changed
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let p = popup(area, 66, 16);
        f.render_widget(Clear, p);
        let block = frame_block(
            " Columns ",
            " a add   r rename   d remove   Shift+Up/Down moves   s/p/f sets a role ",
            t,
        );
        let inner = block.inner(p);
        f.render_widget(block, p);
        let [l, edit, err, buttons] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(button_h(t)),
        ])
        .areas(inner);
        let board = &self.board;
        let items: Vec<ListItem> = board
            .columns
            .iter()
            .map(|c| {
                let roles: Vec<&str> = board
                    .roles_of(c.id)
                    .into_iter()
                    .map(|r| r.label())
                    .collect();
                let tag = if roles.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", roles.join(", "))
                };
                ListItem::new(format!("{}{tag}", c.name))
            })
            .collect();
        f.render_stateful_widget(list(items, t), l, &mut self.list);
        match &self.editing {
            Some(kind) => {
                let text = match kind {
                    Editing::Add => "New name: ",
                    Editing::Rename(_) => "Rename to: ",
                };
                let mut r = super::Row::new(edit);
                f.render_widget(Paragraph::new(text).style(t.surface_dim()), r.text(text));
                f.render_stateful_widget(field(t), r.rest(), &mut self.input);
            }
            None => f.render_widget(Paragraph::new("Esc closes").style(t.surface_dim()), edit),
        }
        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), err);
        }
        let labels: Vec<&str> = Self::ACTIONS.iter().map(|(l, _)| *l).collect();
        let rects = button_bar(buttons, &labels, t);
        for ((rect, label), state) in rects.iter().zip(labels.iter()).zip(self.buttons.iter_mut()) {
            render_button(f, *rect, label, state, t);
        }
        if let Some(p) = self.input.screen_cursor() {
            f.set_cursor_position(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use taskologic_core::board::test_support::board_with_members;

    fn key(code: KeyCode, m: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, m))
    }

    #[test]
    fn add_rename_role_and_reorder() {
        let board = board_with_members(1, &[1]);
        let mut panel = ColumnsPanel::new(&board);
        panel.handle(&key(KeyCode::Char('a'), KeyModifiers::NONE));
        for c in "Review".chars() {
            panel.handle(&key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(
            panel.handle(&key(KeyCode::Enter, KeyModifiers::NONE)),
            ColumnsOutcome::Add("Review".into())
        );
        assert_eq!(
            panel.handle(&key(KeyCode::Char('f'), KeyModifiers::NONE)),
            ColumnsOutcome::SetRole(ColumnRole::Finished, ColumnId(1))
        );
        assert_eq!(
            panel.handle(&key(KeyCode::Down, KeyModifiers::SHIFT)),
            ColumnsOutcome::Reorder(vec![ColumnId(2), ColumnId(1), ColumnId(3), ColumnId(4)])
        );
        // The selection followed the move, so this is the second column now.
        assert_eq!(
            panel.handle(&key(KeyCode::Char('d'), KeyModifiers::NONE)),
            ColumnsOutcome::Remove(ColumnId(2))
        );
        assert_eq!(
            panel.handle(&key(KeyCode::Esc, KeyModifiers::NONE)),
            ColumnsOutcome::Cancel
        );
    }
}
