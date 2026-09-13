//! The only module that names rat-widget, rat-focus or rat-event types.
//!
//! These crates have a small maintainer base and several breaking major
//! versions behind them. Everything else in the client goes through here,
//! so a version bump touches one file. The constructors also pin the look:
//! checkboxes are `[x]` and `[ ]`, single choices are dropdowns, and every
//! widget takes its colours from the theme.

pub use rat_event::util::MouseFlags;
pub use rat_event::{HandleEvent, Outcome, Regular};
pub use rat_focus::{Focus, FocusBuilder, HasFocus, Navigation};
pub use rat_widget::button::{Button, ButtonState};
pub use rat_widget::checkbox::{Checkbox, CheckboxState};
pub use rat_widget::choice::{Choice, ChoicePopup, ChoiceState, ChoiceWidget};
pub use rat_widget::event::ButtonOutcome;
pub use rat_widget::event::TextOutcome;
pub use rat_widget::list::{List, ListState};
pub use rat_widget::text::HasScreenCursor;
pub use rat_widget::text_input::{TextInput, TextInputState};
pub use rat_widget::textarea::{TextArea, TextAreaState, TextWrap};

use rat_widget::checkbox::{CheckboxCheck, CheckboxStyle};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders};

use super::theme::Theme;

/// A single line text input with a labelled border, for the dashboard.
pub fn text_input<'a>(label: &'a str, t: &Theme) -> TextInput<'a> {
    TextInput::new()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(t.border_set())
                .border_style(t.dim())
                .title(label),
        )
        .style(t.base())
        .focus_style(t.field_focus())
        .select_style(t.field_select())
        .cursor_style(t.cursor())
}

/// A borderless field for forms, where the label sits beside it.
pub fn field<'a>(t: &Theme) -> TextInput<'a> {
    TextInput::new()
        .style(t.field())
        .focus_style(t.field_focus())
        .select_style(t.field_select())
        .cursor_style(t.cursor())
}

pub fn text_area<'a>(t: &Theme) -> TextArea<'a> {
    TextArea::new()
        .style(t.field())
        .focus_style(t.field_focus())
        .text_wrap(TextWrap::Hard)
}

/// Hand an event to a multi line field.
///
/// A click past the end of a line leaves rat-text's cursor beyond that
/// line, and the next typed character panics inside the crate. Clamping the
/// cursor first costs nothing and keeps a stray click from killing the
/// session, which matters more here than usual: this binary is a login
/// shell.
pub fn text_area_event(state: &mut TextAreaState, ev: &crossterm::event::Event) -> TextOutcome {
    clamp_text_area(state);
    let out = state.handle(ev, Regular);
    clamp_text_area(state);
    out
}

fn clamp_text_area(state: &mut TextAreaState) {
    let cursor = state.cursor();
    let lines = state.len_lines();
    let row = cursor.y.min(lines.saturating_sub(1));
    let width = state.try_line_width(row).unwrap_or(0);
    if cursor.y != row || cursor.x > width {
        state.set_cursor((width.min(cursor.x), row), false);
    }
}

/// `[x]` and `[ ]`, which is what people expect in a terminal.
///
/// The box and its label share the button background, so it is obvious the
/// whole thing can be clicked rather than just the three characters.
pub fn checkbox<'a>(text: String, t: &Theme) -> Checkbox<'a> {
    Checkbox::new()
        .styles(CheckboxStyle {
            true_str: Some(Span::from("[x]")),
            false_str: Some(Span::from("[ ]")),
            // The crate wants a double click by default. One is enough.
            behave_check: Some(CheckboxCheck::SingleClick),
            ..Default::default()
        })
        .text(text)
        .style(t.button())
        .focus_style(t.button_focus())
}

/// A checkbox that knows where it is, so it can light up under the pointer.
pub fn checkbox_at<'a>(text: String, area: Rect, t: &Theme) -> Checkbox<'a> {
    checkbox(text, t).style(t.hover_if(t.button(), area))
}

/// Single choice, as a dropdown. Radio circles were confusing next to the
/// square checkboxes, and long option lists did not fit on one line.
///
/// Returns the two halves: render the first in place, the second after
/// everything else so the open list draws on top.
pub fn dropdown<'a, T, V>(
    items: impl IntoIterator<Item = (T, V)>,
    area: Rect,
    t: &Theme,
) -> (ChoiceWidget<'a, T>, ChoicePopup<'a, T>)
where
    T: PartialEq + Clone + Default,
    V: Into<ratatui::text::Line<'a>>,
{
    let base = t.hover_if(t.field(), area);
    Choice::new()
        .items(items)
        .style(base)
        .button_style(base)
        .focus_style(t.field_focus())
        .select_style(t.selected())
        .popup_style(t.surface())
        .popup_block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(t.border_set())
                .border_style(t.surface_border()),
        )
        .into_widgets()
}

/// The dropdown crate hardcodes a diamond or triangle on its button. Stamp
/// a plain `v` over it, so the widget looks the same in every terminal.
/// Call this right after rendering the dropdown widget.
pub fn dropdown_marker<T>(f: &mut Frame, state: &ChoiceState<T>, t: &Theme)
where
    T: PartialEq + Clone + Default,
{
    let a = state.button_area;
    if a.width >= 3 && a.height >= 1 {
        let y = a.y + a.height.saturating_sub(1) / 2;
        f.buffer_mut()
            .set_string(a.x, y, " v ", t.hover_if(t.field(), state.area));
    }
}

/// Light up the open list's row under the pointer. The crate highlights
/// only the selected row, so a hovered one would otherwise show nothing.
/// Call this after rendering the popup.
pub fn dropdown_popup_hover<T>(f: &mut Frame, state: &ChoiceState<T>, t: &Theme)
where
    T: PartialEq + Clone + Default,
{
    if !state.is_popup_active() {
        return;
    }
    let Some((x, y)) = t.mouse else { return };
    if let Some(area) = state
        .item_areas
        .iter()
        .find(|a| a.contains(Position::new(x, y)))
    {
        f.buffer_mut().set_style(*area, t.selected());
    }
}

/// Render a button, bordered when the area is tall enough for one. Forms
/// hand out taller areas when "bigger buttons" is on.
pub fn render_button(f: &mut Frame, area: Rect, label: &str, state: &mut ButtonState, t: &Theme) {
    let base = t.hover_if(t.button(), area);
    let b = Button::new(label)
        .style(base)
        .focus_style(t.button_focus())
        .armed_style(t.button_armed());
    let b = if area.height >= 3 {
        b.block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(t.border_set())
                .style(base),
        )
    } else {
        b
    };
    f.render_stateful_widget(b, area, state);
}

/// A list inside a popup, form or dialog, on the pale surface those use.
pub fn list<'a, I>(items: I, t: &Theme) -> List<'a>
where
    I: IntoIterator,
    I::Item: Into<ratatui::widgets::ListItem<'a>>,
{
    styled_list(items, t.surface(), t)
}

/// A list on the desktop itself, for a screen that takes the body of the
/// window the way a board does. Same list, without the sheet of white paper
/// behind it.
pub fn screen_list<'a, I>(items: I, t: &Theme) -> List<'a>
where
    I: IntoIterator,
    I::Item: Into<ratatui::widgets::ListItem<'a>>,
{
    styled_list(items, t.base(), t)
}

fn styled_list<'a, I>(items: I, base: ratatui::style::Style, t: &Theme) -> List<'a>
where
    I: IntoIterator,
    I::Item: Into<ratatui::widgets::ListItem<'a>>,
{
    List::new(items)
        .style(base)
        .select_style(t.selected())
        .focus_style(t.selected())
}
