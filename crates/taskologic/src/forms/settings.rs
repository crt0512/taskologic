//! The settings screen: this user's preferences and timezone. The printer
//! profile moved to its own subwindow, `forms::printer`; this screen only
//! shows a summary and the button that opens it.

use chrono_tz::Tz;
use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Clear, Paragraph};
use taskologic_core::barcode::Magic;
use taskologic_core::prefs::{
    AutoprintFilter, CardFields, CustomColors, PrintMode, PrintPrefs, ScannerPrefs, ThemePreset,
    UiPrefs, UserPrefs, parse_reminder_hours, reminder_hours_text,
};
use taskologic_core::print::Symbology;
use taskologic_core::user::User;
use taskologic_print::DeviceProfile;

use super::printer::profile_summary;
use super::{Row, button_h, button_row, check_w, frame_block, label, popup, split_label};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, CheckboxState, ChoiceState, Focus, FocusBuilder, HandleEvent,
    HasFocus, HasScreenCursor, Outcome, Regular, TextInputState, checkbox_at, dropdown,
    dropdown_marker, dropdown_popup_hover, field, render_button,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum SettingsOutcome {
    Continue,
    Changed,
    Cancel,
    Save {
        prefs: UserPrefs,
        timezone: Option<Tz>,
    },
    /// Open the printer setup subwindow.
    Printer,
    /// Open the custom colour editor with these colours.
    EditColors(Box<CustomColors>),
}

const MODE_ITEMS: [(PrintMode, &str); 3] = [
    (PrintMode::Manual, "manually"),
    (PrintMode::OnAdd, "when added"),
    (PrintMode::OnStart, "when started"),
];
const FORMAT_ITEMS: [(Symbology, &str); 4] = [
    (Symbology::Code39, "CODE39"),
    (Symbology::Code128, "CODE128"),
    (Symbology::Qr, "QR"),
    (Symbology::DataMatrix, "DataMatrix"),
];
const MAGIC_ITEMS: [(Magic, &str); 2] = [(Magic::Dots, ".. dots"), (Magic::Dashes, "-- dashes")];
const THEME_ITEMS: [(ThemePreset, &str); 3] = [
    (ThemePreset::Default, "default"),
    (ThemePreset::Dark, "dark"),
    (ThemePreset::Custom, "custom"),
];

fn check(name: &str, value: bool) -> CheckboxState {
    let mut c = CheckboxState::named(name);
    c.set_checked(value);
    c
}

pub struct SettingsForm {
    theme: ChoiceState<ThemePreset>,
    colors: CustomColors,
    edit_colors: ButtonState,
    board_tabs: CheckboxState,
    touchscreen: CheckboxState,
    scanner: CheckboxState,
    show_date: CheckboxState,
    show_time: CheckboxState,
    timezone: TextInputState,
    original_tz: Tz,
    card_default: CheckboxState,
    card_start: CheckboxState,
    card_due: CheckboxState,
    card_assignees: CheckboxState,
    card_deps: CheckboxState,
    card_description: CheckboxState,
    card_short_id: CheckboxState,
    print_button: CheckboxState,
    /// Lead times before each of a task's two dates, in hours.
    reminder_start_hours: TextInputState,
    reminder_hours: TextInputState,
    mode: ChoiceState<PrintMode>,
    filter_on: CheckboxState,
    filter_assigned: CheckboxState,
    filter_created: CheckboxState,
    filter_unless: CheckboxState,
    presses_enter: CheckboxState,
    manual_only: CheckboxState,
    prefix: TextInputState,
    format: ChoiceState<Symbology>,
    magic: ChoiceState<Magic>,
    printer_summary: String,
    printer_btn: ButtonState,
    save: ButtonState,
    cancel: ButtonState,
    mouse_seen: bool,
    pub error: Option<String>,
    pub saving: bool,
}

