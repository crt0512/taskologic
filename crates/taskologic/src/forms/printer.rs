//! The printer setup subwindow, opened from the settings screen.
//!
//! The main panel holds the two things every printer needs: which queue to
//! print on and whether it can cut. Everything that only some printers care
//! about lives behind the Advanced button, so the common case is one
//! dropdown and one checkbox.

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use taskologic_core::print::Symbology;
use taskologic_print::{
    Codepage, DeviceProfile, MAX_CUSTOM_MM, MIN_CUSTOM_MM, OutputMode, PaperWidth, Rotation,
    SlipLayout, SlipSection, SlipSet, paper_for_media_mm, queue_name_ok,
};

use super::{Row, button_h, button_row, button_w, frame_block, label, popup, split_label};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, CheckboxState, ChoiceState, Focus, FocusBuilder, HandleEvent,
    HasFocus, HasScreenCursor, Outcome, Regular, TextInputState, checkbox_at, dropdown,
    dropdown_marker, dropdown_popup_hover, field, render_button,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum PrinterOutcome {
    Changed,
    Cancel,
    /// A different queue is selected: go and ask CUPS about its media.
    QueueChanged(String),
    /// Run the queue detection again.
    Refresh,
    /// Print a sample with the profile as it stands, without saving it.
    TestPrint(Box<DeviceProfile>),
    /// What the profile should be now, None removes the printer.
    Save(Option<DeviceProfile>),
}

/// Which of the three panels is up.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
enum Panel {
    #[default]
    Main,
    /// Everything only some printers care about.
    Advanced,
    /// What a slip shows, in what order.
    Slip,
}

/// Where the keys go in the slip panel. It is a grid rather than a row of
/// widgets, so it does its own focus rather than joining the form's ring.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
enum SlipFocus {
    #[default]
    Grid,
    /// The words for the custom line.
    Text,
    /// The switch between receipts and reminders.
    Kind,
    Back,
}

/// The columns of the slip grid, after the section's name.
const SLIP_COLS: [&str; 4] = ["Show", "Bold", "Large", "Centered"];

/// The paper widths the dropdown offers. `Custom` carries its millimetres
/// in a field beside the dropdown rather than in the item itself.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
enum PaperKind {
    Mm58,
    #[default]
    Mm80,
    Custom,
}

const PAPER_ITEMS: [(PaperKind, &str); 3] = [
    (PaperKind::Mm58, "58 mm"),
    (PaperKind::Mm80, "80 mm"),
    (PaperKind::Custom, "Custom"),
];
const OUTPUT_ITEMS: [(OutputMode, &str); 3] = [
    (OutputMode::EscPos, "ESC/POS"),
    (OutputMode::Bitmap, "Bitmap"),
    (OutputMode::Text, "Text only"),
];
const ROTATION_ITEMS: [(Rotation, &str); 4] = [
    (Rotation::Upright, "Upright"),
    (Rotation::Cw180, "Upside down"),
    (Rotation::Cw90, "90 right"),
    (Rotation::Cw270, "90 left"),
];
const CODEPAGE_ITEMS: [(Codepage, &str); 5] = [
    (Codepage::Utf8, "UTF-8"),
    (Codepage::Cp437, "CP437"),
    (Codepage::Cp850, "CP850"),
    (Codepage::Cp858, "CP858"),
    (Codepage::Wpc1252, "WPC1252"),
];
/// Columns a checkbox or a move button takes.
const MARK_W: u16 = 3;

/// A checkbox as this panel's grid draws one.
fn mark(on: bool) -> &'static str {
    if on { "[x]" } else { "[ ]" }
}

/// The middle `w` columns of `area`, for painting something narrower than
/// the space it is allowed without colouring the gap around it.
fn centered_in(area: Rect, w: u16) -> Rect {
    Rect::new(area.x + area.width.saturating_sub(w) / 2, area.y, w.min(area.width), area.height)
}

/// What the queue dropdown calls having no printer at all.
const NO_PRINTER: &str = "(no printer on this client)";

pub fn paper_label(p: PaperWidth) -> String {
    match p {
        PaperWidth::Mm58 => "58 mm".into(),
        PaperWidth::Mm80 => "80 mm".into(),
        PaperWidth::Custom(mm) => format!("{mm} mm printable"),
    }
}

pub fn codepage_label(c: Codepage) -> &'static str {
    match c {
        Codepage::Utf8 => "UTF-8",
        Codepage::Cp437 => "CP437",
        Codepage::Cp850 => "CP850",
        Codepage::Cp858 => "CP858",
        Codepage::Wpc1252 => "WPC1252",
    }
}

/// One line for the settings screen: what printer this client has, if any.
pub fn profile_summary(p: Option<&DeviceProfile>) -> String {
    match p {
        Some(p) => format!(
            "queue {}, {}, {}, {}{}{}",
            p.queue,
            p.output.label(),
            paper_label(p.paper),
            codepage_label(p.codepage),
            if p.auto_cutter { ", cutter" } else { "" },
            if p.use_raw() { ", raw" } else { "" }
        ),
        None => "no printer on this client".into(),
    }
}

pub struct PrinterForm {
    /// Queue names `lpstat -e` reported, empty until detection answers.
    detected: Vec<String>,
    detect_error: Option<String>,
    pub detecting: bool,
    /// The queue, and the only copy of it. The text field in the advanced
    /// panel writes through to this on every keystroke, so there is never a
    /// second answer to which printer was chosen.
    queue: ChoiceState<String>,
    /// Types a queue name detection cannot see, in the advanced panel.
    queue_text: TextInputState,
    cutter: CheckboxState,
    paper: ChoiceState<PaperKind>,
    custom_mm: TextInputState,
    codepage: ChoiceState<Codepage>,
    output: ChoiceState<OutputMode>,
    rotation: ChoiceState<Rotation>,
    raw: CheckboxState,
    /// What each kind of slip shows, edited in its own panel.
    slips: SlipSet,
    /// Which of the two the slip panel is editing.
    editing_reminder: bool,
    /// Media width CUPS reports for the selected queue, once it answers.
    media_mm: Option<u16>,
    /// Set once the paper width has been chosen by hand. After that the
    /// printer's own media stops overriding it; a deliberate choice outranks
    /// a detected one.
    paper_touched: bool,
    /// Kept from the existing profile; there is no UI for it yet.
    symbologies: Vec<Symbology>,
    rescan: ButtonState,
    test_print: ButtonState,
    advanced_btn: ButtonState,
    slip_btn: ButtonState,
    save: ButtonState,
    cancel: ButtonState,
    back: ButtonState,
    /// Which panel is up.
    panel: Panel,
    /// Row and column of the slip grid's cursor.
    slip_at: (usize, usize),
    slip_focus: SlipFocus,
    /// The custom line's words, while the slip panel is open.
    slip_text: TextInputState,
    /// Where each grid cell landed, so a click can find it.
    slip_cells: Vec<(Rect, usize, usize)>,
    /// Where each row's move buttons landed: the rect, the row, and whether
    /// it is the up one.
    slip_moves: Vec<(Rect, usize, bool)>,
    slip_back: ButtonState,
    slip_kind_btn: ButtonState,
    pub error: Option<String>,
}

