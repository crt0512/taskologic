//! The Print codes panel: control codes by category, printed one at a time
//! or a whole category as a strip. What the codes mean is in
//! `taskologic_core::control`; this only lists, asks and picks. An entry
//! that needs something first (a key, text, a column, plus so much) asks in
//! a small popup; Combine appends a value to a command; sticky prints an
//! override that stays armed.

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Clear, ListItem, Paragraph, Wrap};
use taskologic_core::control::{self, Asks, Category, ColumnRef, Control, Entry, Unit, Value};

use super::{ListArrows, Row, RowClicks, button_bar, button_h, check_w, clicked_outside, frame_block, label, list_arrow_tap, list_double_click, list_with_arrows, popup, split_label, tall_item};
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
    /// Set on the first of two offsets: the sign the second one takes.
    then_minus: Option<bool>,
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
    /// The scroll arrows drawn last, for taps.
    arrows: ListArrows,
    /// Click timing per row, for double clicks.
    clicks: RowClicks,
    /// Print overrides so they stay armed until Esc.
    sticky: CheckboxState,
    /// Print armed commands with `/SEL` on the end: one scan does it to the
    /// highlighted task.
    on_selected: CheckboxState,
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
            arrows: ListArrows::default(),
            clicks: RowClicks::default(),
            sticky: CheckboxState::named("sticky"),
            on_selected: CheckboxState::named("on_selected"),
            print_btn: ButtonState::new(),
            all_btn: ButtonState::new(),
            combine_btn: ButtonState::new(),
            back_btn: ButtonState::new(),
            close_btn: ButtonState::new(),
            open_btn: ButtonState::new(),
            error: None,
        }
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
                    .widget(&self.on_selected)
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
        if sticky_ok && self.on_selected.checked() {
            commands.push(Control::Selected);
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
        } else if kind == Asks::Count {
            text.set_text("2");
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
                then_minus: None,
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
            Asks::OffsetPair => {
                // "start minus, due plus": the first sign now, the second after.
                let (start, due) = e.label.split_once(", start ").map_or(("", ""), |(_, r)| r.split_once(", due ").unwrap_or(("", "")));
                self.open_ask(from, Asks::Offset, e.label.to_string(), e.commands.clone(), e.sticky, start == "minus");
                if let View::Ask { ask, .. } = &mut self.view {
                    ask.then_minus = Some(due == "minus");
                }
                CodesOutcome::Changed
            }
            kind => {
                let minus = e.label.contains("inus");
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
            Asks::Count => {
                let n: u8 = match text.parse() {
                    Ok(n) if (1..=99).contains(&n) => n,
                    _ => {
                        self.error = Some("a whole number, 1 to 99".into());
                        return CodesOutcome::Changed;
                    }
                };
                // "Tab several times" becomes the code with its count.
                match commands.pop() {
                    Some(Control::Named(k)) => Control::NamedTimes(k, n),
                    other => {
                        commands.extend(other);
                        return CodesOutcome::Changed;
                    }
                }
            }
            // A pair is asked one offset at a time, as plain offsets.
            Asks::Nothing | Asks::OffsetPair => unreachable!("nothing to ask"),
        };
        if let Some(then_minus) = ask.then_minus {
            // The first of two offsets: ask for the due date's next.
            commands.push(added);
            commands.push(Control::Value(Value::Split));
            self.open_ask(from, Asks::Offset, label, commands, sticky_ok, then_minus);
            return CodesOutcome::Changed;
        }
        let pair = commands.contains(&Control::Value(Value::Split));
        let label = if pair {
            let mut all = commands.clone();
            all.push(added.clone());
            all.iter().map(Control::label).collect::<Vec<_>>().join(", ")
        } else if commands.is_empty() || matches!(added, Control::MoveTo(_) | Control::NamedTimes(..)) {
            added.label()
        } else if ask.kind == Asks::Offset {
            // "Set the due date, plus so much" reads "Set the due date, plus 30 minutes".
            format!("{}, {}", label.split(", ").next().unwrap_or(&label), added.label())
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
        if list_arrow_tap(ev, self.arrows, &mut self.list) {
            return CodesOutcome::Changed;
        }
        if let Some(row) = list_double_click(ev, &mut self.clicks, &self.list) {
            self.list.select(Some(row));
            return self.open_selected();
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
        self.on_selected.handle(ev, Regular);
        if key == Some(KeyCode::Char('s')) && matches!(self.view, View::Entries(_)) {
            self.sticky.flip_checked();
            return CodesOutcome::Changed;
        }
        if key == Some(KeyCode::Char('t')) && matches!(self.view, View::Entries(_)) {
            self.on_selected.flip_checked();
            return CodesOutcome::Changed;
        }
        let enter = self.print_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || self.open_btn.handle(ev, Regular) == ButtonOutcome::Pressed
            || (key == Some(KeyCode::Enter) && self.list.is_focused());
        if enter {
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
                    self.list.set_offset(0);
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
            let (sticky, on_selected) = (self.sticky.checked(), self.on_selected.checked());
            let codes: Vec<(String, Vec<Control>)> = control::entries(cat)
                .iter()
                .filter(|e| e.asks == Asks::Nothing)
                .map(|e| {
                    let mut c = e.commands.clone();
                    if sticky && e.sticky {
                        c.push(Control::Sticky);
                    }
                    if on_selected && e.sticky {
                        c.push(Control::Selected);
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
                    self.list.set_offset(0);
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
                // The entries were scrolled; the short list above starts
                // at the top again.
                let at = Category::ALL.iter().position(|c| c == cat).unwrap_or(0);
                self.view = View::Categories;
                self.list.select(Some(at));
                self.list.set_offset(0);
                self.error = None;
                CodesOutcome::Changed
            }
            View::Combine { from, .. } | View::Ask { from, .. } => {
                self.view = View::Entries(*from);
                self.list.focus().set(true);
                self.list.set_offset(0);
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
            View::Categories => " Enter, Open or a double click opens a category   Esc closes ",
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
                Asks::Offset | Asks::OffsetPair if ask.then_minus.is_some() => "the start date: how much, and in what; the due date is next".to_string(),
                Asks::Offset | Asks::OffsetPair if ask.base.contains(&Control::Value(Value::Split)) => "the due date: how much, and in what".to_string(),
                Asks::Offset | Asks::OffsetPair => "how much, and in what".to_string(),
                Asks::Count => "how many times, 1 to 99".to_string(),
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
                        let name = match ask.kind {
                            Asks::Key => "Key",
                            Asks::Count => "Times",
                            _ => "Text",
                        };
                        label(f, lab, name, t);
                        let width = if matches!(ask.kind, Asks::Key | Asks::Count) { 4 } else { w.width.saturating_sub(1) };
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
                self.arrows = list_with_arrows(f, l, items, &mut self.list, t);
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
                let used2 = used + check_w("sticky") + 1;
                let cb2 = Rect::new(
                    buttons.x + used2,
                    buttons.y + buttons.height / 2,
                    check_w("on selected").min(buttons.width.saturating_sub(used2)),
                    1,
                );
                if cb2.width > 0 {
                    f.render_stateful_widget(checkbox_at("on selected".into(), cb2, t), cb2, &mut self.on_selected);
                }
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
    use crossterm::event::{KeyEvent, KeyModifiers, MouseButton, MouseEventKind};

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
    fn the_dates_category_prints_start_and_due_codes() {
        let mut p = CodesPanel::new();
        let at = Category::ALL.iter().position(|c| *c == Category::Dates).unwrap();
        p.list.select(Some(at));
        p.handle(&key(KeyCode::Enter));
        let entries = control::entries(Category::Dates);
        let pick = |p: &mut CodesPanel, label: &str| {
            p.list.select(Some(entries.iter().position(|e| e.label == label).unwrap()));
            p.handle(&key(KeyCode::Enter))
        };
        assert_eq!(payload(pick(&mut p, "Set the start date to now")), "--1SS/N--");
        assert_eq!(payload(pick(&mut p, "Clear the due date")), "--1SU/X--");
        pick(&mut p, "Set the due date, plus so much");
        if let View::Ask { ask, .. } = &mut p.view {
            ask.text.set_text("30".to_string());
            ask.unit.set_value(Unit::Minutes);
        }
        assert_eq!(payload(p.handle(&key(KeyCode::Enter))), "--1SU/P30--");
        pick(&mut p, "Set the start date, minus so much");
        if let View::Ask { ask, .. } = &mut p.view {
            ask.text.set_text("2".to_string());
        }
        assert_eq!(payload(p.handle(&key(KeyCode::Enter))), "--1SS/M2H--");
    }

    #[test]
    fn on_selected_prints_the_code_with_sel_on_the_end() {
        let mut p = CodesPanel::new();
        let at = Category::ALL.iter().position(|c| *c == Category::Dates).unwrap();
        p.list.select(Some(at));
        p.handle(&key(KeyCode::Enter));
        p.handle(&key(KeyCode::Char('t')));
        assert!(p.on_selected.checked());
        let entries = control::entries(Category::Dates);
        p.list.select(Some(entries.iter().position(|e| e.label == "Set start and due to now").unwrap()));
        assert_eq!(payload(p.handle(&key(KeyCode::Enter))), "--1SB/N/SEL--");
        // An offset asked for, then SEL; with sticky too it reads /STK/SEL.
        p.list.select(Some(entries.iter().position(|e| e.label == "Set start and due, minus so much").unwrap()));
        p.handle(&key(KeyCode::Enter));
        if let View::Ask { ask, .. } = &mut p.view {
            ask.text.set_text("30".to_string());
            ask.unit.set_value(Unit::Minutes);
        }
        assert_eq!(payload(p.handle(&key(KeyCode::Enter))), "--1SB/M30/SEL--");
        p.handle(&key(KeyCode::Char('s')));
        p.list.select(Some(entries.iter().position(|e| e.label == "Set start and due to now").unwrap()));
        assert_eq!(payload(p.handle(&key(KeyCode::Enter))), "--1SB/N/STK/SEL--");
    }

    #[test]
    fn start_and_due_take_two_offsets_in_one_code() {
        use control::Value;
        assert_eq!(
            control::parse("SB/P30/U/P2D").unwrap().len(),
            4,
            "set, offset, split, offset"
        );
        assert!(control::parse("SB/P30/U/P2D").unwrap().contains(&Control::Value(Value::Split)));
        let mut p = CodesPanel::new();
        let at = Category::ALL.iter().position(|c| *c == Category::Dates).unwrap();
        p.list.select(Some(at));
        p.handle(&key(KeyCode::Enter));
        let entries = control::entries(Category::Dates);
        p.list.select(Some(entries.iter().position(|e| e.label == "Set start and due, start plus, due minus").unwrap()));
        assert_eq!(p.handle(&key(KeyCode::Enter)), CodesOutcome::Changed, "asks for the start");
        if let View::Ask { ask, .. } = &mut p.view {
            ask.text.set_text("30".to_string());
            ask.unit.set_value(Unit::Minutes);
        }
        assert_eq!(p.handle(&key(KeyCode::Enter)), CodesOutcome::Changed, "then for the due date");
        if let View::Ask { ask, .. } = &mut p.view {
            ask.text.set_text("2".to_string());
            ask.unit.set_value(Unit::Days);
        }
        assert_eq!(payload(p.handle(&key(KeyCode::Enter))), "--1SB/P30/U/M2D--");
    }

    #[test]
    fn function_keys_print_and_parse() {
        let code = control::encode(&[Control::Named(control::NamedKey::F(2))]);
        assert_eq!(code, "--1XF2--");
        assert_eq!(control::parse("XF12").unwrap(), vec![Control::Named(control::NamedKey::F(12))]);
        assert!(control::parse("XF13").is_err());
        assert!(control::parse("XF0").is_err());
    }

    #[test]
    fn a_key_can_repeat_and_the_menu_asks_how_often() {
        use control::NamedKey::{BackTab, Tab};
        assert_eq!(control::parse("XT3").unwrap(), vec![Control::NamedTimes(Tab, 3)]);
        assert_eq!(control::parse("xb12").unwrap(), vec![Control::NamedTimes(BackTab, 12)]);
        assert_eq!(control::encode(&[Control::NamedTimes(Tab, 3)]), "--1XT3--");
        for bad in ["XT0", "XT100", "XF12X", "XF3X2", "X3"] {
            assert!(control::parse(bad).is_err(), "{bad}");
        }
        let mut p = CodesPanel::new();
        p.list.select(Some(1));
        p.handle(&key(KeyCode::Enter));
        let entries = control::entries(Category::Navigation);
        p.list.select(Some(entries.iter().position(|e| e.label == "Shift+Tab several times").unwrap()));
        assert_eq!(p.handle(&key(KeyCode::Enter)), CodesOutcome::Changed, "asks");
        if let View::Ask { ask, .. } = &mut p.view {
            ask.text.set_text("4".to_string());
        }
        assert_eq!(payload(p.handle(&key(KeyCode::Enter))), "--1XB4--");
    }

    #[test]
    fn a_double_click_on_a_row_opens_it_and_a_single_one_only_selects() {
        use crossterm::event::{KeyModifiers, MouseEvent};
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut p = CodesPanel::new();
        let theme = crate::ui::theme::Theme::default().with_touch(true);
        let mut term = Terminal::new(TestBackend::new(100, 44)).unwrap();
        term.draw(|f| p.render(f, f.area(), &theme)).unwrap();
        let press = |kind, x, y| {
            Event::Mouse(MouseEvent {
                kind,
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            })
        };
        // One click (down and up) on the second category selects it and
        // nothing more.
        let row = p.list.row_areas[1];
        let (x, y) = (row.x + 2, row.y + 1);
        assert_eq!(p.handle(&press(MouseEventKind::Down(MouseButton::Left), x, y)), CodesOutcome::Changed);
        assert_eq!(p.handle(&press(MouseEventKind::Up(MouseButton::Left), x, y)), CodesOutcome::Changed);
        assert_eq!(p.list.selected(), Some(1));
        assert_eq!(p.category(), None, "a single click does not open");
        // A quick second click on another row is a click on that row, not a
        // double click: it selects and does nothing more.
        let other = p.list.row_areas[2];
        p.handle(&press(MouseEventKind::Down(MouseButton::Left), other.x + 2, other.y + 1));
        p.handle(&press(MouseEventKind::Up(MouseButton::Left), other.x + 2, other.y + 1));
        assert_eq!(p.list.selected(), Some(2));
        assert_eq!(p.category(), None, "two rows, two clicks");
        // The same row again right away makes the double click: it opens.
        p.handle(&press(MouseEventKind::Down(MouseButton::Left), other.x + 2, other.y + 1));
        assert_eq!(p.handle(&press(MouseEventKind::Up(MouseButton::Left), other.x + 2, other.y + 1)), CodesOutcome::Changed);
        assert_eq!(p.category(), Some(Category::DataEntry));
    }

    #[test]
    fn a_long_list_grows_arrows_that_page_the_selection() {
        use crossterm::event::{KeyModifiers, MouseEvent};
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut p = CodesPanel::new();
        p.handle(&key(KeyCode::Enter)); // next scanned task: thirty odd entries
        let theme = crate::ui::theme::Theme::default().with_touch(true);
        let mut term = Terminal::new(TestBackend::new(100, 44)).unwrap();
        // The controls follow the list by one frame: draw, then draw again.
        for _ in 0..2 {
            term.draw(|f| p.render(f, f.area(), &theme)).unwrap();
        }
        assert!(p.arrows.up.is_none(), "at the top");
        let down = p.arrows.down.expect("more below");
        assert_eq!(down.height, 1, "one row, as drawn");
        assert!(down.width > 60, "the full width of the box: {down:?}");
        // The list box ends above the error row and the three-row buttons,
        // inside the frame: the bar is flush with its bottom edge.
        assert_eq!(down.bottom(), p.area.bottom() - 1 - 1 - 3, "the bar sits at the bottom of the box: {down:?} in {:?}", p.area);
        let tap = |r: Rect| {
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: r.x + 1,
                row: r.y,
                modifiers: KeyModifiers::NONE,
            })
        };
        assert_eq!(p.handle(&tap(down)), CodesOutcome::Changed);
        let after = p.list.selected().unwrap();
        assert!(after > 0, "a page down: {after}");
        for _ in 0..2 {
            term.draw(|f| p.render(f, f.area(), &theme)).unwrap();
        }
        assert!(p.list.offset() > 0, "and the list scrolled with it");
        let up = p.arrows.up.expect("now something is above");
        assert_eq!(up.height, 1);
        // Back to the categories: the short list starts at the top, not
        // scrolled to where the long one was.
        p.handle(&key(KeyCode::Esc));
        assert_eq!(p.list.offset(), 0);
        assert_eq!(p.list.selected(), Some(0));
        p.handle(&key(KeyCode::Enter));
        for _ in 0..2 {
            term.draw(|f| p.render(f, f.area(), &theme)).unwrap();
        }
        // Flush with the top of the box, under the frame and the two note rows.
        assert_eq!(up.y, p.area.y + 1 + 2, "the bar sits at the top of the box: {up:?} in {:?}", p.area);
        p.handle(&tap(up));
        assert!(p.list.selected().unwrap() < after);
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