impl SettingsForm {
    pub fn new(user: &User, printer: Option<&DeviceProfile>, mouse_seen: bool) -> Self {
        let p = &user.prefs;
        let board_tabs = check("board_tabs", p.ui.show_board_tabs);
        board_tabs.focus().set(true);
        let mut timezone = TextInputState::named("timezone");
        timezone.set_text(user.timezone.name());
        let mut reminder_start_hours = TextInputState::named("reminder_start_hours");
        if let Some(m) = p.print.reminder_start_minutes {
            reminder_start_hours.set_text(reminder_hours_text(m));
        }
        let mut reminder_hours = TextInputState::named("reminder_hours");
        if let Some(m) = p.print.reminder_due_minutes {
            reminder_hours.set_text(reminder_hours_text(m));
        }
        let mut mode = ChoiceState::named("mode");
        mode.set_value(p.print.mode);
        let filter = p.print.autoprint_filter.clone().unwrap_or_default();
        let mut prefix = TextInputState::named("prefix");
        prefix.set_text(p.scanner.prefix.clone());
        let mut format = ChoiceState::named("format");
        format.set_value(p.scanner.format);
        let mut magic = ChoiceState::named("magic");
        magic.set_value(p.scanner.magic);
        let cards = p.ui.card_fields.unwrap_or_default();
        let mut theme = ChoiceState::named("theme");
        theme.set_value(p.ui.theme);
        Self {
            theme,
            colors: p.ui.custom_colors.clone(),
            edit_colors: ButtonState::new(),
            board_tabs,
            touchscreen: check("touchscreen", p.ui.touchscreen),
            scanner: check("scanner", p.ui.scanner_enabled),
            show_date: check("show_date", p.ui.show_date),
            show_time: check("show_time", p.ui.show_time),
            timezone,
            original_tz: user.timezone,
            card_default: check("card_default", p.ui.card_fields.is_none()),
            card_start: check("card_start", cards.start_date),
            card_due: check("card_due", cards.due_date),
            card_assignees: check("card_assignees", cards.assignees),
            card_deps: check("card_deps", cards.dependencies),
            card_description: check("card_description", cards.description),
            card_short_id: check("card_short_id", cards.short_id),
            print_button: check("print_button", p.print.show_print_button),
            reminder_start_hours,
            reminder_hours,
            mode,
            filter_on: check("filter_on", p.print.autoprint_filter.is_some()),
            filter_assigned: check("filter_assigned", filter.assigned_to_me),
            filter_created: check("filter_created", filter.created_by_me),
            filter_unless: check("filter_unless", filter.unless_not_assigned_to_me),
            presses_enter: check("presses_enter", p.scanner.presses_enter),
            manual_only: check("manual_only", p.scanner.manual_only),
            prefix,
            format,
            magic,
            printer_summary: profile_summary(printer),
            printer_btn: ButtonState::new(),
            save: ButtonState::new(),
            cancel: ButtonState::new(),
            mouse_seen,
            error: None,
            saving: false,
        }
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        b.widget(&self.theme);
        if self.theme.value() == ThemePreset::Custom {
            b.widget(&self.edit_colors);
        }
        b.widget(&self.board_tabs)
            .widget(&self.touchscreen)
            .widget(&self.scanner)
            .widget(&self.show_date)
            .widget(&self.show_time)
            .widget(&self.timezone)
            .widget(&self.card_default);
        if !self.card_default.checked() {
            b.widget(&self.card_start)
                .widget(&self.card_due)
                .widget(&self.card_assignees)
                .widget(&self.card_deps)
                .widget(&self.card_description)
                .widget(&self.card_short_id);
        }
        b.widget(&self.print_button)
            .widget(&self.reminder_start_hours)
            .widget(&self.reminder_hours)
            .widget(&self.mode)
            .widget(&self.filter_on);
        if self.filter_on.checked() {
            b.widget(&self.filter_assigned)
                .widget(&self.filter_created)
                .widget(&self.filter_unless);
        }
        b.widget(&self.presses_enter)
            .widget(&self.manual_only)
            .widget(&self.prefix)
            .widget(&self.format)
            .widget(&self.magic)
            .widget(&self.printer_btn)
            .widget(&self.save)
            .widget(&self.cancel);
        b.build()
    }