impl PrinterForm {
    pub fn new(printer: Option<&DeviceProfile>) -> Self {
        let d = printer.cloned().unwrap_or_default();
        let mut queue = ChoiceState::named("queue");
        queue.set_value(d.queue.clone());
        queue.focus().set(true);
        let mut queue_text = TextInputState::named("queue_text");
        queue_text.set_text(d.queue.clone());
        let mut paper = ChoiceState::named("paper");
        paper.set_value(match d.paper {
            PaperWidth::Mm58 => PaperKind::Mm58,
            PaperWidth::Mm80 => PaperKind::Mm80,
            PaperWidth::Custom(_) => PaperKind::Custom,
        });
        let mut custom_mm = TextInputState::named("custom_mm");
        custom_mm.set_text(d.paper.printable_mm().to_string());
        let mut codepage = ChoiceState::named("codepage");
        codepage.set_value(d.codepage);
        let mut output = ChoiceState::named("output");
        output.set_value(d.output);
        let mut rotation = ChoiceState::named("rotation");
        rotation.set_value(d.rotation);
        let mut cutter = CheckboxState::named("cutter");
        cutter.set_checked(d.auto_cutter);
        let mut raw = CheckboxState::named("raw");
        raw.set_checked(d.raw);
        Self {
            detected: Vec::new(),
            detect_error: None,
            detecting: true,
            queue,
            queue_text,
            cutter,
            paper,
            custom_mm,
            codepage,
            output,
            rotation,
            raw,
            slips: d.slips.clone(),
            editing_reminder: false,
            media_mm: None,
            paper_touched: false,
            symbologies: d.native_symbologies,
            rescan: ButtonState::new(),
            test_print: ButtonState::new(),
            advanced_btn: ButtonState::new(),
            slip_btn: ButtonState::new(),
            save: ButtonState::new(),
            cancel: ButtonState::new(),
            back: ButtonState::new(),
            panel: Panel::Main,
            slip_at: (0, 0),
            slip_focus: SlipFocus::Grid,
            slip_text: TextInputState::named("slip_text"),
            slip_cells: Vec::new(),
            slip_moves: Vec::new(),
            slip_back: ButtonState::new(),
            slip_kind_btn: ButtonState::new(),
            error: None,
        }
    }

    /// The queue to ask CUPS about when the panel opens, if there is one.
    pub fn initial_queue(&self) -> Option<String> {
        let q = self.queue_value();
        queue_name_ok(&q).then_some(q)
    }

    /// The detection answered. Nothing is selected on the client's behalf:
    /// an empty queue means no printer, and that is a choice to make, not
    /// one to inherit from whichever queue CUPS happened to list first.
    pub fn set_detected(&mut self, queues: Vec<String>, error: Option<String>) {
        self.detecting = false;
        self.detect_error = error;
        self.detected = queues;
    }

    /// CUPS answered about a queue's media. A late answer for a queue that is
    /// no longer selected is dropped rather than applied to the wrong printer.
    pub fn set_media(&mut self, queue: &str, mm: Option<u16>) {
        if queue != self.queue_value() {
            return;
        }
        self.media_mm = mm;
        if self.paper_touched {
            return;
        }
        if let Some(paper) = mm.and_then(paper_for_media_mm) {
            self.set_paper(paper);
        }
    }

    fn set_paper(&mut self, paper: PaperWidth) {
        self.paper.set_value(match paper {
            PaperWidth::Mm58 => PaperKind::Mm58,
            PaperWidth::Mm80 => PaperKind::Mm80,
            PaperWidth::Custom(_) => PaperKind::Custom,
        });
        self.custom_mm.set_text(paper.printable_mm().to_string());
    }

    /// The queue as it stands.
    fn queue_value(&self) -> String {
        self.queue.value().trim().to_string()
    }

    /// What the dropdown offers: no printer, whatever CUPS listed, and the
    /// configured queue even when detection cannot see it, so that a queue
    /// typed in the advanced panel still shows as the one in use.
    fn queue_items(&self) -> Vec<(String, String)> {
        let mut items = vec![(String::new(), NO_PRINTER.to_string())];
        items.extend(self.detected.iter().map(|q| (q.clone(), q.clone())));
        let current = self.queue_value();
        if !current.is_empty() && !self.detected.contains(&current) {
            items.push((current.clone(), format!("{current} (not detected)")));
        }
        items
    }

    /// Carry the typed queue name over to the dropdown, which holds the
    /// value everything else reads.
    fn sync_typed_queue(&mut self) {
        let typed = self.queue_text.text().trim().to_string();
        if typed != self.queue_value() {
            self.queue.set_value(typed);
        }
    }

    fn open_advanced(&mut self) {
        self.queue_text.set_text(self.queue_value());
        self.panel = Panel::Advanced;
        self.error = None;
    }

    /// The layout the slip panel is editing.
    fn slip(&self) -> &SlipLayout {
        if self.editing_reminder { &self.slips.reminder } else { &self.slips.task }
    }

    fn slip_mut(&mut self) -> &mut SlipLayout {
        if self.editing_reminder { &mut self.slips.reminder } else { &mut self.slips.task }
    }

    /// Swap the panel between receipts and reminders, keeping the edits to
    /// each. The cursor starts at the top of whichever is now shown.
    fn switch_slip_kind(&mut self) {
        self.store_slip_text();
        self.editing_reminder = !self.editing_reminder;
        self.slip_at.0 = 0;
        self.sync_slip_text();
    }

    fn open_slip(&mut self) {
        self.panel = Panel::Slip;
        self.sync_slip_text();
        self.set_slip_focus(SlipFocus::Grid);
        self.error = None;
    }

    /// Move the panel's focus, and tell the widgets. They keep their own flag
    /// and will neither take a key nor draw themselves as focused unless it
    /// is set, so a focus this panel only remembers for itself is one the
    /// person using it cannot see or type into.
    fn set_slip_focus(&mut self, f: SlipFocus) {
        if self.slip_focus == SlipFocus::Text && f != SlipFocus::Text {
            self.store_slip_text();
        }
        let entering_text = f == SlipFocus::Text && self.slip_focus != SlipFocus::Text;
        self.slip_focus = f;
        self.slip_text.focus().set(f == SlipFocus::Text);
        self.slip_kind_btn.focus().set(f == SlipFocus::Kind);
        self.slip_back.focus().set(f == SlipFocus::Back);
        if entering_text {
            self.sync_slip_text();
        }
    }

    /// The text field always shows the custom row's words, wherever that row
    /// has been moved to.
    fn sync_slip_text(&mut self) {
        let text = self.slip().get(SlipSection::Custom).map(|r| r.text.clone()).unwrap_or_default();
        self.slip_text.set_text(text);
    }

    /// Carry the typed words back onto the custom row.
    fn store_slip_text(&mut self) {
        let text = self.slip_text.text().to_string();
        if let Some(row) = self.slip_mut().rows.iter_mut().find(|r| r.section == SlipSection::Custom)
        {
            row.text = text;
        }
    }

    /// Back to the main panel, keeping whatever the panel being left was
    /// editing. Each panel has its own scratch state, so only the one that is
    /// open gets written back: the advanced panel's queue field must not
    /// speak for the slip panel, which never touched it.
    fn close_panel(&mut self) {
        match self.panel {
            Panel::Advanced => self.sync_typed_queue(),
            Panel::Slip => self.store_slip_text(),
            Panel::Main => {}
        }
        self.panel = Panel::Main;
        self.error = None;
    }

    fn popup_open(&self) -> bool {
        self.output.is_popup_active()
            || self.rotation.is_popup_active()
            || self.paper.is_popup_active()
            || self.codepage.is_popup_active()
            || self.queue.is_popup_active()
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        if self.panel == Panel::Advanced {
            b.widget(&self.queue_text).widget(&self.paper);
            if self.paper.value() == PaperKind::Custom {
                b.widget(&self.custom_mm);
            }
            b.widget(&self.output);
            // Each mode only offers what it can actually do: a bitmap is the
            // only thing there is to turn, and character set and raw are
            // ESC/POS notions. The rest render as notes, with nothing to land
            // on.
            match self.output.value() {
                OutputMode::Bitmap => b.widget(&self.rotation),
                OutputMode::EscPos => b.widget(&self.codepage).widget(&self.raw),
                OutputMode::Text => &mut b,
            };
            b.widget(&self.back);
        } else {
            b.widget(&self.queue)
                .widget(&self.rescan)
                .widget(&self.cutter)
                .widget(&self.test_print)
                .widget(&self.slip_btn)
                .widget(&self.advanced_btn)
                .widget(&self.save)
                .widget(&self.cancel);
        }
        b.build()
    }

