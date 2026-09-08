//! Member management for one board: who is on it, who can be added.

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Block, Borders, Clear, ListItem, Paragraph};
use taskologic_core::board::Board;
use taskologic_core::ids::{BoardId, Uid};
use taskologic_core::user::UserSummary;

use super::{button_bar, button_h, frame_block, popup};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, Focus, FocusBuilder, HandleEvent, HasFocus, ListState, Outcome,
    Regular, list, render_button,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum MembersOutcome {
    Changed,
    Cancel,
    Add(Uid),
    Remove(Uid),
}

pub struct MembersPanel {
    pub board_id: BoardId,
    owner: Uid,
    privileged: bool,
    members: Vec<UserSummary>,
    all_users: Vec<UserSummary>,
    candidates: Vec<UserSummary>,
    members_list: ListState,
    add_list: ListState,
    add_btn: ButtonState,
    remove_btn: ButtonState,
    close_btn: ButtonState,
    pub error: Option<String>,
}

impl MembersPanel {
    pub fn new(board: &Board, privileged: bool) -> Self {
        let members_list = ListState::named("members");
        members_list.focus().set(true);
        Self {
            board_id: board.id,
            owner: board.owner_uid,
            privileged,
            members: Vec::new(),
            all_users: Vec::new(),
            candidates: Vec::new(),
            members_list,
            add_list: ListState::named("candidates"),
            add_btn: ButtonState::new(),
            remove_btn: ButtonState::new(),
            close_btn: ButtonState::new(),
            error: None,
        }
    }

    pub fn set_members(&mut self, members: Vec<UserSummary>) {
        self.members = members;
        self.recompute();
    }

    pub fn set_users(&mut self, users: Vec<UserSummary>) {
        self.all_users = users;
        self.recompute();
    }

    fn recompute(&mut self) {
        self.candidates = self
            .all_users
            .iter()
            .filter(|u| !self.members.iter().any(|m| m.uid == u.uid))
            .cloned()
            .collect();
        let clamp = |list: &mut ListState, len: usize| {
            let sel = list.selected().unwrap_or(0);
            list.select(if len == 0 {
                None
            } else {
                Some(sel.min(len - 1))
            });
        };
        clamp(&mut self.members_list, self.members.len());
        clamp(&mut self.add_list, self.candidates.len());
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        b.widget(&self.members_list).widget(&self.add_list);
        if self.privileged {
            b.widget(&self.add_btn).widget(&self.remove_btn);
        }
        b.widget(&self.close_btn);
        b.build()
    }

    fn selected_member(&self) -> Option<Uid> {
        self.members_list
            .selected()
            .and_then(|i| self.members.get(i))
            .map(|u| u.uid)
    }

    fn selected_candidate(&self) -> Option<Uid> {
        self.add_list
            .selected()
            .and_then(|i| self.candidates.get(i))
            .map(|u| u.uid)
    }

    /// Shared by the key and the button: adding needs the right to manage.
    fn add(&mut self, uid: Option<Uid>) -> MembersOutcome {
        if !self.privileged {
            self.error = Some("only the board owner or an admin can manage members".into());
            return MembersOutcome::Changed;
        }
        match uid {
            Some(uid) => MembersOutcome::Add(uid),
            None => MembersOutcome::Changed,
        }
    }

    fn remove(&mut self, uid: Option<Uid>) -> MembersOutcome {
        if !self.privileged {
            self.error = Some("only the board owner or an admin can manage members".into());
            return MembersOutcome::Changed;
        }
        match uid {
            Some(uid) if uid == self.owner => {
                self.error = Some("the owner is always a member".into());
                MembersOutcome::Changed
            }
            Some(uid) => MembersOutcome::Remove(uid),
            None => MembersOutcome::Changed,
        }
    }

    pub fn handle(&mut self, ev: &Event) -> MembersOutcome {
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        if key == Some(KeyCode::Esc) {
            return MembersOutcome::Cancel;
        }
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return MembersOutcome::Changed;
        }
        if self.add_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            let uid = self.selected_candidate();
            return self.add(uid);
        }
        if self.remove_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            let uid = self.selected_member();
            return self.remove(uid);
        }
        if self.close_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return MembersOutcome::Cancel;
        }
        if key == Some(KeyCode::Enter) && self.add_list.is_focused() {
            let uid = self.selected_candidate();
            return self.add(uid);
        }
        if matches!(
            key,
            Some(KeyCode::Delete | KeyCode::Backspace | KeyCode::Char('x'))
        ) && self.members_list.is_focused()
        {
            let uid = self.selected_member();
            return self.remove(uid);
        }
        self.members_list.handle(ev, Regular);
        self.add_list.handle(ev, Regular);
        MembersOutcome::Changed
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let p = popup(area, 70, 18);
        f.render_widget(Clear, p);
        let hint = if self.privileged {
            " Tab moves   Enter adds   x removes   Esc closes "
        } else {
            " Esc closes "
        };
        let block = frame_block(" Members ", hint, t);
        let inner = block.inner(p);
        f.render_widget(block, p);
        let bh = button_h(t);
        let [lists, err, buttons] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(bh),
        ])
        .areas(inner);
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(lists);

        let owner = self.owner;
        let items: Vec<ListItem> = if self.members.is_empty() {
            vec![ListItem::new("loading...").style(t.surface_dim())]
        } else {
            self.members
                .iter()
                .map(|m| {
                    let tag = if m.uid == owner {
                        " (owner)"
                    } else if m.is_admin {
                        " (admin)"
                    } else {
                        ""
                    };
                    ListItem::new(format!("{}{tag}", m.username))
                })
                .collect()
        };
        let lb = Block::default()
            .borders(Borders::ALL)
            .border_set(t.border_set())
            .border_style(t.surface_border())
            .title(" On the board ");
        f.render_stateful_widget(list(items, t).block(lb), left, &mut self.members_list);

        let items: Vec<ListItem> = if self.candidates.is_empty() {
            vec![ListItem::new("nobody left to add").style(t.surface_dim())]
        } else {
            self.candidates
                .iter()
                .map(|u| ListItem::new(u.username.clone()))
                .collect()
        };
        let rb = Block::default()
            .borders(Borders::ALL)
            .border_set(t.border_set())
            .border_style(t.surface_border())
            .title(" Can be added ");
        f.render_stateful_widget(list(items, t).block(rb), right, &mut self.add_list);

        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), err);
        }

        let labels: Vec<&str> = if self.privileged {
            vec![" Add ", " Remove ", " Close "]
        } else {
            vec![" Close "]
        };
        let rects = button_bar(buttons, &labels, t);
        let mut states: Vec<&mut ButtonState> = if self.privileged {
            vec![&mut self.add_btn, &mut self.remove_btn, &mut self.close_btn]
        } else {
            vec![&mut self.close_btn]
        };
        for ((rect, label), state) in rects.iter().zip(labels.iter()).zip(states.iter_mut()) {
            render_button(f, *rect, label, state, t);
        }
    }
}