    pub fn handle(&mut self, ev: &Event) -> SettingsOutcome {
        if self.saving {
            return SettingsOutcome::Continue;
        }
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        // An open dropdown eats Esc to close itself.
        let popup_open = self.mode.is_popup_active()
            || self.format.is_popup_active()
            || self.magic.is_popup_active()
            || self.theme.is_popup_active();
        match key {
            Some(KeyCode::Esc) if !popup_open => return SettingsOutcome::Cancel,
            Some(KeyCode::F(2)) => return self.try_save(),
            _ => {}
        }
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return SettingsOutcome::Changed;
        }
        if self.save.handle(ev, Regular) == ButtonOutcome::Pressed {
            return self.try_save();
        }
        if self.cancel.handle(ev, Regular) == ButtonOutcome::Pressed {
            return SettingsOutcome::Cancel;
        }
        if self.printer_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
            return SettingsOutcome::Printer;
        }
        if self.edit_colors.handle(ev, Regular) == ButtonOutcome::Pressed {
            return SettingsOutcome::EditColors(Box::new(self.colors.clone()));
        }
        for c in [
            &mut self.board_tabs,
            &mut self.touchscreen,
            &mut self.scanner,
            &mut self.show_date,
            &mut self.show_time,
            &mut self.card_default,
            &mut self.card_start,
            &mut self.card_due,
            &mut self.card_assignees,
            &mut self.card_deps,
            &mut self.card_description,
            &mut self.card_short_id,
            &mut self.print_button,
            &mut self.filter_on,
            &mut self.filter_assigned,
            &mut self.filter_created,
            &mut self.filter_unless,
            &mut self.presses_enter,
            &mut self.manual_only,
        ] {
            c.handle(ev, Regular);
        }
        self.timezone.handle(ev, Regular);
        self.reminder_start_hours.handle(ev, Regular);
        self.reminder_hours.handle(ev, Regular);
        self.prefix.handle(ev, Regular);
        self.mode.handle(ev, Regular);
        self.format.handle(ev, Regular);
        self.magic.handle(ev, Regular);
        self.theme.handle(ev, Regular);
        SettingsOutcome::Changed
    }

    fn try_save(&mut self) -> SettingsOutcome {
        match self.values() {
            Ok((prefs, timezone)) => {
                self.error = None;
                SettingsOutcome::Save { prefs, timezone }
            }
            Err(e) => {
                self.error = Some(e);
                SettingsOutcome::Changed
            }
        }
    }

    /// The colour editor answered, keep what it produced.
    pub fn set_colors(&mut self, colors: CustomColors) {
        self.colors = colors;
    }

    /// The printer subwindow saved, refresh the summary line.
    pub fn set_printer(&mut self, printer: Option<&DeviceProfile>) {
        self.printer_summary = profile_summary(printer);
    }

    /// What the screen should look like while this form is open, so a theme
    /// change shows the moment it is picked.
    pub fn preview(&self) -> (ThemePreset, CustomColors) {
        (self.theme.value(), self.colors.clone())
    }

    pub fn values(&self) -> Result<(UserPrefs, Option<Tz>), String> {
        let reminder_start_minutes = parse_reminder_hours(self.reminder_start_hours.text())?;
        let reminder_due_minutes = parse_reminder_hours(self.reminder_hours.text())?;
        let tz_text = self.timezone.text().trim().to_string();
        let tz: Tz = tz_text
            .parse()
            .map_err(|_| format!("{tz_text:?} is not a timezone name like Europe/Berlin"))?;
        let prefs = UserPrefs {
            ui: UiPrefs {
                show_board_tabs: self.board_tabs.checked(),
                theme: self.theme.value(),
                custom_colors: self.colors.clone(),
                card_fields: (!self.card_default.checked()).then(|| CardFields {
                    start_date: self.card_start.checked(),
                    due_date: self.card_due.checked(),
                    assignees: self.card_assignees.checked(),
                    dependencies: self.card_deps.checked(),
                    description: self.card_description.checked(),
                    short_id: self.card_short_id.checked(),
                }),
                touchscreen: self.touchscreen.checked(),
                scanner_enabled: self.scanner.checked(),
                show_date: self.show_date.checked(),
                show_time: self.show_time.checked(),
            },
            print: PrintPrefs {
                show_print_button: self.print_button.checked(),
                reminder_start_minutes,
                reminder_due_minutes,
                mode: self.mode.value(),
                autoprint_filter: self.filter_on.checked().then(|| AutoprintFilter {
                    assigned_to_me: self.filter_assigned.checked(),
                    created_by_me: self.filter_created.checked(),
                    unless_not_assigned_to_me: self.filter_unless.checked(),
                }),
            },
            scanner: ScannerPrefs {
                presses_enter: self.presses_enter.checked(),
                manual_only: self.manual_only.checked(),
                format: self.format.value(),
                prefix: self.prefix.text().to_string(),
                magic: self.magic.value(),
            },
        };
        Ok((prefs, (tz != self.original_tz).then_some(tz)))
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let bh = button_h(t);
        let pad = if t.touch { 2 } else { 0 };
        let p = popup(area, 78, 20 + bh);
        f.render_widget(Clear, p);
        let hint = if self.saving {
            " saving... "
        } else {
            " Tab moves   Space toggles   F2 saves   Esc cancels "
        };
        let block = frame_block(" Settings ", hint, t);
        let inner = block.inner(p);
        f.render_widget(block, p);
        let mut constraints = vec![Constraint::Length(1); 18];
        constraints.push(Constraint::Length(bh));
        let rows = Layout::vertical(constraints).split(inner);
        let lw = 11;

        let (l, w) = split_label(rows[0], lw);
        label(f, l, "Theme", t);
        let mut r = Row::new(w);
        let theme_area = r.take(13);
        let (theme_w, theme_popup) = dropdown(THEME_ITEMS, theme_area, t);
        f.render_stateful_widget(theme_w, theme_area, &mut self.theme);
        dropdown_marker(f, &self.theme, t);
        if self.theme.value() == ThemePreset::Custom {
            let ec = r.take(super::button_w(" Edit colours ") + pad);
            render_button(f, ec, " Edit colours ", &mut self.edit_colors, t);
        } else {
            f.render_widget(
                Paragraph::new("pick custom to choose your own colours").style(t.surface_dim()),
                r.rest(),
            );
        }

        let (l, w) = split_label(rows[1], lw);
        label(f, l, "Interface", t);
        let mut r = Row::new(w);
        let cb = r.take(check_w("board tabs"));
        f.render_stateful_widget(
            checkbox_at("board tabs".into(), cb, t),
            cb,
            &mut self.board_tabs,
        );
        let cb = r.take(check_w("bigger buttons"));
        f.render_stateful_widget(
            checkbox_at("bigger buttons".into(), cb, t),
            cb,
            &mut self.touchscreen,
        );
        let cb = r.take(check_w("barcode scanner"));
        f.render_stateful_widget(
            checkbox_at("barcode scanner".into(), cb, t),
            cb,
            &mut self.scanner,
        );

        // Five checkboxes do not fit on one row, so Interface runs on.
        let (_, w) = split_label(rows[2], lw);
        let mut r = Row::new(w);
        let cb = r.take(check_w("show date"));
        f.render_stateful_widget(
            checkbox_at("show date".into(), cb, t),
            cb,
            &mut self.show_date,
        );
        let cb = r.take(check_w("show time"));
        f.render_stateful_widget(
            checkbox_at("show time".into(), cb, t),
            cb,
            &mut self.show_time,
        );

        let (_, w) = split_label(rows[3], lw);
        let mouse = if self.mouse_seen {
            "mouse events are arriving, clicking and dragging work"
        } else {
            "no mouse events yet: your terminal may not forward them"
        };
        f.render_widget(Paragraph::new(mouse).style(t.surface_dim()), w);

        let (l, w) = split_label(rows[4], lw);
        label(f, l, "Timezone", t);
        let mut r = Row::new(w);
        f.render_stateful_widget(field(t), r.take(26), &mut self.timezone);
        f.render_widget(
            Paragraph::new("IANA name, e.g. Europe/Berlin").style(t.surface_dim()),
            r.rest(),
        );

        let (l, w) = split_label(rows[5], lw);
        label(f, l, "Cards show", t);
        let mut r = Row::new(w);
        let cb = r.take(check_w("what the board says"));
        f.render_stateful_widget(
            checkbox_at("what the board says".into(), cb, t),
            cb,
            &mut self.card_default,
        );
        if self.card_default.checked() {
            f.render_widget(
                Paragraph::new("untick to choose for yourself").style(t.surface_dim()),
                r.rest(),
            );
        }
        let (_, w) = split_label(rows[6], lw);
        if !self.card_default.checked() {
            let mut r = Row::new(w);
            let cb = r.take(check_w("start"));
            f.render_stateful_widget(
                checkbox_at("start".into(), cb, t),
                cb,
                &mut self.card_start,
            );
            let cb = r.take(check_w("due date"));
            f.render_stateful_widget(
                checkbox_at("due date".into(), cb, t),
                cb,
                &mut self.card_due,
            );
            let cb = r.take(check_w("assignees"));
            f.render_stateful_widget(
                checkbox_at("assignees".into(), cb, t),
                cb,
                &mut self.card_assignees,
            );
            let cb = r.take(check_w("dependencies"));
            f.render_stateful_widget(
                checkbox_at("dependencies".into(), cb, t),
                cb,
                &mut self.card_deps,
            );
            let cb = r.take(check_w("description"));
            f.render_stateful_widget(
                checkbox_at("description".into(), cb, t),
                cb,
                &mut self.card_description,
            );
            let cb = r.take(check_w("id"));
            f.render_stateful_widget(checkbox_at("id".into(), cb, t), cb, &mut self.card_short_id);
        }

        let (l, w) = split_label(rows[7], lw);
        label(f, l, "Slips show", t);
        let mut r = Row::new(w);
        let cb = r.take(check_w("print button"));
        f.render_stateful_widget(
            checkbox_at("print button".into(), cb, t),
            cb,
            &mut self.print_button,
        );
        f.render_widget(
            Paragraph::new("what a slip shows is set up with the printer, under Printer")
                .style(t.surface_dim()),
            r.rest(),
        );

        let (l, w) = split_label(rows[9], lw);
        label(f, l, "Remind", t);
        let mut r = Row::new(w);
        f.render_stateful_widget(field(t), r.take(6), &mut self.reminder_start_hours);
        f.render_widget(
            Paragraph::new("h before it starts,").style(t.surface_dim()),
            r.text("h before it starts,"),
        );
        f.render_stateful_widget(field(t), r.take(6), &mut self.reminder_hours);
        f.render_widget(
            Paragraph::new("h before it is due; fractions ok, empty for none")
                .style(t.surface_dim()),
            r.rest(),
        );

        let (l, w) = split_label(rows[10], lw);
        label(f, l, "Auto print", t);
        let mut r = Row::new(w);
        let mode_area = r.take(16);
        let (mode_w, mode_popup) = dropdown(MODE_ITEMS, mode_area, t);
        f.render_stateful_widget(mode_w, mode_area, &mut self.mode);
        dropdown_marker(f, &self.mode, t);
        let cb = r.take(check_w("filter what prints"));
        f.render_stateful_widget(
            checkbox_at("filter what prints".into(), cb, t),
            cb,
            &mut self.filter_on,
        );

        let (_, w) = split_label(rows[11], lw);
        if self.filter_on.checked() {
            let mut r = Row::new(w);
            let cb = r.take(check_w("assigned to me"));
            f.render_stateful_widget(
                checkbox_at("assigned to me".into(), cb, t),
                cb,
                &mut self.filter_assigned,
            );
            let cb = r.take(check_w("created by me"));
            f.render_stateful_widget(
                checkbox_at("created by me".into(), cb, t),
                cb,
                &mut self.filter_created,
            );
            let cb = r.take(check_w("unless not mine"));
            f.render_stateful_widget(
                checkbox_at("unless not mine".into(), cb, t),
                cb,
                &mut self.filter_unless,
            );
        }

        let (l, w) = split_label(rows[12], lw);
        label(f, l, "Scanner", t);
        let mut r = Row::new(w);
        let cb = r.take(check_w("presses Enter"));
        f.render_stateful_widget(
            checkbox_at("presses Enter".into(), cb, t),
            cb,
            &mut self.presses_enter,
        );
        let cb = r.take(check_w("manual only"));
        f.render_stateful_widget(
            checkbox_at("manual only".into(), cb, t),
            cb,
            &mut self.manual_only,
        );
        f.render_widget(
            Paragraph::new("prefix").style(t.surface_dim()),
            r.text("prefix"),
        );
        f.render_stateful_widget(field(t), r.take(10), &mut self.prefix);

        let (l, w) = split_label(rows[13], lw);
        label(f, l, "Barcodes", t);
        let mut r = Row::new(w);
        let format_area = r.take(14);
        let (format_w, format_popup) = dropdown(FORMAT_ITEMS, format_area, t);
        f.render_stateful_widget(format_w, format_area, &mut self.format);
        dropdown_marker(f, &self.format, t);
        f.render_widget(
            Paragraph::new("magic").style(t.surface_dim()),
            r.text("magic"),
        );
        let magic_area = r.take(12);
        let (magic_w, magic_popup) = dropdown(MAGIC_ITEMS, magic_area, t);
        f.render_stateful_widget(magic_w, magic_area, &mut self.magic);
        dropdown_marker(f, &self.magic, t);

        let (l, w) = split_label(rows[15], lw);
        label(f, l, "Printer", t);
        let mut r = Row::new(w);
        let pb = r.take(super::button_w(" Printer setup ") + pad);
        render_button(f, pb, " Printer setup ", &mut self.printer_btn, t);
        f.render_widget(
            Paragraph::new(self.printer_summary.clone()).style(t.surface_dim()),
            r.rest(),
        );

        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), rows[16]);
        }
        let (save, cancel) = button_row(rows[18], " Save ", " Cancel ", t);
        render_button(f, save, " Save ", &mut self.save, t);
        render_button(f, cancel, " Cancel ", &mut self.cancel, t);

        // Open dropdown lists draw over everything else.
        f.render_stateful_widget(theme_popup, theme_area, &mut self.theme);
        dropdown_popup_hover(f, &self.theme, t);
        f.render_stateful_widget(mode_popup, mode_area, &mut self.mode);
        dropdown_popup_hover(f, &self.mode, t);
        f.render_stateful_widget(format_popup, format_area, &mut self.format);
        dropdown_popup_hover(f, &self.format, t);
        f.render_stateful_widget(magic_popup, magic_area, &mut self.magic);
        dropdown_popup_hover(f, &self.magic, t);

        if let Some(p) = [
            self.timezone.screen_cursor(),
            self.reminder_start_hours.screen_cursor(),
            self.reminder_hours.screen_cursor(),
            self.prefix.screen_cursor(),
        ]
        .into_iter()
        .flatten()
        .next()
        {
            f.set_cursor_position(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn user() -> User {
        let mut prefs = UserPrefs::default();
        prefs.print.reminder_due_minutes = Some(180);
        User {
            uid: 1,
            username: "alice".into(),
            is_admin: false,
            timezone: chrono_tz::Europe::Berlin,
            prefs,
            has_pin: false,
            created_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        }
    }

    #[test]
    fn round_trips_prefs_and_reports_only_a_changed_timezone() {
        let u = user();
        let form = SettingsForm::new(&u, None, false);
        let (prefs, tz) = form.values().unwrap();
        assert_eq!(prefs, u.prefs);
        assert_eq!(tz, None);
    }

    #[test]
    fn space_toggles_the_focused_checkbox_and_f2_saves() {
        let u = user();
        let mut form = SettingsForm::new(&u, None, false);
        form.handle(&Event::Key(KeyEvent::new(
            KeyCode::Char(' '),
            KeyModifiers::NONE,
        )));
        match form.handle(&Event::Key(KeyEvent::new(
            KeyCode::F(2),
            KeyModifiers::NONE,
        ))) {
            SettingsOutcome::Save { prefs, .. } => assert!(!prefs.ui.show_board_tabs),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_hour_and_a_half_is_kept_as_ninety_minutes() {
        let u = user();
        let mut form = SettingsForm::new(&u, None, false);
        form.reminder_hours.set_text("1.5");
        form.reminder_start_hours.set_text("0.5");
        let print = form.values().unwrap().0.print;
        assert_eq!(print.reminder_due_minutes, Some(90));
        assert_eq!(print.reminder_start_minutes, Some(30));
    }

    #[test]
    fn the_clock_checkboxes_reach_the_prefs() {
        let u = user();
        let mut form = SettingsForm::new(&u, None, false);
        form.show_date.set_checked(true);
        form.show_time.set_checked(true);
        let ui = form.values().unwrap().0.ui;
        assert!(ui.show_date && ui.show_time);
    }

    #[test]
    fn card_fields_default_to_the_board_until_overridden() {
        let u = user();
        let mut form = SettingsForm::new(&u, None, false);
        assert_eq!(form.values().unwrap().0.ui.card_fields, None);
        form.card_default.set_checked(false);
        form.card_short_id.set_checked(true);
        let fields = form.values().unwrap().0.ui.card_fields.unwrap();
        assert!(fields.short_id && fields.due_date);
    }
}