    pub fn handle(&mut self, ev: &Event) -> PrinterOutcome {
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        let popup_open = self.popup_open();
        match key {
            // An open dropdown eats Esc to close itself.
            Some(KeyCode::Esc) if !popup_open && self.panel != Panel::Main => {
                self.close_panel();
                return PrinterOutcome::Changed;
            }
            Some(KeyCode::Esc) if !popup_open => return PrinterOutcome::Cancel,
            Some(KeyCode::F(2)) => return self.try_save(),
            _ => {}
        }
        if self.panel == Panel::Slip {
            return self.handle_slip(ev, key);
        }
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return PrinterOutcome::Changed;
        }
        match self.panel {
            Panel::Advanced => self.handle_advanced(ev),
            // The slip panel answered above, before the shared focus ring.
            Panel::Slip | Panel::Main => self.handle_main(ev),
        }
    }

    fn handle_main(&mut self, ev: &Event) -> PrinterOutcome {
        if self.save.handle(ev, Regular) == ButtonOutcome::Pressed {
            return self.try_save();
        }
        if self.cancel.handle(ev, Regular) == ButtonOutcome::Pressed {
            return PrinterOutcome::Cancel;
        }
        if self.advanced_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            self.open_advanced();
            return PrinterOutcome::Changed;
        }
        if self.slip_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            self.open_slip();
            return PrinterOutcome::Changed;
        }
        if self.rescan.handle(ev, Regular) == ButtonOutcome::Pressed && !self.detecting {
            self.detecting = true;
            return PrinterOutcome::Refresh;
        }
        if self.test_print.handle(ev, Regular) == ButtonOutcome::Pressed {
            match self.values() {
                Ok(Some(profile)) => {
                    self.error = None;
                    return PrinterOutcome::TestPrint(Box::new(profile));
                }
                Ok(None) => self.error = Some("pick a printer before test printing".into()),
                Err(e) => self.error = Some(e),
            }
            return PrinterOutcome::Changed;
        }
        let was = self.queue_value();
        self.queue.handle(ev, Regular);
        self.cutter.handle(ev, Regular);
        self.queue_changed(&was)
    }

    /// Whether the queue moved, and if so a nudge to go and look up its media.
    /// A queue with an unusable name never becomes an argv entry.
    fn queue_changed(&mut self, was: &str) -> PrinterOutcome {
        let now = self.queue_value();
        if now == was {
            return PrinterOutcome::Changed;
        }
        self.media_mm = None;
        if queue_name_ok(&now) {
            PrinterOutcome::QueueChanged(now)
        } else {
            PrinterOutcome::Changed
        }
    }

    /// The slip grid. Arrows walk it, Space works the cell under the cursor,
    /// and Shift with an arrow carries the whole row up or down the slip.
    fn handle_slip(&mut self, ev: &Event, key: Option<KeyCode>) -> PrinterOutcome {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

        // A click lands straight on a cell, which is how a touchscreen works
        // this panel at all.
        if let Event::Mouse(m) = ev && m.kind == MouseEventKind::Down(MouseButton::Left) {
            let at = ratatui::layout::Position::new(m.column, m.row);
            if let Some(&(_, row, up)) =
                self.slip_moves.iter().find(|(a, _, _)| a.contains(at))
            {
                self.set_slip_focus(SlipFocus::Grid);
                self.slip_at.0 = row;
                self.move_slip_row(up);
                return PrinterOutcome::Changed;
            }
            if let Some(&(_, row, col)) =
                self.slip_cells.iter().find(|(a, _, _)| a.contains(at))
            {
                self.set_slip_focus(SlipFocus::Grid);
                self.slip_at = (row, col);
                self.toggle_slip_cell();
                return PrinterOutcome::Changed;
            }
            if self.slip_text.area.contains(at) {
                self.set_slip_focus(SlipFocus::Text);
                // Forwarded as well, so the caret lands where they tapped.
                self.slip_text.handle(ev, Regular);
                return PrinterOutcome::Changed;
            }
            // The two buttons keep the click, but take the focus with it so
            // the panel shows where it went.
            if self.slip_kind_btn.area.contains(at) {
                self.set_slip_focus(SlipFocus::Kind);
            } else if self.slip_back.area.contains(at) {
                self.set_slip_focus(SlipFocus::Back);
            }
        }
        if self.slip_back.handle(ev, Regular) == ButtonOutcome::Pressed {
            self.close_panel();
            return PrinterOutcome::Changed;
        }
        if self.slip_kind_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            self.switch_slip_kind();
            return PrinterOutcome::Changed;
        }

        let shift = matches!(ev, Event::Key(k) if k.modifiers.contains(KeyModifiers::SHIFT));
        match key {
            Some(KeyCode::Tab) | Some(KeyCode::BackTab) => {
                const RING: [SlipFocus; 4] =
                    [SlipFocus::Grid, SlipFocus::Text, SlipFocus::Kind, SlipFocus::Back];
                let at = RING.iter().position(|f| *f == self.slip_focus).unwrap_or(0);
                let step = if key == Some(KeyCode::BackTab) { RING.len() - 1 } else { 1 };
                self.set_slip_focus(RING[(at + step) % RING.len()]);
                return PrinterOutcome::Changed;
            }
            _ => {}
        }
        match self.slip_focus {
            SlipFocus::Text => {
                self.slip_text.handle(ev, Regular);
                self.store_slip_text();
                return PrinterOutcome::Changed;
            }
            // Both were offered the event above, where their own handler
            // answers Enter and Space. Acting again here would fire twice,
            // which on a switch means nothing happens at all.
            SlipFocus::Kind | SlipFocus::Back => return PrinterOutcome::Changed,
            SlipFocus::Grid => {}
        }

        let last = self.slip().rows.len().saturating_sub(1);
        match key {
            // Shift carries the row itself, so the order is edited with the
            // same two keys that walk it.
            Some(KeyCode::Up) if shift => self.move_slip_row(true),
            Some(KeyCode::Down) if shift => self.move_slip_row(false),
            Some(KeyCode::Up) => self.slip_at.0 = self.slip_at.0.saturating_sub(1),
            Some(KeyCode::Down) => self.slip_at.0 = (self.slip_at.0 + 1).min(last),
            Some(KeyCode::Left) => self.slip_at.1 = self.slip_at.1.saturating_sub(1),
            Some(KeyCode::Right) => self.slip_at.1 = (self.slip_at.1 + 1).min(SLIP_COLS.len() - 1),
            Some(KeyCode::Char(' ')) | Some(KeyCode::Enter) => self.toggle_slip_cell(),
            _ => {}
        }
        PrinterOutcome::Changed
    }

    /// Carry the row under the cursor one place up or down, cursor with it.
    fn move_slip_row(&mut self, up: bool) {
        let at = self.slip_at.0;
        let moved = if up { self.slip_mut().move_up(at) } else { self.slip_mut().move_down(at) };
        if moved {
            self.slip_at.0 = if up { at - 1 } else { at + 1 };
        }
    }

    fn toggle_slip_cell(&mut self) {
        let (row, col) = self.slip_at;
        let Some(r) = self.slip_mut().rows.get_mut(row) else { return };
        match col {
            0 => r.enabled = !r.enabled,
            1 => r.bold = !r.bold,
            2 => r.large = !r.large,
            _ => r.centered = !r.centered,
        }
    }

    fn handle_advanced(&mut self, ev: &Event) -> PrinterOutcome {
        if self.back.handle(ev, Regular) == ButtonOutcome::Pressed {
            self.close_panel();
            return PrinterOutcome::Changed;
        }
        let was = self.queue_value();
        let paper_was = (self.paper.value(), self.custom_mm.text().to_string());
        self.queue_text.handle(ev, Regular);
        self.sync_typed_queue();
        self.paper.handle(ev, Regular);
        self.custom_mm.handle(ev, Regular);
        self.output.handle(ev, Regular);
        self.rotation.handle(ev, Regular);
        self.codepage.handle(ev, Regular);
        self.raw.handle(ev, Regular);
        if paper_was != (self.paper.value(), self.custom_mm.text().to_string()) {
            self.paper_touched = true;
        }
        self.queue_changed(&was)
    }

    fn try_save(&mut self) -> PrinterOutcome {
        match self.values() {
            Ok(profile) => {
                self.error = None;
                PrinterOutcome::Save(profile)
            }
            Err(e) => {
                // The bad field may well be on the other panel.
                self.panel = Panel::Advanced;
                self.error = Some(e);
                PrinterOutcome::Changed
            }
        }
    }

    fn paper_value(&self) -> Result<PaperWidth, String> {
        match self.paper.value() {
            PaperKind::Mm58 => Ok(PaperWidth::Mm58),
            PaperKind::Mm80 => Ok(PaperWidth::Mm80),
            PaperKind::Custom => {
                let text = self.custom_mm.text().trim().to_string();
                let mm: u16 = text.parse().map_err(|_| {
                    format!(
                        "custom paper width must be a whole number of millimetres, not {text:?}"
                    )
                })?;
                if !(MIN_CUSTOM_MM..=MAX_CUSTOM_MM).contains(&mm) {
                    return Err(format!(
                        "custom paper width must be between {MIN_CUSTOM_MM} and {MAX_CUSTOM_MM} mm"
                    ));
                }
                Ok(PaperWidth::Custom(mm))
            }
        }
    }

    fn values(&self) -> Result<Option<DeviceProfile>, String> {
        // The queue name is the one string that reaches the lp subprocess.
        // It is passed as a plain argument, never through a shell, but a
        // tight character set costs nothing and catches typos too.
        let queue = self.queue_value();
        if !queue
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(
                "printer queue names may only use letters, digits, '-', '_' and '.'".into(),
            );
        }
        let paper = self.paper_value()?;
        Ok((!queue.is_empty()).then(|| DeviceProfile {
            queue,
            paper,
            codepage: self.codepage.value(),
            auto_cutter: self.cutter.checked(),
            output: self.output.value(),
            rotation: self.rotation.value(),
            slips: self.slips.clone(),
            raw: self.raw.checked(),
            native_symbologies: self.symbologies.clone(),
        }))
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        match self.panel {
            Panel::Main => self.render_main(f, area, t),
            Panel::Advanced => self.render_advanced(f, area, t),
            Panel::Slip => self.render_slip(f, area, t),
        }
    }

    /// What a slip shows: one row a section, in the order they print.
    fn render_slip(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let bh = button_h(t);
        let pad = if t.touch { 2 } else { 0 };
        let rows_n = self.slip().rows.len() as u16;
        let p = popup(area, 72, rows_n + 5 + bh);
        f.render_widget(Clear, p);
        let hint = " Space works a cell   Shift+Up/Down moves a row   Esc goes back ";
        let title = if self.editing_reminder {
            " Printer setup - what a reminder shows "
        } else {
            " Printer setup - what a receipt shows "
        };
        let block = frame_block(title, hint, t);
        let inner = block.inner(p);
        f.render_widget(block, p);
        let [head, grid, legend, custom, foot] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(rows_n),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(bh),
        ])
        .areas(inner);

        // Section name, then one column a flag, then the two move buttons.
        let name_w = 18;
        let col_w = 9;
        let head_row = Row::new(head);
        let mut r = head_row;
        r.take(name_w);
        for c in SLIP_COLS {
            let a = r.take(col_w);
            f.render_widget(
                Paragraph::new(c).alignment(Alignment::Center).style(t.surface_dim()),
                a,
            );
        }

        self.slip_cells.clear();
        self.slip_moves.clear();
        for (i, row) in self.slip().rows.clone().iter().enumerate() {
            let line = Rect::new(grid.x, grid.y + i as u16, grid.width, 1);
            let on_row = self.slip_focus == SlipFocus::Grid && self.slip_at.0 == i;
            if on_row {
                f.render_widget(Block::default().style(t.selected()), line);
            }
            let mut r = Row::new(line);
            let name = r.take(name_w);
            f.render_widget(Paragraph::new(row.section.label()), name);
            let flags =
                [mark(row.enabled), mark(row.bold), mark(row.large), mark(row.centered)];
            for (c, text) in flags.iter().enumerate() {
                let cell = r.take(col_w);
                // The whole column is the target, so it is easy to hit, but
                // only the box itself is coloured: a highlight stretched
                // across the padding reads as a wide button that is not there.
                let box_area = centered_in(cell, MARK_W);
                let style = if on_row && self.slip_at.1 == c {
                    t.cursor()
                } else {
                    t.hover_if(t.surface(), cell)
                };
                f.render_widget(Paragraph::new(*text).style(style), box_area);
                self.slip_cells.push((cell, i, c));
            }
            // A row moves with these as well as with Shift and an arrow, so
            // the order can be set with a finger and no keyboard at all.
            for (up, glyph) in [(true, " ^ "), (false, " v ")] {
                let a = r.take(MARK_W);
                let usable = if up { i > 0 } else { i + 1 < rows_n as usize };
                let style = if usable { t.hover_if(t.button(), a) } else { t.surface_dim() };
                f.render_widget(Paragraph::new(glyph).style(style), a);
                if usable {
                    self.slip_moves.push((a, i, up));
                }
            }
        }

        let mut r = Row::new(legend);
        let other = if self.editing_reminder { " Show a receipt " } else { " Show a reminder " };
        let kb = r.take(button_w(other) + pad);
        render_button(f, kb, other, &mut self.slip_kind_btn, t);
        f.render_widget(
            Paragraph::new("receipts and reminders are set up apart").style(t.surface_dim()),
            r.rest(),
        );

        let (l, w) = split_label(custom, name_w);
        label(f, l, "Custom words", t);
        let mut r = Row::new(w);
        let field_area = r.take(24);
        f.render_stateful_widget(field(t), field_area, &mut self.slip_text);
        f.render_widget(
            Paragraph::new("what the custom line says").style(t.surface_dim()),
            r.rest(),
        );

        let mut r = Row::new(foot);
        let bb = r.take(button_w(" Back ") + pad);
        render_button(f, Rect::new(bb.x, bb.y, bb.width, bh), " Back ", &mut self.slip_back, t);

        if self.slip_focus == SlipFocus::Text
            && let Some(pos) = self.slip_text.screen_cursor()
        {
            f.set_cursor_position(pos);
        }
    }

    fn render_main(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let bh = button_h(t);
        let pad = if t.touch { 2 } else { 0 };
        let p = popup(area, 78, 9 + 2 * bh);
        f.render_widget(Clear, p);
        let hint = " Tab moves   F2 saves   Esc cancels ";
        let block = frame_block(" Printer setup ", hint, t);
        let inner = block.inner(p);
        f.render_widget(block, p);
        let rows = Layout::vertical([
            Constraint::Length(1), // queue dropdown
            Constraint::Length(1), // detection status
            Constraint::Length(1),
            Constraint::Length(1), // cutter
            Constraint::Length(1), // cutter explanation
            Constraint::Length(1),
            Constraint::Length(bh), // test print / advanced
            Constraint::Length(1),  // error
            Constraint::Length(bh), // save / cancel
        ])
        .split(inner);
        let lw = 11;

        let (l, w) = split_label(rows[0], lw);
        label(f, l, "Printer", t);
        let mut r = Row::new(w);
        let queue_area = r.take(32);
        let (queue_w, queue_popup) = dropdown(self.queue_items(), queue_area, t);
        f.render_stateful_widget(queue_w, queue_area, &mut self.queue);
        dropdown_marker(f, &self.queue, t);
        let rb = r.take(button_w(" Rescan ") + pad);
        render_button(f, rb, " Rescan ", &mut self.rescan, t);

        let (_, w) = split_label(rows[1], lw);
        let status = if self.detecting {
            "looking for printers...".to_string()
        } else {
            match &self.detect_error {
                Some(e) => format!("could not list queues: {e}"),
                None if self.detected.is_empty() => {
                    "none found; set one up in CUPS, or type its name under Advanced".into()
                }
                None => format!("{} queue(s) found on this machine", self.detected.len()),
            }
        };
        f.render_widget(
            Paragraph::new(status)
                .style(t.surface_dim())
                .wrap(Wrap { trim: true }),
            w,
        );

        let (l, w) = split_label(rows[3], lw);
        label(f, l, "Cutter", t);
        let mut r = Row::new(w);
        let cb = r.take(super::check_w("cut after each slip"));
        f.render_stateful_widget(
            checkbox_at("cut after each slip".into(), cb, t),
            cb,
            &mut self.cutter,
        );
        let (_, w) = split_label(rows[4], lw);
        f.render_widget(
            Paragraph::new("leave off if the printer has no blade").style(t.surface_dim()),
            w,
        );

        let (_, w) = split_label(rows[6], lw);
        let mut r = Row::new(w);
        let tp = r.take(button_w(" Test print ") + pad);
        render_button(f, tp, " Test print ", &mut self.test_print, t);
        let sb = r.take(button_w(" Receipt Layout ") + pad);
        render_button(f, sb, " Receipt Layout ", &mut self.slip_btn, t);
        let ab = r.take(button_w(" Advanced ") + pad);
        render_button(f, ab, " Advanced ", &mut self.advanced_btn, t);

        if let Some(e) = &self.error {
            let (_, w) = split_label(rows[7], 0);
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), w);
        }
        let (save, cancel) = button_row(rows[8], " Save ", " Cancel ", t);
        render_button(f, save, " Save ", &mut self.save, t);
        render_button(f, cancel, " Cancel ", &mut self.cancel, t);

        // The open dropdown list draws over everything else.
        f.render_stateful_widget(queue_popup, queue_area, &mut self.queue);
        dropdown_popup_hover(f, &self.queue, t);
    }

    fn render_advanced(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let bh = button_h(t);
        let pad = if t.touch { 2 } else { 0 };
        let p = popup(area, 78, 20 + bh);
        f.render_widget(Clear, p);
        let hint = " Tab moves   F2 saves   Esc goes back ";
        let block = frame_block(" Printer setup - advanced ", hint, t);
        let inner = block.inner(p);
        f.render_widget(block, p);
        let rows = Layout::vertical([
            Constraint::Length(1), // queue name
            Constraint::Length(1), // queue explanation
            Constraint::Length(1),
            Constraint::Length(1), // print as
            Constraint::Length(1), // print as explanation
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1), // paper
            Constraint::Length(1), // paper explanation
            Constraint::Length(1),
            Constraint::Length(1), // character set
            Constraint::Length(1), // character set explanation
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1), // raw
            Constraint::Length(1), // raw explanation
            Constraint::Length(1),
            Constraint::Length(1),  // error
            Constraint::Length(bh), // back
        ])
        .split(inner);
        let lw = 14;

        let (l, w) = split_label(rows[0], lw);
        label(f, l, "Queue", t);
        let mut r = Row::new(w);
        f.render_stateful_widget(field(t), r.take(24), &mut self.queue_text);
        f.render_widget(
            Paragraph::new("empty means no printer on this client").style(t.surface_dim()),
            r.rest(),
        );
        let (_, w) = split_label(rows[1], lw);
        f.render_widget(
            Paragraph::new("type a CUPS queue name here when detection cannot see it")
                .style(t.surface_dim()),
            w,
        );

        let (l, w) = split_label(rows[3], lw);
        label(f, l, "Print as", t);
        let mut r = Row::new(w);
        let out_area = r.take(14);
        let (out_w, out_popup) = dropdown(OUTPUT_ITEMS, out_area, t);
        f.render_stateful_widget(out_w, out_area, &mut self.output);
        dropdown_marker(f, &self.output, t);
        let mode = self.output.value();
        f.render_widget(
            Paragraph::new(match mode {
                OutputMode::EscPos => "the printer's own language",
                OutputMode::Bitmap => "the slip drawn as a picture",
                OutputMode::Text => "plain text, for the driver to set",
            })
            .style(t.surface_dim()),
            r.rest(),
        );
        let (_, w) = split_label(rows[4], lw);
        f.render_widget(
            Paragraph::new(match mode {
                OutputMode::EscPos => {
                    "commands straight to the printer, using its own fonts, barcodes and cutter"
                }
                OutputMode::Bitmap => {
                    "drawn here, rasterised by the queue's driver. Works on any printer CUPS \
                     can drive, and keeps the barcodes"
                }
                OutputMode::Text => {
                    "the driver sets the type. Simplest thing that works on a driver queue, \
                     but no barcodes"
                }
            })
            .style(t.surface_dim())
            .wrap(Wrap { trim: true }),
            Rect::new(w.x, w.y, w.width, 2),
        );

        let (l, w) = split_label(rows[7], lw);
        label(f, l, "Paper", t);
        let mut r = Row::new(w);
        let paper_area = r.take(11);
        let (paper_w, paper_popup) = dropdown(PAPER_ITEMS, paper_area, t);
        f.render_stateful_widget(paper_w, paper_area, &mut self.paper);
        dropdown_marker(f, &self.paper, t);
        let custom = self.paper.value() == PaperKind::Custom;
        if custom {
            f.render_stateful_widget(field(t), r.take(5), &mut self.custom_mm);
            f.render_widget(
                Paragraph::new("mm the printer can print across").style(t.surface_dim()),
                r.rest(),
            );
        } else {
            f.render_widget(
                Paragraph::new("the width of the receipt roll").style(t.surface_dim()),
                r.rest(),
            );
        }
        let (_, w) = split_label(rows[8], lw);
        let paper_help = if let Some(mm) = self.media_mm {
            let agrees = paper_for_media_mm(mm) == self.paper_value().ok();
            format!(
                "the queue's media is {mm} mm wide{}",
                if agrees { ", which is what this is set to" } else { ", which does not match this" }
            )
        } else if custom {
            "printable mm: a 58 mm roll prints 48, an 80 mm roll prints 72".to_string()
        } else {
            format!(
                // How many fit depends on the mode as well as the roll: the
                // bitmap font is wider than the printer's built in one.
                "{} characters per line in this mode",
                self.paper_value().map(|p| mode.columns(p)).unwrap_or(0)
            )
        };
        f.render_widget(
            Paragraph::new(paper_help)
                .style(t.surface_dim())
                .wrap(Wrap { trim: true }),
            w,
        );

        let escpos = mode == OutputMode::EscPos;
        let bitmap = mode == OutputMode::Bitmap;
        let (l, w) = split_label(rows[10], lw);
        label(f, l, if bitmap { "Rotation" } else { "Character set" }, t);
        let mut r = Row::new(w);
        let choice_area = r.take(14);
        let (cp_w, cp_popup) = dropdown(CODEPAGE_ITEMS, choice_area, t);
        let (rot_w, rot_popup) = dropdown(ROTATION_ITEMS, choice_area, t);
        if escpos {
            f.render_stateful_widget(cp_w, choice_area, &mut self.codepage);
            dropdown_marker(f, &self.codepage, t);
            f.render_widget(
                Paragraph::new("how text is put on the wire").style(t.surface_dim()),
                r.rest(),
            );
        } else if bitmap {
            f.render_stateful_widget(rot_w, choice_area, &mut self.rotation);
            dropdown_marker(f, &self.rotation, t);
            f.render_widget(
                Paragraph::new("which way up it goes on the paper").style(t.surface_dim()),
                r.rest(),
            );
        } else {
            f.render_widget(
                Paragraph::new("nothing to choose in this mode").style(t.surface_dim()),
                w,
            );
        }
        let (_, w) = split_label(rows[11], lw);
        f.render_widget(
            Paragraph::new(if escpos {
                "UTF-8 suits anything current. The CP tables are for older printers, \
                 often ones on a serial or parallel port."
            } else if bitmap && self.rotation.value().swaps_sides() {
                "a quarter turn swaps width and height, so the slip has to be shorter \
                 than the roll is wide or the driver will shrink it."
            } else if bitmap {
                "upside down suits a printer whose roll feeds the other way round."
            } else {
                "the queue's driver picks the font and the encoding."
            })
            .style(t.surface_dim())
            .wrap(Wrap { trim: true }),
            Rect::new(w.x, w.y, w.width, 2),
        );

        let (l, w) = split_label(rows[14], lw);
        label(f, l, "Raw", t);
        if escpos {
            let mut r = Row::new(w);
            let cb = r.take(super::check_w("send raw, past the CUPS filters"));
            f.render_stateful_widget(
                checkbox_at("send raw, past the CUPS filters".into(), cb, t),
                cb,
                &mut self.raw,
            );
        } else {
            f.render_widget(
                Paragraph::new("not available in this mode").style(t.surface_dim()),
                w,
            );
        }
        let (_, w) = split_label(rows[15], lw);
        f.render_widget(
            Paragraph::new(if escpos {
                "on for a queue that takes ESC/POS as it is: socket, serial or parallel. \
                 Off lets the queue's own driver filter the job."
            } else {
                "filtering is the point of this mode, so it always happens."
            })
            .style(t.surface_dim())
            .wrap(Wrap { trim: true }),
            Rect::new(w.x, w.y, w.width, 2),
        );

        if let Some(e) = &self.error {
            let (_, w) = split_label(rows[17], 0);
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), w);
        }
        let mut r = Row::new(rows[18]);
        let bb = r.take(button_w(" Back ") + pad);
        render_button(
            f,
            Rect::new(bb.x, bb.y, bb.width, bh),
            " Back ",
            &mut self.back,
            t,
        );

        // Open dropdown lists draw over everything else.
        f.render_stateful_widget(out_popup, out_area, &mut self.output);
        dropdown_popup_hover(f, &self.output, t);
        f.render_stateful_widget(paper_popup, paper_area, &mut self.paper);
        dropdown_popup_hover(f, &self.paper, t);
        if escpos {
            f.render_stateful_widget(cp_popup, choice_area, &mut self.codepage);
            dropdown_popup_hover(f, &self.codepage, t);
        } else if bitmap {
            f.render_stateful_widget(rot_popup, choice_area, &mut self.rotation);
            dropdown_popup_hover(f, &self.rotation, t);
        }

        if let Some(pos) = self
            .queue_text
            .screen_cursor()
            .or_else(|| self.custom_mm.screen_cursor())
        {
            f.set_cursor_position(pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_profile_and_validates_the_queue() {
        let existing = DeviceProfile {
            queue: "receipt".into(),
            paper: PaperWidth::Mm80,
            ..Default::default()
        };
        let mut form = PrinterForm::new(Some(&existing));
        assert_eq!(form.values().unwrap(), Some(existing));
        form.queue.set_value("bad name; rm -rf /".to_string());
        assert!(form.values().unwrap_err().contains("queue"));
        form.queue.set_value(String::new());
        assert_eq!(
            form.values().unwrap(),
            None,
            "clearing the queue removes the printer"
        );
    }

    #[test]
    fn the_dropdown_offers_no_printer_and_everything_detected() {
        let mut form = PrinterForm::new(None);
        form.set_detected(vec!["front_desk".into(), "kitchen".into()], None);
        let labels: Vec<String> = form.queue_items().into_iter().map(|(_, l)| l).collect();
        assert_eq!(labels, [NO_PRINTER, "front_desk", "kitchen"]);
        assert_eq!(
            form.queue_value(),
            "",
            "detection picks no printer on the client's behalf"
        );
    }

    #[test]
    fn a_queue_detection_cannot_see_stays_on_the_dropdown() {
        let existing = DeviceProfile {
            queue: "backroom".into(),
            ..Default::default()
        };
        let mut form = PrinterForm::new(Some(&existing));
        form.set_detected(vec!["front_desk".into()], None);
        let labels: Vec<String> = form.queue_items().into_iter().map(|(_, l)| l).collect();
        assert_eq!(
            labels,
            [NO_PRINTER, "front_desk", "backroom (not detected)"]
        );
    }

    #[test]
    fn typing_a_queue_in_advanced_writes_through_to_the_dropdown() {
        let mut form = PrinterForm::new(None);
        form.open_advanced();
        form.queue_text.set_text("backroom");
        form.sync_typed_queue();
        assert_eq!(form.queue_value(), "backroom");
        form.close_panel();
        assert_eq!(form.panel, Panel::Main);
        assert_eq!(form.values().unwrap().unwrap().queue, "backroom");
    }

    #[test]
    fn opening_advanced_shows_the_queue_already_chosen() {
        let existing = DeviceProfile {
            queue: "kitchen".into(),
            ..Default::default()
        };
        let mut form = PrinterForm::new(Some(&existing));
        form.open_advanced();
        assert_eq!(form.queue_text.text(), "kitchen");
    }

    #[test]
    fn a_custom_paper_width_is_read_from_the_field_beside_it() {
        let existing = DeviceProfile {
            queue: "receipt".into(),
            paper: PaperWidth::Custom(64),
            ..Default::default()
        };
        let mut form = PrinterForm::new(Some(&existing));
        assert_eq!(form.paper.value(), PaperKind::Custom);
        assert_eq!(form.custom_mm.text(), "64");
        assert_eq!(
            form.values().unwrap().unwrap().paper,
            PaperWidth::Custom(64)
        );

        form.custom_mm.set_text("nonsense");
        assert!(form.values().unwrap_err().contains("whole number"));
        form.custom_mm.set_text("2");
        assert!(form.values().unwrap_err().contains("between"));
    }

    #[test]
    fn the_presets_ignore_whatever_the_custom_field_holds() {
        let existing = DeviceProfile {
            queue: "receipt".into(),
            paper: PaperWidth::Mm58,
            ..Default::default()
        };
        let mut form = PrinterForm::new(Some(&existing));
        form.custom_mm.set_text("nonsense");
        assert_eq!(form.values().unwrap().unwrap().paper, PaperWidth::Mm58);
    }

    #[test]
    fn a_bad_field_sends_you_to_the_panel_that_holds_it() {
        let existing = DeviceProfile {
            queue: "receipt".into(),
            paper: PaperWidth::Custom(64),
            ..Default::default()
        };
        let mut form = PrinterForm::new(Some(&existing));
        form.custom_mm.set_text("0");
        assert_eq!(form.try_save(), PrinterOutcome::Changed);
        assert_eq!(form.panel, Panel::Advanced, "the width that failed is an advanced field");
        assert!(form.error.is_some());
    }

    #[test]
    fn a_new_printer_defaults_to_the_common_one() {
        let form = PrinterForm::new(None);
        assert_eq!(form.paper.value(), PaperKind::Mm80);
        assert_eq!(form.codepage.value(), Codepage::Utf8);
        assert!(!form.raw.checked(), "raw printing is opt in");
        assert!(form.cutter.checked());
    }

    #[test]
    fn a_new_printer_takes_the_paper_width_the_queue_is_set_up_for() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("TSP143-STR_T-001".to_string());
        // 71.97 mm of media, as a Star TSP143 reports it, rounded to 72.
        form.set_media("TSP143-STR_T-001", Some(72));
        assert_eq!(form.paper.value(), PaperKind::Mm80);
        assert_eq!(form.values().unwrap().unwrap().paper, PaperWidth::Mm80);
    }

    #[test]
    fn a_width_chosen_by_hand_outranks_the_one_the_printer_reports() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.paper_touched = true;
        form.paper.set_value(PaperKind::Mm58);
        form.set_media("receipt", Some(72));
        assert_eq!(form.paper.value(), PaperKind::Mm58, "the hand picked width stands");
        assert_eq!(form.media_mm, Some(72), "but the panel still says what CUPS reported");
    }

    #[test]
    fn a_late_answer_never_lands_on_a_different_printer() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("kitchen".to_string());
        // An answer for the queue that was selected a moment ago.
        form.set_media("front_desk", Some(48));
        assert_eq!(form.paper.value(), PaperKind::Mm80, "still the default");
        assert_eq!(form.media_mm, None);
    }

    #[test]
    fn an_unusable_queue_name_is_never_looked_up() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("bad name; rm -rf /".to_string());
        assert_eq!(form.queue_changed(""), PrinterOutcome::Changed, "no lookup for a name lp would refuse");
        form.queue.set_value("receipt".to_string());
        assert_eq!(form.queue_changed(""), PrinterOutcome::QueueChanged("receipt".into()));
        assert_eq!(form.initial_queue(), Some("receipt".to_string()));
    }

    #[test]
    fn media_a_receipt_printer_would_never_report_is_ignored() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("office_laser".to_string());
        form.set_media("office_laser", Some(216)); // Letter
        assert_eq!(form.paper.value(), PaperKind::Mm80, "an A4 queue does not reshape a receipt slip");
    }

    #[test]
    fn the_mode_decides_which_settings_are_live() {
        let mut form = PrinterForm::new(None);
        form.open_advanced();
        form.output.set_value(OutputMode::EscPos);
        // ESC/POS is the only mode where a codepage or raw means anything, so
        // it is the only one where Tab can land on them.
        let names = |f: &PrinterForm| format!("{:?}", f.focus());
        assert!(names(&form).contains("codepage"), "{}", names(&form));
        form.output.set_value(OutputMode::Bitmap);
        assert!(!names(&form).contains("codepage"), "{}", names(&form));
        assert!(!names(&form).contains("raw"), "{}", names(&form));
    }

    #[test]
    fn a_filtered_mode_never_asks_for_raw_however_the_box_was_left() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.raw.set_checked(true);
        for (mode, want) in [
            (OutputMode::EscPos, true),
            (OutputMode::Bitmap, false),
            (OutputMode::Text, false),
        ] {
            form.output.set_value(mode);
            let p = form.values().unwrap().unwrap();
            assert_eq!(p.output, mode);
            assert_eq!(p.use_raw(), want, "{mode:?}");
            assert!(p.raw, "the checkbox itself is remembered either way");
        }
    }

    #[test]
    fn the_mode_is_saved_and_shows_in_the_summary() {
        let existing =
            DeviceProfile { queue: "receipt".into(), output: OutputMode::Bitmap, ..Default::default() };
        let form = PrinterForm::new(Some(&existing));
        assert_eq!(form.values().unwrap(), Some(existing.clone()));
        assert!(profile_summary(Some(&existing)).contains("Bitmap"));
    }

    #[test]
    fn a_new_printer_starts_on_the_mode_that_works_on_the_most_printers() {
        let form = PrinterForm::new(None);
        assert_eq!(form.output.value(), OutputMode::Bitmap);
        assert_eq!(form.rotation.value(), Rotation::Upright);
    }

    #[test]
    fn rotation_is_saved_and_only_offered_where_it_does_something() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.open_advanced();
        form.rotation.set_value(Rotation::Cw180);
        assert_eq!(form.values().unwrap().unwrap().rotation, Rotation::Cw180);

        // Only a bitmap is ours to turn, so it is the only mode where Tab
        // reaches the control.
        let names = |f: &PrinterForm| format!("{:?}", f.focus());
        assert!(names(&form).contains("rotation"), "{}", names(&form));
        for mode in [OutputMode::EscPos, OutputMode::Text] {
            form.output.set_value(mode);
            assert!(!names(&form).contains("rotation"), "{mode:?}: {}", names(&form));
        }
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE))
    }

    fn shift(code: KeyCode) -> Event {
        Event::Key(crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::SHIFT))
    }

    #[test]
    fn the_slip_panel_starts_from_the_documented_defaults() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.open_slip();
        assert_eq!(form.panel, Panel::Slip);
        assert_eq!(form.slip_text.text(), "Taskologic", "the custom words come with it");
        let saved = form.values().unwrap().unwrap().slips;
        assert_eq!(saved, SlipSet::default());
    }

    /// Draw the panel so the clickable areas are recorded, then click one.
    fn draw(form: &mut PrinterForm) {
        use crate::ui::theme::Theme;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut t = Terminal::new(TestBackend::new(90, 26)).unwrap();
        let theme = Theme::default();
        t.draw(|f| {
            let a = f.area();
            form.render(f, a, &theme);
        })
        .unwrap();
    }

    fn click(at: Rect) -> Event {
        Event::Mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: at.x,
            row: at.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        })
    }

    #[test]
    fn a_row_can_be_moved_with_the_mouse_alone() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.open_slip();
        draw(&mut form);

        // The down button on the top row.
        let (area, row, _) =
            *form.slip_moves.iter().find(|(_, r, up)| *r == 0 && !*up).unwrap();
        assert_eq!(row, 0);
        form.handle(&click(area));
        assert_eq!(form.slip().rows[1].section, SlipSection::Custom, "it went down one");
        assert_eq!(form.slip_at.0, 1, "and the cursor followed it");

        // Back up again with the up button on its new row.
        draw(&mut form);
        let (area, _, _) = *form.slip_moves.iter().find(|(_, r, up)| *r == 1 && *up).unwrap();
        form.handle(&click(area));
        assert_eq!(form.slip().rows[0].section, SlipSection::Custom);
        assert_eq!(*form.slip(), SlipLayout::task(), "back where it started");
    }

    #[test]
    fn the_ends_of_the_list_offer_no_button_that_would_do_nothing() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.open_slip();
        draw(&mut form);
        let last = form.slip().rows.len() - 1;
        assert!(
            !form.slip_moves.iter().any(|(_, r, up)| *r == 0 && *up),
            "the top row has no up button"
        );
        assert!(
            !form.slip_moves.iter().any(|(_, r, up)| *r == last && !*up),
            "nor the bottom one a down button"
        );
    }

    #[test]
    fn a_click_in_a_column_works_the_box_wherever_in_it_the_pointer_landed() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.open_slip();
        draw(&mut form);
        // Row 1 is the board title, column 0 is Show. The whole column is the
        // target even though only the box itself is coloured.
        let (cell, _, _) =
            *form.slip_cells.iter().find(|(_, r, c)| *r == 1 && *c == 0).unwrap();
        assert!(cell.width > 3, "the target is wider than the box");
        assert!(form.slip().rows[1].enabled);
        form.handle(&click(Rect::new(cell.x, cell.y, 1, 1)));
        assert!(!form.slip().rows[1].enabled, "a click at the left edge still counted");
    }

    fn press(form: &mut PrinterForm, code: KeyCode) {
        form.handle(&key(code));
    }

    #[test]
    fn the_custom_words_can_actually_be_typed_into() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.open_slip();
        // Tab off the grid and onto the field. The widget keeps its own focus
        // flag and ignores every key until it is set, so this is the thing
        // that decides whether typing works at all.
        press(&mut form, KeyCode::Tab);
        assert_eq!(form.slip_focus, SlipFocus::Text);
        assert!(form.slip_text.is_focused(), "the field itself has to know");
        press(&mut form, KeyCode::Char('Z'));
        assert!(form.slip_text.text().contains('Z'), "{:?}", form.slip_text.text());
        press(&mut form, KeyCode::Esc);
        assert!(
            form.values().unwrap().unwrap().slips.task.get(SlipSection::Custom).unwrap().text
                .contains('Z'),
            "what was typed is what was saved"
        );
    }

    #[test]
    fn clicking_the_custom_words_puts_the_focus_there() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.open_slip();
        draw(&mut form);
        assert_eq!(form.slip_focus, SlipFocus::Grid);
        form.handle(&click(form.slip_text.area));
        assert_eq!(form.slip_focus, SlipFocus::Text);
        assert!(form.slip_text.is_focused());
        press(&mut form, KeyCode::Char('Q'));
        assert!(form.slip_text.text().contains('Q'));
    }

    #[test]
    fn the_switch_flips_once_a_press_however_it_was_pressed() {
        use crossterm::event::{MouseButton, MouseEventKind};
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.open_slip();
        draw(&mut form);

        // By mouse: down then up is one press, not two.
        let at = form.slip_kind_btn.area;
        form.handle(&Event::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x,
            row: at.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }));
        form.handle(&Event::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: at.x,
            row: at.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }));
        assert!(form.editing_reminder, "one click, one flip");
        assert_eq!(form.slip_focus, SlipFocus::Kind, "and the focus went with it");

        // By key: the button answers Enter itself, and the panel must not
        // answer it a second time, which would flip it straight back.
        draw(&mut form);
        press(&mut form, KeyCode::Enter);
        assert!(!form.editing_reminder, "one press, one flip");
    }

    #[test]
    fn tab_reaches_every_stop_and_comes_back_round() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.open_slip();
        let mut seen = vec![form.slip_focus];
        for _ in 0..3 {
            press(&mut form, KeyCode::Tab);
            seen.push(form.slip_focus);
        }
        assert_eq!(
            seen,
            [SlipFocus::Grid, SlipFocus::Text, SlipFocus::Kind, SlipFocus::Back]
        );
        press(&mut form, KeyCode::Tab);
        assert_eq!(form.slip_focus, SlipFocus::Grid, "and round again");
        // Backwards too.
        press(&mut form, KeyCode::BackTab);
        assert_eq!(form.slip_focus, SlipFocus::Back);
    }

    #[test]
    fn the_two_kinds_of_slip_are_edited_apart() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.open_slip();
        assert!(!form.editing_reminder, "receipts first");

        // Turn the board title off on the receipt only.
        form.slip_at = (1, 0);
        form.handle(&key(KeyCode::Char(' ')));
        assert!(!form.slips.task.rows[1].enabled);
        assert!(form.slips.reminder.rows[1].enabled, "the reminder is untouched");

        // The same panel shows the reminder, with its own cursor and rows.
        form.switch_slip_kind();
        assert!(form.editing_reminder);
        assert_eq!(form.slip_at.0, 0, "the cursor starts at the top of the new list");
        form.slip_at = (1, 0);
        form.handle(&key(KeyCode::Char(' ')));
        assert!(!form.slips.reminder.rows[1].enabled);

        // And the orders move independently.
        form.slip_at = (3, 0);
        form.handle(&shift(KeyCode::Up));
        assert_ne!(
            form.slips.reminder.rows.iter().map(|r| r.section).collect::<Vec<_>>(),
            form.slips.task.rows.iter().map(|r| r.section).collect::<Vec<_>>(),
            "one order does not follow the other"
        );
    }

    #[test]
    fn space_works_the_cell_under_the_cursor() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.open_slip();
        // Row 1 is the board title, column 1 is Bold.
        form.slip_at = (1, 1);
        assert!(form.slip().rows[1].bold);
        form.handle(&key(KeyCode::Char(' ')));
        assert!(!form.slip().rows[1].bold, "space turned bold off");

        form.slip_at = (1, 0);
        assert!(form.slip().rows[1].enabled);
        form.handle(&key(KeyCode::Char(' ')));
        assert!(!form.slip().rows[1].enabled, "space turned the section off");
        form.handle(&key(KeyCode::Char(' ')));
        assert!(form.slip().rows[1].enabled, "and back on");
    }

    #[test]
    fn shift_and_an_arrow_carries_the_row_with_the_cursor() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.open_slip();
        form.slip_at = (1, 0);
        assert_eq!(form.slip().rows[1].section, SlipSection::BoardTitle);

        form.handle(&shift(KeyCode::Up));
        assert_eq!(form.slip().rows[0].section, SlipSection::BoardTitle, "moved up");
        assert_eq!(form.slip_at.0, 0, "the cursor went with it");

        // And it stops at the top rather than falling off.
        form.handle(&shift(KeyCode::Up));
        assert_eq!(form.slip().rows[0].section, SlipSection::BoardTitle);
        assert_eq!(form.slip_at.0, 0);

        form.handle(&shift(KeyCode::Down));
        assert_eq!(form.slip().rows[1].section, SlipSection::BoardTitle);
        assert_eq!(form.slip_at.0, 1);
    }

    #[test]
    fn the_arrows_alone_only_walk_the_grid() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.open_slip();
        let before = form.slip().clone();
        form.handle(&key(KeyCode::Down));
        form.handle(&key(KeyCode::Right));
        assert_eq!(form.slip_at, (1, 1));
        assert_eq!(*form.slip(), before, "walking changes nothing");
        // The cursor stops at the edges.
        form.slip_at = (0, 0);
        form.handle(&key(KeyCode::Up));
        form.handle(&key(KeyCode::Left));
        assert_eq!(form.slip_at, (0, 0));
    }

    #[test]
    fn the_custom_words_are_kept_when_the_panel_closes() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.open_slip();
        form.slip_focus = SlipFocus::Text;
        form.slip_text.set_text("Bay 4");
        form.handle(&key(KeyCode::Esc));
        assert_eq!(form.panel, Panel::Main, "esc goes back, it does not cancel");
        let saved = form.values().unwrap().unwrap().slips.task;
        assert_eq!(saved.get(SlipSection::Custom).unwrap().text, "Bay 4");
    }

    #[test]
    fn an_edited_layout_is_what_gets_saved_and_test_printed() {
        let mut form = PrinterForm::new(None);
        form.queue.set_value("receipt".to_string());
        form.open_slip();
        form.slip_at = (0, 0);
        form.handle(&key(KeyCode::Char(' '))); // custom line on
        form.handle(&shift(KeyCode::Down)); // and one place down
        form.handle(&key(KeyCode::Esc));

        let saved = form.values().unwrap().unwrap().slips.task;
        assert_eq!(saved.rows[0].section, SlipSection::BoardTitle);
        assert_eq!(saved.rows[1].section, SlipSection::Custom);
        assert!(saved.rows[1].enabled);
        assert_ne!(saved, SlipLayout::task(), "the edit survived the round trip");
    }

    #[test]
    fn summary_reads_like_a_sentence() {
        assert_eq!(profile_summary(None), "no printer on this client");
        let p = DeviceProfile {
            queue: "receipt".into(),
            auto_cutter: true,
            output: OutputMode::EscPos,
            ..Default::default()
        };
        let s = profile_summary(Some(&p));
        assert!(
            s.contains("receipt") && s.contains("cutter") && s.contains("UTF-8"),
            "{s}"
        );
        assert!(!s.contains("raw"), "{s}");
        let raw = DeviceProfile { raw: true, ..p };
        assert!(profile_summary(Some(&raw)).contains("raw"));
        // Raw is an ESC/POS notion, so a bitmap profile never claims it.
        let bitmap = DeviceProfile { output: OutputMode::Bitmap, ..raw };
        let s = profile_summary(Some(&bitmap));
        assert!(s.contains("Bitmap") && !s.contains("raw"), "{s}");
    }
}
