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

use super::{ListArrows, RowClicks, button_bar, button_h, clicked_outside, frame_block, list_arrow_tap, list_double_click, list_with_arrows, popup, tall_item};
use crate::ui::adapter::{ButtonOutcome, ButtonState, Focus, FocusBuilder, HandleEvent, HasFocus, ListState, Outcome, Regular, render_button};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum TemplatesOutcome {
    Changed,
    Cancel,
    New,
    Use(Box<Template>),
    Edit(Box<Template>),
    Delete(Box<Template>),
    /// Print the template's own control codes.
    PrintCode(Box<Template>),
}

pub struct TemplatesPanel {
    pub board_id: BoardId,
    me: Uid,
    /// Where the window was last drawn; a click anywhere else closes it.
    area: Rect,
    /// The scroll arrows drawn last, for taps.
    arrows: ListArrows,
    /// Click timing per row, for double clicks.
    clicks: RowClicks,
    /// Board owner or admin: may manage every template here.
    privileged: bool,
    /// Template plus its owner's name for the list line.
    templates: Vec<(Template, String)>,
    list: ListState,
    use_btn: ButtonState,
    new_btn: ButtonState,
    edit_btn: ButtonState,
    delete_btn: ButtonState,
    code_btn: ButtonState,
    close_btn: ButtonState,
    pub error: Option<String>,
}

impl TemplatesPanel {
    pub fn new(board_id: BoardId, me: Uid, privileged: bool) -> Self {
        let list = ListState::named("templates");
        list.focus().set(true);
        Self {
            area: Rect::default(),
            arrows: ListArrows::default(),
            clicks: RowClicks::default(),
            board_id,
            me,
            privileged,
            templates: Vec::new(),
            list,
            use_btn: ButtonState::new(),
            new_btn: ButtonState::new(),
            edit_btn: ButtonState::new(),
            delete_btn: ButtonState::new(),
            code_btn: ButtonState::new(),
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
            .widget(&self.code_btn)
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
        if list_arrow_tap(ev, self.arrows, &mut self.list) {
            return TemplatesOutcome::Changed;
        }
        if let Some(row) = list_double_click(ev, &mut self.clicks, &self.list) {
            self.list.select(Some(row));
            self.list.focus().set(true);
            return self.handle(&Event::Key(crossterm::event::KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE)));
        }
        if matches!(key, Some(KeyCode::Esc | KeyCode::Char('q'))) || clicked_outside(ev, self.area) {
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
        if self.code_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || key == Some(KeyCode::Char('c'))
        {
            return match self.selected() {
                Some(t) => TemplatesOutcome::PrintCode(Box::new(t.clone())),
                None => TemplatesOutcome::Changed,
            };
        }
        self.list.handle(ev, Regular);
        TemplatesOutcome::Changed
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        // The same window as a task's, so the panels line up.
        let p = popup(area, 78, 32 + button_h(t));
        self.area = p;
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
                .map(|(tpl, owner)| tall_item(format!("{:<32} by {owner}", tpl.name), t))
                .collect()
        };
        self.arrows = list_with_arrows(f, l, items, &mut self.list, t);
        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), err);
        }
        let labels = [" Use ", " New ", " Edit ", " Delete ", " Print ", " Close "];
        let rects = button_bar(buttons, &labels, t);
        let states = [
            &mut self.use_btn,
            &mut self.new_btn,
            &mut self.edit_btn,
            &mut self.delete_btn,
            &mut self.code_btn,
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
            short_id: taskologic_core::ids::ShortId::from_index(0),
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

    /// Where a text lands on a rendered screen, as (row, column). The test
    /// backend quotes every line, and the frame is drawn in wide glyphs, so
    /// columns are counted in characters past the quote.
    fn find(out: &str, text: &str) -> (usize, usize) {
        out.lines()
            .enumerate()
            .find_map(|(y, l)| {
                let l = l.trim_matches('"');
                l.find(text).map(|x| (y, l[..x].chars().count()))
            })
            .unwrap_or_else(|| panic!("{text:?} not on screen:\n{out}"))
    }

    #[test]
    fn a_click_on_the_board_behind_the_window_closes_it() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let press = |column, row| {
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            })
        };
        let mut p = TemplatesPanel::new(BoardId(1), 1, true);
        p.set_templates(vec![tpl(1, 1)], &|u| format!("u{u}"));
        // Before the first draw nothing counts as outside.
        assert!(!matches!(p.handle(&press(0, 0)), TemplatesOutcome::Cancel));
        let theme = crate::ui::theme::Theme::default();
        let mut term = Terminal::new(TestBackend::new(100, 44)).unwrap();
        term.draw(|f| p.render(f, f.area(), &theme)).unwrap();
        // The window is 78 wide from column 11, 33 tall from row 5.
        assert!(matches!(p.handle(&press(0, 0)), TemplatesOutcome::Cancel));
        assert!(matches!(p.handle(&press(95, 20)), TemplatesOutcome::Cancel));
        assert!(!matches!(p.handle(&press(20, 8)), TemplatesOutcome::Cancel), "inside is a list click");
    }

    #[test]
    fn with_bigger_buttons_every_row_is_three_tall_and_the_panel_is_the_task_window() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut p = TemplatesPanel::new(BoardId(1), 1, true);
        p.set_templates(vec![tpl(1, 1), tpl(2, 1)], &|u| format!("u{u}"));
        let render = |p: &mut TemplatesPanel, touch: bool| {
            let theme = crate::ui::theme::Theme::default().with_touch(touch);
            let mut term = Terminal::new(TestBackend::new(100, 44)).unwrap();
            term.draw(|f| p.render(f, f.area(), &theme)).unwrap();
            term.backend().to_string()
        };
        let plain = render(&mut p, false);
        assert_eq!(find(&plain, "t2").0 - find(&plain, "t1").0, 1, "one row each:\n{plain}");
        let touch = render(&mut p, true);
        assert_eq!(find(&touch, "t2").0 - find(&touch, "t1").0, 3, "three rows each:\n{touch}");
        // 78 wide like the task form: centred in 100 columns, so 11 in.
        assert_eq!(find(&touch, "┌ Templates ").1, 11, "{touch}");
        let frame_rows = touch.lines().filter(|l| l.contains('│')).count();
        assert_eq!(frame_rows, 32 + 3 - 2, "32 rows plus a three high button row, minus the corners");
    }
}
