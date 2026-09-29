//! The Print codes panel: control codes by category, printed one at a time
//! or a whole category as a strip. What the codes mean is in
//! `taskologic_core::control`; this only lists, asks and picks. An entry
//! that needs something first (a key, text, a column, plus so much) asks in
//! a small popup; Combine appends a value to a command; sticky prints an
//! override that stays armed.

use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Clear, ListItem, Paragraph, Wrap};
use taskologic_core::control::{self, Asks, Category, ColumnRef, Control, Entry, Unit, Value};

use super::{Row, button_bar, button_h, check_w, clicked_outside, frame_block, label, popup, split_label, tall_item};
use crate::ui::adapter::{checkbox_at, dropdown, dropdown_marker, dropdown_popup_hover};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, CheckboxState, ChoiceState, Focus, FocusBuilder, HandleEvent,
    HasFocus, HasScreenCursor, ListState, Outcome, Regular, TextInputState, field, list,
    render_button,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum CodesOutcome {
    Changed,
    Cancel,
    /// Print these, one slip: a heading and the labelled codes.
    Print {
        heading: String,
        codes: Vec<(String, Vec<Control>)>,
    },
}

const UNIT_ITEMS: [(Unit, &str); 6] = [
    (Unit::Minutes, "minutes"),
    (Unit::Hours, "hours"),
    (Unit::Days, "days"),
    (Unit::Weeks, "weeks"),
    (Unit::Months, "months"),
    (Unit::Years, "years"),
];

/// Every column a code can name, for the column popup.
fn column_options() -> Vec<ColumnRef> {
    let mut v = vec![
        ColumnRef::Todo,
        ColumnRef::Paused,
        ColumnRef::Doing,
        ColumnRef::Finished,
    ];
    v.extend((1..=9).map(ColumnRef::Index));
    v.push(ColumnRef::Last);
    v
}

/// A question the panel asks before it can print.
struct Ask {
    kind: Asks,
    /// The label so far, and the commands the answer is appended to.
    label: String,
    base: Vec<Control>,
    /// Whether the finished code may print sticky.
    sticky_ok: bool,
    /// Plus or minus, for an offset.
    minus: bool,
    text: TextInputState,
    unit: ChoiceState<Unit>,
    /// The highlighted column option.
    sel: usize,
}

enum View {
    Categories,
    Entries(Category),
    /// Picking a value to append to `base`.
    Combine {
        from: Category,
        label: String,
        base: Vec<Control>,
        sticky_ok: bool,
    },
    Ask {
        from: Category,
        ask: Box<Ask>,
    },
}

pub struct CodesPanel {
    view: View,
    list: ListState,
    area: Rect,
    /// Print overrides so they stay armed until Esc.
    sticky: CheckboxState,
    print_btn: ButtonState,
    all_btn: ButtonState,
    combine_btn: ButtonState,
    back_btn: ButtonState,
    close_btn: ButtonState,
    /// Opens a category, or answers an ask: Enter for a finger.
    open_btn: ButtonState,
    pub error: Option<String>,
}

impl Default for CodesPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl CodesPanel {
    pub fn new() -> Self {
        let mut list = ListState::named("codes");
        list.focus().set(true);
        list.select(Some(0));
        Self {
            view: View::Categories,
            list,
            area: Rect::default(),
            sticky: CheckboxState::named("sticky"),
            print_btn: ButtonState::new(),
            all_btn: ButtonState::new(),
            combine_btn: ButtonState::new(),
            back_btn: ButtonState::new(),
            close_btn: ButtonState::new(),
            open_btn: ButtonState::new(),
            error: None,
        }
    }

