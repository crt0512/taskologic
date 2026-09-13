//! The template panel: saved task templates on one board. Use stamps a new
//! task from one, New and Edit open the task form in template mode, Delete
//! asks first. Managing follows the task rules; the daemon is the authority
//! and this panel only pre-checks to say why instead of a bare refusal.

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Clear, ListItem, Paragraph};
use taskologic_core::ids::{BoardId, Uid};
use taskologic_core::template::Template;

use super::{button_bar, button_h, frame_block, popup};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, Focus, FocusBuilder, HandleEvent, HasFocus, ListState, Outcome,
    Regular, list, render_button,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum TemplatesOutcome {
    Changed,
    Cancel,
    New,
    Use(Box<Template>),
    Edit(Box<Template>),
    Delete(Box<Template>),
}

pub struct TemplatesPanel {
    pub board_id: BoardId,
    me: Uid,
    /// Board owner or admin: may manage every template here.
    privileged: bool,
    /// Template plus its owner's name for the list line.
    templates: Vec<(Template, String)>,
    list: ListState,
    use_btn: ButtonState,
    new_btn: ButtonState,
    edit_btn: ButtonState,
    delete_btn: ButtonState,
    close_btn: ButtonState,
    pub error: Option<String>,
}

impl TemplatesPanel {
    pub fn new(board_id: BoardId, me: Uid, privileged: bool) -> Self {
        let list = ListState::named("templates");
        list.focus().set(true);
        Self {
            board_id,
            me,
            privileged,
            templates: Vec::new(),
            list,
            use_btn: ButtonState::new(),
            new_btn: ButtonState::new(),
            edit_btn: ButtonState::new(),
            delete_btn: ButtonState::new(),
            close_btn: ButtonState::new(),
            error: None,
        }
    }

    pub fn set_templates(&mut self, templates: Vec<Template>, name_of: &dyn Fn(Uid) -> String) {
        self.templates = templates
            .into_iter()
            .map(|t| {
                let n = name_of(t.owner_uid);
                (t, n)
            })
            .collect();
        let sel = self.list.selected().unwrap_or(0);
        self.list.select(if self.templates.is_empty() {
            None
        } else {
            Some(sel.min(self.templates.len() - 1))
        });
    }

    fn selected(&self) -> Option<&Template> {
        self.list
            .selected()
            .and_then(|i| self.templates.get(i))
            .map(|(t, _)| t)
    }

    /// Every template on this board. The task form needs the list so a
    /// template can be told to depend on its neighbours.
    pub fn all(&self) -> Vec<Template> {
        self.templates.iter().map(|(t, _)| t.clone()).collect()
    }

    fn can_manage(&self, t: &Template) -> bool {
        self.privileged || t.owner_uid == self.me
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        b.widget(&self.list)
            .widget(&self.use_btn)
            .widget(&self.new_btn)
            .widget(&self.edit_btn)
            .widget(&self.delete_btn)
            .widget(&self.close_btn);
        b.build()
    }

    /// Selected template if the user may manage it, else an error message.
    fn manageable(&mut self) -> Option<Template> {
        let t = self.selected()?.clone();
        if self.can_manage(&t) {
            Some(t)
        } else {
            self.error = Some("only the template creator, the board owner or an admin can change or delete this template".into());
            None
        }
    }

    pub fn handle(&mut self, ev: &Event) -> TemplatesOutcome {
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        if matches!(key, Some(KeyCode::Esc | KeyCode::Char('q'))) {
            return TemplatesOutcome::Cancel;
        }
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return TemplatesOutcome::Changed;
        }
        if self.close_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return TemplatesOutcome::Cancel;
        }
        if self.new_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || key == Some(KeyCode::Char('n'))
        {
            return TemplatesOutcome::New;
        }
        let use_it = self.use_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || (key == Some(KeyCode::Enter) && self.list.is_focused());
        if use_it {
            return match self.selected() {
                Some(t) => TemplatesOutcome::Use(Box::new(t.clone())),
                None => TemplatesOutcome::Changed,
            };
        }
        if self.edit_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return match self.manageable() {
                Some(t) => TemplatesOutcome::Edit(Box::new(t)),
                None => TemplatesOutcome::Changed,
            };
        }
        if self.delete_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return match self.manageable() {
                Some(t) => TemplatesOutcome::Delete(Box::new(t)),
                None => TemplatesOutcome::Changed,
            };
        }
        self.list.handle(ev, Regular);
        TemplatesOutcome::Changed
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let p = popup(area, 62, 18);
        f.render_widget(Clear, p);
        let block = frame_block(
            " Templates ",
            " Enter uses the selected one   Esc closes ",
            t,
        );
        let inner = block.inner(p);
        f.render_widget(block, p);
        let [l, err, buttons] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(button_h(t)),
        ])
        .areas(inner);
        let items: Vec<ListItem> = if self.templates.is_empty() {
            vec![
                ListItem::new("no templates on this board yet, New saves one")
                    .style(t.surface_dim()),
            ]
        } else {
            self.templates
                .iter()
                .map(|(tpl, owner)| ListItem::new(format!("{:<32} by {owner}", tpl.name)))
                .collect()
        };
        f.render_stateful_widget(list(items, t), l, &mut self.list);
        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), err);
        }
        let labels = [" Use ", " New ", " Edit ", " Delete ", " Close "];
        let rects = button_bar(buttons, &labels, t);
        let states = [
            &mut self.use_btn,
            &mut self.new_btn,
            &mut self.edit_btn,
            &mut self.delete_btn,
            &mut self.close_btn,
        ];
        for ((r, label), state) in rects.iter().zip(labels).zip(states) {
            render_button(f, *r, label, state, t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taskologic_core::ids::TemplateId;
    use taskologic_core::task::TaskDraft;

    fn tpl(id: i64, owner: Uid) -> Template {
        Template {
            id: TemplateId(id),
            board_id: BoardId(1),
            owner_uid: owner,
            name: format!("t{id}"),
            draft: TaskDraft::default(),
            options: Default::default(),
        }
    }

    #[test]
    fn managing_someone_elses_template_is_pre_refused_with_a_reason() {
        let mut p = TemplatesPanel::new(BoardId(1), 2, false);
        p.set_templates(vec![tpl(1, 1), tpl(2, 2)], &|u| format!("u{u}"));
        p.list.select(Some(0));
        assert_eq!(p.manageable(), None, "not the owner");
        assert!(p.error.take().is_some());
        p.list.select(Some(1));
        assert_eq!(p.manageable().map(|t| t.id), Some(TemplateId(2)));
        // Board owner or admin manages everything.
        let mut p = TemplatesPanel::new(BoardId(1), 2, true);
        p.set_templates(vec![tpl(1, 1)], &|u| format!("u{u}"));
        p.list.select(Some(0));
        assert!(p.manageable().is_some());
    }
}
