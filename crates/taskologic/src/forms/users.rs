//! Admin panel: every known user and their admin flag.

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Clear, ListItem, Paragraph};
use taskologic_core::ids::Uid;
use taskologic_core::user::UserSummary;

use super::{button_bar, button_h, frame_block, popup};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, Focus, FocusBuilder, HandleEvent, HasFocus, ListState, Outcome,
    Regular, list, render_button,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum UsersOutcome {
    Changed,
    Cancel,
    SetAdmin { uid: Uid, is_admin: bool },
}

pub struct UsersPanel {
    me: Uid,
    users: Vec<UserSummary>,
    list: ListState,
    toggle_btn: ButtonState,
    close_btn: ButtonState,
    pub error: Option<String>,
}

impl UsersPanel {
    pub fn new(me: Uid) -> Self {
        let list = ListState::named("users");
        list.focus().set(true);
        Self {
            me,
            users: Vec::new(),
            list,
            toggle_btn: ButtonState::new(),
            close_btn: ButtonState::new(),
            error: None,
        }
    }

    pub fn set_users(&mut self, users: Vec<UserSummary>) {
        self.users = users;
        let sel = self.list.selected().unwrap_or(0);
        self.list.select(if self.users.is_empty() {
            None
        } else {
            Some(sel.min(self.users.len() - 1))
        });
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        b.widget(&self.list)
            .widget(&self.toggle_btn)
            .widget(&self.close_btn);
        b.build()
    }

    fn toggle(&mut self) -> UsersOutcome {
        match self.list.selected().and_then(|i| self.users.get(i)) {
            Some(u) if u.uid == self.me && u.is_admin => {
                self.error = Some("you cannot revoke your own admin flag".into());
                UsersOutcome::Changed
            }
            Some(u) => UsersOutcome::SetAdmin {
                uid: u.uid,
                is_admin: !u.is_admin,
            },
            None => UsersOutcome::Changed,
        }
    }

    pub fn handle(&mut self, ev: &Event) -> UsersOutcome {
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        if key == Some(KeyCode::Esc) {
            return UsersOutcome::Cancel;
        }
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return UsersOutcome::Changed;
        }
        if self.toggle_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return self.toggle();
        }
        if self.close_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return UsersOutcome::Cancel;
        }
        if matches!(key, Some(KeyCode::Enter | KeyCode::Char(' '))) && self.list.is_focused() {
            return self.toggle();
        }
        self.list.handle(ev, Regular);
        UsersOutcome::Changed
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let p = popup(area, 50, 18);
        f.render_widget(Clear, p);
        let block = frame_block(" Users ", " Enter toggles admin   Esc closes ", t);
        let inner = block.inner(p);
        f.render_widget(block, p);
        let [l, err, buttons] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(button_h(t)),
        ])
        .areas(inner);
        let items: Vec<ListItem> = if self.users.is_empty() {
            vec![ListItem::new("loading...").style(t.surface_dim())]
        } else {
            self.users
                .iter()
                .map(|u| {
                    ListItem::new(format!(
                        "{:<24} {}",
                        u.username,
                        if u.is_admin { "admin" } else { "" }
                    ))
                })
                .collect()
        };
        f.render_stateful_widget(list(items, t), l, &mut self.list);
        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), err);
        }
        let labels = [" Toggle admin ", " Close "];
        let rects = button_bar(buttons, &labels, t);
        if let Some(r) = rects.first() {
            render_button(f, *r, labels[0], &mut self.toggle_btn, t);
        }
        if let Some(r) = rects.get(1) {
            render_button(f, *r, labels[1], &mut self.close_btn, t);
        }
    }
}