    /// A click on the row that is already selected: what a double click
    /// amounts to when the first click did the selecting. Rows are three
    /// tall with bigger buttons on, and the list knows where each one is.
    fn clicked_selected(&self, ev: &Event) -> bool {
        let Event::Mouse(m) = ev else { return false };
        if !matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
            return false;
        }
        let pos = ratatui::layout::Position::new(m.column, m.row);
        self.list
            .row_areas
            .iter()
            .position(|r| r.contains(pos))
            .is_some_and(|i| self.list.selected() == Some(i))
    }

    /// The category whose entries are listed, if any.
    #[cfg(test)]
    pub fn category(&self) -> Option<Category> {
        match &self.view {
            View::Entries(c) => Some(*c),
            _ => None,
        }
    }

    fn entries(&self) -> Vec<Entry> {
        match &self.view {
            View::Entries(c) => control::entries(*c),
            View::Combine { .. } => control::entries(Category::Values),
            _ => Vec::new(),
        }
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        match &self.view {
            View::Ask { ask, .. } => {
                match ask.kind {
                    Asks::Column => {}
                    Asks::Offset => {
                        b.widget(&ask.text).widget(&ask.unit);
                    }
                    _ => {
                        b.widget(&ask.text);
                    }
                }
                b.widget(&self.open_btn).widget(&self.back_btn);
            }
            View::Categories => {
                b.widget(&self.list).widget(&self.open_btn).widget(&self.close_btn);
            }
            View::Entries(_) => {
                b.widget(&self.list)
                    .widget(&self.print_btn)
                    .widget(&self.all_btn)
                    .widget(&self.combine_btn)
                    .widget(&self.sticky)
                    .widget(&self.back_btn)
                    .widget(&self.close_btn);
            }
            View::Combine { .. } => {
                b.widget(&self.list).widget(&self.print_btn).widget(&self.back_btn);
            }
        }
        b.build()
    }

    /// The finished code: sticky appended when asked for and allowed.
    fn print_one(&self, label: String, mut commands: Vec<Control>, sticky_ok: bool) -> CodesOutcome {
        if sticky_ok && self.sticky.checked() {
            commands.push(Control::Sticky);
        }
        CodesOutcome::Print {
            heading: label.clone(),
            codes: vec![(label, commands)],
        }
    }

    fn open_ask(&mut self, from: Category, kind: Asks, label: String, base: Vec<Control>, sticky_ok: bool, minus: bool) {
        let mut text = TextInputState::named("ask");
        let mut unit = ChoiceState::named("unit");
        unit.set_value(Unit::Hours);
        if kind == Asks::Offset {
            text.set_text("1");
        }
        text.focus().set(true);
        self.view = View::Ask {
            from,
            ask: Box::new(Ask {
                kind,
                label,
                base,
                sticky_ok,
                minus,
                text,
                unit,
                sel: 0,
            }),
        };
        self.error = None;
    }

    /// Enter on an entry: print it, or ask first.
    fn choose_entry(&mut self, from: Category, e: &Entry) -> CodesOutcome {
        match e.asks {
            Asks::Nothing => self.print_one(e.label.to_string(), e.commands.clone(), e.sticky),
            kind => {
                let minus = e.label.starts_with("Minus");
                self.open_ask(from, kind, e.label.to_string(), e.commands.clone(), e.sticky, minus);
                CodesOutcome::Changed
            }
        }
    }

    /// The answer to an ask, made into the finished code.
    fn finish_ask(&mut self) -> CodesOutcome {
        let View::Ask { from, ask } = &mut self.view else {
            return CodesOutcome::Changed;
        };
        let from = *from;
        let mut commands = ask.base.clone();
        let text = ask.text.text().trim().to_string();
        let (label, sticky_ok) = (ask.label.clone(), ask.sticky_ok);
        let added: Control = match ask.kind {
            Asks::Key => {
                let mut chars = text.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Control::Key(c),
                    _ => {
                        self.error = Some("one key: a single character".into());
                        return CodesOutcome::Changed;
                    }
                }
            }
            Asks::Text => {
                if text.is_empty() {
                    self.error = Some("type the text first".into());
                    return CodesOutcome::Changed;
                }
                Control::Value(Value::Text(text.clone()))
            }
            Asks::Offset => {
                let n: u32 = match text.parse() {
                    Ok(n) if n > 0 => n,
                    _ => {
                        self.error = Some("a whole number, at least 1".into());
                        return CodesOutcome::Changed;
                    }
                };
                let unit = ask.unit.value();
                Control::Value(if ask.minus { Value::Minus(n, unit) } else { Value::Plus(n, unit) })
            }
            Asks::Column => {
                let col = column_options()[ask.sel];
                // A "move to" asked for its column becomes the code with it.
                if let Some(Control::MoveTo(None)) = commands.last() {
                    commands.pop();
                }
                Control::MoveTo(Some(col))
            }
            Asks::Nothing => unreachable!("nothing to ask"),
        };
        let label = if commands.is_empty() || matches!(added, Control::MoveTo(_)) {
            added.label()
        } else {
            format!("{label}, {}", added.label())
        };
        commands.push(added);
        self.view = View::Entries(from);
        self.list.focus().set(true);
        self.print_one(label, commands, sticky_ok)
    }

    pub fn handle(&mut self, ev: &Event) -> CodesOutcome {
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        if clicked_outside(ev, self.area) {
            return CodesOutcome::Cancel;
        }
        // An ask has its own keys: Enter answers, Esc goes back.
        if let View::Ask { from, ask } = &mut self.view {
            let from = *from;
            match key {
                Some(KeyCode::Esc) if !ask.unit.is_popup_active() => {
                    self.view = View::Entries(from);
                    self.list.focus().set(true);
                    return CodesOutcome::Changed;
                }
                Some(KeyCode::Enter) if !ask.unit.is_popup_active() => return self.finish_ask(),
                _ => {}
            }
            if ask.kind == Asks::Column {
                let n = column_options().len();
                match key {
                    Some(KeyCode::Up | KeyCode::Char('k')) => ask.sel = ask.sel.saturating_sub(1),
                    Some(KeyCode::Down | KeyCode::Char('j')) => ask.sel = (ask.sel + 1).min(n - 1),
                    _ => {}
                }
            } else {
                let mut focus = self.focus();
                focus.handle(ev, Regular);
                if let View::Ask { ask, .. } = &mut self.view {
                    ask.text.handle(ev, Regular);
                    ask.unit.handle(ev, Regular);
                }
            }
            if self.open_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
                return self.finish_ask();
            }
            if self.back_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
                self.view = View::Entries(from);
                self.list.focus().set(true);
            }
            return CodesOutcome::Changed;
        }
        if matches!(key, Some(KeyCode::Esc | KeyCode::Char('q'))) {
            return self.back();
        }
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return CodesOutcome::Changed;
        }
        if self.close_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return CodesOutcome::Cancel;
        }
        if self.back_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return self.back();
        }
        self.sticky.handle(ev, Regular);
        if key == Some(KeyCode::Char('s')) && matches!(self.view, View::Entries(_)) {
            self.sticky.flip_checked();
            return CodesOutcome::Changed;
        }
        let enter = self.print_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || self.open_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || (key == Some(KeyCode::Enter) && self.list.is_focused());
        if enter {
            return self.open_selected();
        }
        if self.clicked_selected(ev) {
            return self.open_selected();
        }
        let combine = self.combine_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || key == Some(KeyCode::Char('c'));
        if combine && let View::Entries(cat) = self.view {
            let entries = control::entries(cat);
            match self.list.selected().and_then(|i| entries.get(i)) {
                Some(e) if e.asks == Asks::Nothing && !e.commands.is_empty() => {
                    self.view = View::Combine {
                        from: cat,
                        label: e.label.to_string(),
                        base: e.commands.clone(),
                        sticky_ok: e.sticky,
                    };
                    self.list.select(Some(0));
                    self.error = None;
                }
                Some(_) => self.error = Some("combine a finished code; this one asks first".into()),
                None => {}
            }
            return CodesOutcome::Changed;
        }
        let all = self.all_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || key == Some(KeyCode::Char('a'));
        if all && let View::Entries(cat) = self.view {
            let sticky = self.sticky.checked();
            let codes: Vec<(String, Vec<Control>)> = control::entries(cat)
                .iter()
                .filter(|e| e.asks == Asks::Nothing)
                .map(|e| {
                    let mut c = e.commands.clone();
                    if sticky && e.sticky {
                        c.push(Control::Sticky);
                    }
                    (e.label.to_string(), c)
                })
                .collect();
            if codes.is_empty() {
                self.error = Some("nothing here prints without asking".into());
                return CodesOutcome::Changed;
            }
            return CodesOutcome::Print {
                heading: cat.label().to_string(),
                codes,
            };
        }
        self.list.handle(ev, Regular);
        CodesOutcome::Changed
    }

    fn open_selected(&mut self) -> CodesOutcome {
        match &self.view {
            View::Categories => {
                if let Some(cat) = self.list.selected().and_then(|i| Category::ALL.get(i)) {
                    self.view = View::Entries(*cat);
                    self.list.select(Some(0));
                    self.error = None;
                }
                CodesOutcome::Changed
            }
            View::Entries(cat) => {
                let cat = *cat;
                let entries = control::entries(cat);
                match self.list.selected().and_then(|i| entries.get(i)).cloned() {
                    Some(e) => self.choose_entry(cat, &e),
                    None => CodesOutcome::Changed,
                }
            }
            View::Combine { from, label, base, sticky_ok } => {
                let (from, label, base, sticky_ok) = (*from, label.clone(), base.clone(), *sticky_ok);
                let values = control::entries(Category::Values);
                let Some(v) = self.list.selected().and_then(|i| values.get(i)).cloned() else {
                    return CodesOutcome::Changed;
                };
                if v.asks == Asks::Nothing {
                    let mut commands = base;
                    commands.extend(v.commands.iter().cloned());
                    self.view = View::Entries(from);
                    self.print_one(format!("{label}, {}", v.label.to_lowercase()), commands, sticky_ok)
                } else {
                    let minus = v.label.starts_with("Minus");
                    self.open_ask(from, v.asks, label, base, sticky_ok, minus);
                    CodesOutcome::Changed
                }
            }
            View::Ask { .. } => CodesOutcome::Changed,
        }
    }

    fn back(&mut self) -> CodesOutcome {
        match &self.view {
            View::Categories => CodesOutcome::Cancel,
            View::Entries(cat) => {
                let at = Category::ALL.iter().position(|c| c == cat).unwrap_or(0);
                self.view = View::Categories;
                self.list.select(Some(at));
                self.error = None;
                CodesOutcome::Changed
            }
            View::Combine { from, .. } | View::Ask { from, .. } => {
                self.view = View::Entries(*from);
                self.list.focus().set(true);
                self.error = None;
                CodesOutcome::Changed
            }
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let p = popup(area, 78, 32 + button_h(t));
        self.area = p;
        f.render_widget(Clear, p);
        let title = match &self.view {
            View::Categories => " Print codes ".to_string(),
            View::Entries(c) => format!(" Print codes - {} ", c.label()),
            View::Combine { label, .. } => format!(" Combine \"{label}\" with a value "),
            View::Ask { ask, .. } => format!(" {} ", ask.label),
        };
        let hint = match &self.view {
            View::Categories => " Enter or Open opens a category, a click on the selected one too   Esc closes ",
            View::Entries(_) => " Enter prints   a prints all   c combines   s sticky   Esc goes back ",
            View::Combine { .. } => " Enter picks the value   Esc goes back ",
            View::Ask { .. } => " Enter prints   Esc goes back ",
        };
        let block = frame_block(&title, hint, t);
        let inner = block.inner(p);
        f.render_widget(block, p);
        let [note, l, err, buttons] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(button_h(t)),
        ])
        .areas(inner);
        let blurb = match &self.view {
            View::Categories => "Barcodes that drive this screen: scan one and the client does what it says. \
                                 Stick a strip on the wall for a terminal without a keyboard."
                .to_string(),
            View::Entries(c) => c.blurb().to_string(),
            View::Combine { .. } => "one code that does both: the command, then the value it takes".to_string(),
            View::Ask { ask, .. } => match ask.kind {
                Asks::Key => "which key? one character, case as pressed".to_string(),
                Asks::Text => "the text the code will type or search for".to_string(),
                Asks::Offset => "how much, and in what".to_string(),
                Asks::Column => "which column, by role or counted from the left".to_string(),
                Asks::Nothing => String::new(),
            },
        };
        f.render_widget(
            Paragraph::new(blurb).style(t.surface_dim()).wrap(Wrap { trim: true }),
            note,
        );
        let mut unit_popup = None;
        match &mut self.view {
            View::Ask { ask, .. } => {
                let lw = 8;
                let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(l);
                match ask.kind {
                    Asks::Column => {
                        let items: Vec<ListItem> = column_options()
                            .iter()
                            .enumerate()
                            .map(|(i, c)| {
                                let marker = if i == ask.sel { "> " } else { "  " };
                                ListItem::new(format!("{marker}{}   ({})", c.label(), c.code()))
                            })
                            .collect();
                        let mut ls = ListState::named("columns");
                        ls.select(Some(ask.sel));
                        f.render_stateful_widget(list(items, t), l, &mut ls);
                    }
                    Asks::Offset => {
                        let (lab, w) = split_label(rows[0], lw);
                        label(f, lab, if ask.minus { "Minus" } else { "Plus" }, t);
                        let mut r = Row::new(w);
                        f.render_stateful_widget(field(t), r.take(6), &mut ask.text);
                        let ua = r.take(12);
                        let (uw, up) = dropdown(UNIT_ITEMS, ua, t);
                        f.render_stateful_widget(uw, ua, &mut ask.unit);
                        dropdown_marker(f, &ask.unit, t);
                        unit_popup = Some((up, ua));
                    }
                    _ => {
                        let (lab, w) = split_label(rows[0], lw);
                        label(f, lab, if ask.kind == Asks::Key { "Key" } else { "Text" }, t);
                        let width = if ask.kind == Asks::Key { 4 } else { w.width.saturating_sub(1) };
                        let mut r = Row::new(w);
                        f.render_stateful_widget(field(t), r.take(width), &mut ask.text);
                    }
                }
                if let Some(pos) = ask.text.screen_cursor() {
                    f.set_cursor_position(pos);
                }
            }
            _ => {
                let items: Vec<ListItem> = match &self.view {
                    View::Categories => Category::ALL
                        .iter()
                        .map(|c| tall_item(format!("{:<28} {}", c.label(), c.blurb()), t))
                        .collect(),
                    _ => self
                        .entries()
                        .iter()
                        .map(|e| {
                            let payload = if e.asks == Asks::Nothing {
                                control::encode(&e.commands)
                            } else {
                                "asks first".to_string()
                            };
                            tall_item(format!("{:<34} {payload}", e.label), t)
                        })
                        .collect(),
                };
                f.render_stateful_widget(list(items, t), l, &mut self.list);
            }
        }
        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), err);
        }
        match &self.view {
            View::Categories => {
                let labels = [" Open ", " Close "];
                let rects = button_bar(buttons, &labels, t);
                let states = [&mut self.open_btn, &mut self.close_btn];
                for ((r, label), state) in rects.iter().zip(labels).zip(states) {
                    render_button(f, *r, label, state, t);
                }
            }
            View::Entries(_) => {
                let labels = [" Print ", " Print all ", " Combine ", " Back ", " Close "];
                let rects = button_bar(buttons, &labels, t);
                let states = [
                    &mut self.print_btn,
                    &mut self.all_btn,
                    &mut self.combine_btn,
                    &mut self.back_btn,
                    &mut self.close_btn,
                ];
                for ((r, label), state) in rects.iter().zip(labels).zip(states) {
                    render_button(f, *r, label, state, t);
                }
                // The sticky box sits after the buttons, on the same row.
                let used: u16 = rects.iter().map(|r| r.width + 1).sum();
                let cb = Rect::new(
                    buttons.x + used,
                    buttons.y + buttons.height / 2,
                    check_w("sticky").min(buttons.width.saturating_sub(used)),
                    1,
                );
                f.render_stateful_widget(checkbox_at("sticky".into(), cb, t), cb, &mut self.sticky);
            }
            View::Combine { .. } => {
                let labels = [" Pick ", " Back "];
                let rects = button_bar(buttons, &labels, t);
                let states = [&mut self.print_btn, &mut self.back_btn];
                for ((r, label), state) in rects.iter().zip(labels).zip(states) {
                    render_button(f, *r, label, state, t);
                }
            }
            View::Ask { .. } => {
                let labels = [" OK ", " Back "];
                let rects = button_bar(buttons, &labels, t);
                let states = [&mut self.open_btn, &mut self.back_btn];
                for ((r, label), state) in rects.iter().zip(labels).zip(states) {
                    render_button(f, *r, label, state, t);
                }
            }
        }
        if let (Some((up, ua)), View::Ask { ask, .. }) = (unit_popup, &mut self.view) {
            f.render_stateful_widget(up, ua, &mut ask.unit);
            dropdown_popup_hover(f, &ask.unit, t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn typed(p: &mut CodesPanel, s: &str) {
        for c in s.chars() {
            p.handle(&key(KeyCode::Char(c)));
        }
    }

    fn payload(out: CodesOutcome) -> String {
        match out {
            CodesOutcome::Print { codes, .. } => control::encode(&codes[0].1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn categories_open_entries_print_and_esc_walks_back_out() {
        let mut p = CodesPanel::new();
        p.list.select(Some(1));
        assert_eq!(p.handle(&key(KeyCode::Enter)), CodesOutcome::Changed);
        assert_eq!(p.category(), Some(Category::Navigation));
        match p.handle(&key(KeyCode::Enter)) {
            CodesOutcome::Print { heading, codes } => {
                assert_eq!(heading, "Show all boards");
                assert_eq!(codes[0].1, vec![Control::Dashboard]);
            }
            other => panic!("{other:?}"),
        }
        match p.handle(&key(KeyCode::Char('a'))) {
            CodesOutcome::Print { heading, codes } => {
                assert_eq!(heading, "Navigation and keys");
                assert!(codes.iter().any(|(l, _)| l == "Scanner check"));
                assert!(!codes.iter().any(|(l, _)| l == "Press a key"), "asks for a key");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(p.handle(&key(KeyCode::Esc)), CodesOutcome::Changed);
        assert_eq!(p.category(), None);
        assert_eq!(p.list.selected(), Some(1));
        assert_eq!(p.handle(&key(KeyCode::Esc)), CodesOutcome::Cancel);
    }

    #[test]
    fn entries_that_ask_get_their_answer_in_a_popup() {
        let mut p = CodesPanel::new();
        // Navigation: "Press a key" asks for one character.
        p.list.select(Some(1));
        p.handle(&key(KeyCode::Enter));
        let entries = control::entries(Category::Navigation);
        let at = |label: &str| entries.iter().position(|e| e.label == label).unwrap();
        p.list.select(Some(at("Press a key")));
        assert_eq!(p.handle(&key(KeyCode::Enter)), CodesOutcome::Changed, "asks");
        typed(&mut p, "T");
        assert_eq!(payload(p.handle(&key(KeyCode::Enter))), "--1KT--");
        assert_eq!(p.category(), Some(Category::Navigation), "back on the list");
        // Search asks for text, which is appended.
        p.list.select(Some(at("Search")));
        p.handle(&key(KeyCode::Enter));
        typed(&mut p, "soap");
        assert_eq!(payload(p.handle(&key(KeyCode::Enter))), "--1Q/V.soap--");
        // Values: plus so much asks for a number and a unit.
        p.handle(&key(KeyCode::Esc));
        p.list.select(Some(3));
        p.handle(&key(KeyCode::Enter));
        let values = control::entries(Category::Values);
        p.list.select(Some(values.iter().position(|e| e.label == "Plus so much").unwrap()));
        p.handle(&key(KeyCode::Enter));
        typed(&mut p, "2"); // the field starts with 1, so this reads 12; select and retype
        if let View::Ask { ask, .. } = &mut p.view {
            ask.text.set_text("2".to_string());
        }
        assert_eq!(payload(p.handle(&key(KeyCode::Enter))), "--1P2H--");
        // Next scan: move to a column asks which.
        p.handle(&key(KeyCode::Esc));
        p.list.select(Some(0));
        p.handle(&key(KeyCode::Enter));
        let next = control::entries(Category::NextScan);
        p.list.select(Some(next.iter().position(|e| e.label == "Move to a column").unwrap()));
        p.handle(&key(KeyCode::Enter));
        for _ in 0..6 {
            p.handle(&key(KeyCode::Down));
        }
        assert_eq!(payload(p.handle(&key(KeyCode::Enter))), "--1MC3--", "todo, paused, doing, done, 1, 2, 3");
    }

    #[test]
    fn a_click_on_the_selected_row_opens_it_and_the_buttons_do_enter() {
        use crossterm::event::{KeyModifiers, MouseEvent};
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut p = CodesPanel::new();
        let theme = crate::ui::theme::Theme::default().with_touch(true);
        let mut term = Terminal::new(TestBackend::new(100, 44)).unwrap();
        term.draw(|f| p.render(f, f.area(), &theme)).unwrap();
        // The first category is selected; a click on its row opens it.
        let row = p.list.row_areas[0];
        let click = |x, y| {
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            })
        };
        assert_eq!(p.handle(&click(row.x + 2, row.y + 1)), CodesOutcome::Changed);
        assert_eq!(p.category(), Some(Category::NextScan), "opened by the click");
        // Inside, a click on a row that is not selected only selects it.
        term.draw(|f| p.render(f, f.area(), &theme)).unwrap();
        let second = p.list.row_areas[1];
        let out = p.handle(&click(second.x + 2, second.y + 1));
        assert_eq!(out, CodesOutcome::Changed);
        assert_eq!(p.list.selected(), Some(1));
        // And a click on it now prints it.
        term.draw(|f| p.render(f, f.area(), &theme)).unwrap();
        let second = p.list.row_areas[1];
        assert!(matches!(p.handle(&click(second.x + 2, second.y + 1)), CodesOutcome::Print { .. }));
    }

    #[test]
    fn combine_appends_a_value_and_sticky_prints_the_variant() {
        let mut p = CodesPanel::new();
        // Data entry: Insert text asks; "Insert now" combines with plus two hours.
        p.list.select(Some(2));
        p.handle(&key(KeyCode::Enter));
        p.list.select(Some(0)); // Insert now
        p.handle(&key(KeyCode::Char('c')));
        assert!(matches!(p.view, View::Combine { .. }));
        let values = control::entries(Category::Values);
        p.list.select(Some(values.iter().position(|e| e.label == "Plus so much").unwrap()));
        p.handle(&key(KeyCode::Enter));
        if let View::Ask { ask, .. } = &mut p.view {
            ask.text.set_text("2".to_string());
        }
        assert_eq!(payload(p.handle(&key(KeyCode::Enter))), "--1I/N/P2H--");
        // Next scan with sticky ticked prints the /STK variant, once per code.
        p.handle(&key(KeyCode::Esc));
        p.list.select(Some(0));
        p.handle(&key(KeyCode::Enter));
        p.handle(&key(KeyCode::Char('s')));
        assert!(p.sticky.checked());
        p.list.select(Some(0)); // Move left
        assert_eq!(payload(p.handle(&key(KeyCode::Enter))), "--1ML/STK--");
        match p.handle(&key(KeyCode::Char('a'))) {
            CodesOutcome::Print { codes, .. } => {
                assert!(codes.iter().filter(|(l, _)| l == "Move left").all(|(_, c)| c.last() == Some(&Control::Sticky)));
                assert!(codes.iter().find(|(l, _)| l == "The selected task").is_some_and(|(_, c)| c.last() != Some(&Control::Sticky)), "SEL is never sticky");
            }
            other => panic!("{other:?}"),
        }
    }
}
