//! Board form: the creation wizard and the settings screen share it. Members
//! and columns of an existing board are managed in their own panels, which
//! this form opens through buttons.

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Clear, Paragraph};
use taskologic_core::board::{
    Board, ColumnRole, DEFAULT_ARCHIVE_AFTER_SECS, DEFAULT_PURGE_DELETED_AFTER_SECS,
    validate_column_names, validate_name,
};
use taskologic_core::ids::{BoardId, ColumnId, Uid};
use taskologic_core::prefs::CardFields;
use taskologic_core::user::UserSummary;
use taskologic_proto::{CreateBoard, UpdateBoard};

use super::{
    Row, button_h, button_row, check_w, days_text, frame_block, label, parse_days, popup,
    split_label,
};
use crate::ui::adapter::{
    ButtonOutcome, ButtonState, CheckboxState, ChoiceState, Focus, FocusBuilder, HandleEvent,
    HasFocus, HasScreenCursor, Navigation, Outcome, Regular, TextAreaState, TextInputState,
    checkbox_at, dropdown, dropdown_marker, dropdown_popup_hover, field, render_button, text_area,
    text_area_event,
};
use crate::ui::theme::Theme;

#[derive(Debug, PartialEq)]
pub enum BoardOutcome {
    Continue,
    Changed,
    Cancel,
    Create(Box<CreateBoard>),
    Update(Box<UpdateBoard>),
    OpenMembers,
    OpenColumns,
}

enum Mode {
    Create,
    Edit(Box<Board>),
}

fn check(name: &str, value: bool) -> CheckboxState {
    let mut c = CheckboxState::named(name);
    c.set_checked(value);
    c
}

pub struct BoardForm {
    mode: Mode,
    me: Uid,
    name: TextInputState,
    description: TextInputState,
    columns: TextAreaState,
    /// Role pickers. Create mode values are 1 based line numbers, edit mode
    /// values are column ids. Zero means nothing picked.
    started: ChoiceState<i64>,
    paused: ChoiceState<i64>,
    finished: ChoiceState<i64>,
    archive_days: TextInputState,
    retention_days: TextInputState,
    card_start: CheckboxState,
    card_due: CheckboxState,
    card_assignees: CheckboxState,
    card_deps: CheckboxState,
    card_description: CheckboxState,
    is_private: CheckboxState,
    is_locked: CheckboxState,
    users: Vec<(Uid, String, CheckboxState)>,
    members_btn: ButtonState,
    columns_btn: ButtonState,
    save: ButtonState,
    cancel: ButtonState,
    pub error: Option<String>,
    pub saving: bool,
}

impl BoardForm {
    fn blank(mode: Mode, me: Uid, cards: CardFields) -> Self {
        let name = TextInputState::named("board_name");
        name.focus().set(true);
        Self {
            mode,
            me,
            name,
            description: TextInputState::named("board_description"),
            columns: TextAreaState::named("columns"),
            started: ChoiceState::named("started"),
            paused: ChoiceState::named("paused"),
            finished: ChoiceState::named("finished"),
            archive_days: TextInputState::named("archive_days"),
            retention_days: TextInputState::named("retention_days"),
            card_start: check("card_start", cards.start_date),
            card_due: check("card_due", cards.due_date),
            card_assignees: check("card_assignees", cards.assignees),
            card_deps: check("card_deps", cards.dependencies),
            card_description: check("card_description", cards.description),
            is_private: CheckboxState::named("private"),
            is_locked: CheckboxState::named("locked"),
            users: Vec::new(),
            members_btn: ButtonState::new(),
            columns_btn: ButtonState::new(),
            save: ButtonState::new(),
            cancel: ButtonState::new(),
            error: None,
            saving: false,
        }
    }

    pub fn create(me: Uid) -> Self {
        let mut f = Self::blank(Mode::Create, me, CardFields::default());
        f.columns.set_text("Todo\nDoing\nWaiting\nDone");
        f.started.set_value(2);
        f.paused.set_value(3);
        f.finished.set_value(4);
        f.archive_days
            .set_text(days_text(DEFAULT_ARCHIVE_AFTER_SECS));
        f.retention_days
            .set_text(days_text(DEFAULT_PURGE_DELETED_AFTER_SECS));
        f
    }

    pub fn edit(board: &Board, me: Uid) -> Self {
        let mut f = Self::blank(Mode::Edit(Box::new(board.clone())), me, board.card_fields);
        f.name.set_text(board.name.clone());
        f.description.set_text(board.description.clone());
        f.started.set_value(board.started_col.0);
        f.paused.set_value(board.paused_col.0);
        f.finished.set_value(board.finished_col.0);
        f.archive_days.set_text(days_text(board.archive_after_secs));
        f.retention_days
            .set_text(days_text(board.purge_deleted_after_secs));
        f.is_private.set_checked(board.is_private);
        f.is_locked.set_checked(board.is_locked);
        f
    }

    pub fn is_edit(&self) -> bool {
        matches!(self.mode, Mode::Edit(_))
    }

    pub fn board_id(&self) -> Option<BoardId> {
        match &self.mode {
            Mode::Edit(b) => Some(b.id),
            Mode::Create => None,
        }
    }

    /// Create mode member picker. The creator is always a member and is
    /// not listed.
    pub fn set_users(&mut self, users: &[UserSummary]) {
        self.users = users
            .iter()
            .filter(|u| u.uid != self.me)
            .map(|u| (u.uid, u.username.clone(), CheckboxState::named(&u.username)))
            .collect();
    }

    /// Edit mode: the board changed underneath (columns renamed, roles
    /// moved). Keep what the user typed, refresh what the pickers offer.
    pub fn refresh(&mut self, board: &Board) {
        if let Mode::Edit(b) = &mut self.mode {
            *b = Box::new(board.clone());
        }
    }

    fn column_choices(&self) -> Vec<(i64, String)> {
        match &self.mode {
            Mode::Create => self
                .columns
                .text()
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .enumerate()
                .map(|(i, n)| (i as i64 + 1, n.to_string()))
                .collect(),
            Mode::Edit(b) => b.columns.iter().map(|c| (c.id.0, c.name.clone())).collect(),
        }
    }

    fn popup_open(&self) -> bool {
        self.started.is_popup_active()
            || self.paused.is_popup_active()
            || self.finished.is_popup_active()
    }

    fn focus(&self) -> Focus {
        let mut b = FocusBuilder::new(None);
        b.widget(&self.name).widget(&self.description);
        if !self.is_edit() {
            // Tab leaves the column list, Enter adds lines inside it.
            b.widget_navigate(&self.columns, Navigation::Regular);
        }
        b.widget(&self.started)
            .widget(&self.paused)
            .widget(&self.finished)
            .widget(&self.archive_days)
            .widget(&self.retention_days)
            .widget(&self.card_start)
            .widget(&self.card_due)
            .widget(&self.card_assignees)
            .widget(&self.card_deps)
            .widget(&self.card_description)
            .widget(&self.is_private)
            .widget(&self.is_locked);
        if !self.is_edit() && self.is_private.checked() {
            for (_, _, c) in &self.users {
                b.widget(c);
            }
        }
        if self.is_edit() {
            b.widget(&self.members_btn).widget(&self.columns_btn);
        }
        b.widget(&self.save).widget(&self.cancel);
        b.build()
    }

    pub fn handle(&mut self, ev: &Event) -> BoardOutcome {
        if self.saving {
            return BoardOutcome::Continue;
        }
        let key = match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => Some(k.code),
            _ => None,
        };
        match key {
            Some(KeyCode::Esc) if !self.popup_open() => return BoardOutcome::Cancel,
            Some(KeyCode::F(2)) => return self.try_save(),
            _ => {}
        }
        let mut focus = self.focus();
        if focus.handle(ev, Regular) == Outcome::Changed && key.is_some() {
            return BoardOutcome::Changed;
        }
        if self.save.handle(ev, Regular) == ButtonOutcome::Pressed {
            return self.try_save();
        }
        if self.cancel.handle(ev, Regular) == ButtonOutcome::Pressed {
            return BoardOutcome::Cancel;
        }
        if self.is_edit() {
            if self.members_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
                return BoardOutcome::OpenMembers;
            }
            if self.columns_btn.handle(ev, Regular) == ButtonOutcome::Pressed {
                return BoardOutcome::OpenColumns;
            }
        }
        self.name.handle(ev, Regular);
        self.description.handle(ev, Regular);
        if !self.is_edit() {
            text_area_event(&mut self.columns, ev);
        }
        self.started.handle(ev, Regular);
        self.paused.handle(ev, Regular);
        self.finished.handle(ev, Regular);
        self.archive_days.handle(ev, Regular);
        self.retention_days.handle(ev, Regular);
        for c in [
            &mut self.card_start,
            &mut self.card_due,
            &mut self.card_assignees,
            &mut self.card_deps,
            &mut self.card_description,
        ] {
            c.handle(ev, Regular);
        }
        self.is_private.handle(ev, Regular);
        self.is_locked.handle(ev, Regular);
        if !self.is_edit() && self.is_private.checked() {
            for (_, _, c) in &mut self.users {
                c.handle(ev, Regular);
            }
        }
        BoardOutcome::Changed
    }

    fn try_save(&mut self) -> BoardOutcome {
        match self.values() {
            Ok(o) => {
                self.error = None;
                o
            }
            Err(e) => {
                self.error = Some(e);
                BoardOutcome::Changed
            }
        }
    }

    fn role_pick(
        &self,
        state: &ChoiceState<i64>,
        role: ColumnRole,
        choices: &[(i64, String)],
    ) -> Result<i64, String> {
        let v = state.value();
        if choices.iter().any(|(id, _)| *id == v) {
            Ok(v)
        } else {
            Err(format!("pick the {} column", role.label()))
        }
    }

    fn card_fields(&self) -> CardFields {
        CardFields {
            start_date: self.card_start.checked(),
            due_date: self.card_due.checked(),
            assignees: self.card_assignees.checked(),
            dependencies: self.card_deps.checked(),
            description: self.card_description.checked(),
            short_id: false,
        }
    }

    pub fn values(&self) -> Result<BoardOutcome, String> {
        let name = self.name.text().trim().to_string();
        validate_name(&name).map_err(|e| e.to_string())?;
        let choices = self.column_choices();
        let started = self.role_pick(&self.started, ColumnRole::Started, &choices)?;
        let paused = self.role_pick(&self.paused, ColumnRole::Paused, &choices)?;
        let finished = self.role_pick(&self.finished, ColumnRole::Finished, &choices)?;
        let archive = parse_days(self.archive_days.text(), "archive delay")?;
        let retention = parse_days(self.retention_days.text(), "deleted task retention")?;
        let cards = self.card_fields();
        match &self.mode {
            Mode::Create => {
                let columns: Vec<String> = choices.iter().map(|(_, n)| n.clone()).collect();
                validate_column_names(&columns).map_err(|e| e.to_string())?;
                let members = if self.is_private.checked() {
                    self.users
                        .iter()
                        .filter(|(_, _, c)| c.checked())
                        .map(|(u, _, _)| *u)
                        .collect()
                } else {
                    Vec::new()
                };
                Ok(BoardOutcome::Create(Box::new(CreateBoard {
                    name,
                    description: self.description.text().trim().to_string(),
                    columns,
                    started_col: started as usize - 1,
                    paused_col: paused as usize - 1,
                    finished_col: finished as usize - 1,
                    archive_after_secs: archive,
                    purge_deleted_after_secs: retention,
                    card_fields: cards,
                    is_private: self.is_private.checked(),
                    is_locked: self.is_locked.checked(),
                    members,
                })))
            }
            Mode::Edit(b) => {
                let mut roles = Vec::new();
                for (role, picked, current) in [
                    (ColumnRole::Started, started, b.started_col),
                    (ColumnRole::Paused, paused, b.paused_col),
                    (ColumnRole::Finished, finished, b.finished_col),
                ] {
                    if picked != current.0 {
                        roles.push((role, ColumnId(picked)));
                    }
                }
                let description = self.description.text().trim().to_string();
                let req = UpdateBoard {
                    board_id: b.id,
                    name: (name != b.name).then_some(name),
                    description: (description != b.description).then_some(description),
                    is_locked: (self.is_locked.checked() != b.is_locked)
                        .then_some(self.is_locked.checked()),
                    is_private: (self.is_private.checked() != b.is_private)
                        .then_some(self.is_private.checked()),
                    archive_after_secs: (archive != b.archive_after_secs).then_some(archive),
                    purge_deleted_after_secs: (retention != b.purge_deleted_after_secs)
                        .then_some(retention),
                    card_fields: (cards != b.card_fields).then_some(cards),
                    roles,
                };
                if req.name.is_none()
                    && req.description.is_none()
                    && req.is_locked.is_none()
                    && req.is_private.is_none()
                    && req.archive_after_secs.is_none()
                    && req.purge_deleted_after_secs.is_none()
                    && req.card_fields.is_none()
                    && req.roles.is_empty()
                {
                    return Err("nothing changed".into());
                }
                Ok(BoardOutcome::Update(Box::new(req)))
            }
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let edit = self.is_edit();
        let choices = self.column_choices();
        let user_rows = if !edit && self.is_private.checked() {
            self.user_rows(60).max(1) as u16
        } else {
            1
        };
        let bh = button_h(t);
        let pad = if t.touch { 2 } else { 0 };
        let height = if edit { 17 + bh } else { 19 + bh + user_rows };
        let p = popup(area, 76, height);
        f.render_widget(Clear, p);
        let title = if edit {
            " Board settings "
        } else {
            " New board "
        };
        let hint = if self.saving {
            " saving... "
        } else {
            " Tab moves   F2 saves   Esc cancels "
        };
        let block = frame_block(title, hint, t);
        let inner = block.inner(p);
        f.render_widget(block, p);
        let lw = 13;

        let mut constraints = vec![Constraint::Length(1), Constraint::Length(1)];
        if !edit {
            constraints.push(Constraint::Length(1));
            constraints.push(Constraint::Length(4));
        }
        constraints.extend([
            Constraint::Length(1), // started
            Constraint::Length(1), // paused
            Constraint::Length(1), // finished
            Constraint::Length(1), // archive
            Constraint::Length(1), // retention
            Constraint::Length(1), // cards
            Constraint::Length(1), // flags
            Constraint::Length(user_rows),
            Constraint::Min(1),
            Constraint::Length(bh),
        ]);
        let rows = Layout::vertical(constraints).split(inner);
        let mut i = 0usize;

        let (l, w) = split_label(rows[i], lw);
        i += 1;
        label(f, l, "Name", t);
        f.render_stateful_widget(field(t), w, &mut self.name);

        let (l, w) = split_label(rows[i], lw);
        i += 1;
        label(f, l, "Description", t);
        f.render_stateful_widget(field(t), w, &mut self.description);

        if !edit {
            let (l, w) = split_label(rows[i], lw);
            i += 1;
            label(f, l, "Columns", t);
            f.render_widget(
                Paragraph::new("one per line, left to right").style(t.surface_dim()),
                w,
            );
            let (_, w) = split_label(rows[i], lw);
            i += 1;
            let [w, _] = Layout::horizontal([Constraint::Length(30), Constraint::Min(0)]).areas(w);
            f.render_stateful_widget(text_area(t), w, &mut self.columns);
        }

        // Role pickers as dropdowns: column names are too long for a row of
        // buttons, and this way the current choice is always readable.
        let mut popups = Vec::new();
        for (role, state) in [
            (ColumnRole::Started, 0usize),
            (ColumnRole::Paused, 1),
            (ColumnRole::Finished, 2),
        ] {
            let (l, w) = split_label(rows[i], lw);
            i += 1;
            label(f, l, role.label(), t);
            // Wide enough for the longest column name plus the marker.
            let longest = choices
                .iter()
                .map(|(_, n)| n.chars().count())
                .max()
                .unwrap_or(0) as u16;
            let [w, _] = Layout::horizontal([
                Constraint::Length((longest + 5).clamp(12, 40)),
                Constraint::Min(0),
            ])
            .areas(w);
            let (widget, popup) = dropdown(choices.clone(), w, t);
            let target = match state {
                0 => &mut self.started,
                1 => &mut self.paused,
                _ => &mut self.finished,
            };
            f.render_stateful_widget(widget, w, target);
            dropdown_marker(f, target, t);
            popups.push((popup, w, state));
        }

        let (l, w) = split_label(rows[i], lw);
        i += 1;
        label(f, l, "Archive", t);
        let mut r = Row::new(w);
        f.render_widget(
            Paragraph::new("finished tasks after").style(t.surface_dim()),
            r.text("finished tasks after"),
        );
        f.render_stateful_widget(field(t), r.take(5), &mut self.archive_days);
        f.render_widget(Paragraph::new("days").style(t.surface_dim()), r.rest());

        let (l, w) = split_label(rows[i], lw);
        i += 1;
        label(f, l, "Purge", t);
        let mut r = Row::new(w);
        f.render_widget(
            Paragraph::new("deleted tasks after").style(t.surface_dim()),
            r.text("deleted tasks after"),
        );
        f.render_stateful_widget(field(t), r.take(5), &mut self.retention_days);
        f.render_widget(
            Paragraph::new("days in the archive").style(t.surface_dim()),
            r.rest(),
        );

        let (l, w) = split_label(rows[i], lw);
        i += 1;
        label(f, l, "Cards show", t);
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

        let (l, w) = split_label(rows[i], lw);
        i += 1;
        label(f, l, "Flags", t);
        let mut r = Row::new(w);
        let cb = r.take(check_w("private"));
        f.render_stateful_widget(
            checkbox_at("private".into(), cb, t),
            cb,
            &mut self.is_private,
        );
        let cb = r.take(check_w("locked"));
        f.render_stateful_widget(checkbox_at("locked".into(), cb, t), cb, &mut self.is_locked);
        let note = if self.is_locked.checked() {
            "locked: unlock before deleting"
        } else {
            "public: everyone is a member"
        };
        let note = if self.is_private.checked() && !self.is_locked.checked() {
            "private: only members see it"
        } else {
            note
        };
        f.render_widget(Paragraph::new(note).style(t.surface_dim()), r.rest());

        let (l, w) = split_label(rows[i], lw);
        i += 1;
        if edit {
            label(f, l, "Manage", t);
            let mut r = Row::new(w);
            let m = r.take(super::button_w(" Members ") + pad);
            render_button(f, m, " Members ", &mut self.members_btn, t);
            let c = r.take(super::button_w(" Columns ") + pad);
            render_button(f, c, " Columns ", &mut self.columns_btn, t);
        } else if self.is_private.checked() {
            label(f, l, "Members", t);
            if self.users.is_empty() {
                f.render_widget(Paragraph::new("loading users...").style(t.surface_dim()), w);
            }
            let mut x = w.x;
            let mut y = w.y;
            for (_, name, c) in &mut self.users {
                let cw = check_w(name) + 1;
                if x + cw > w.right() && x > w.x {
                    x = w.x;
                    y += 1;
                }
                if y >= w.bottom() {
                    break;
                }
                let cb = Rect::new(x, y, cw.min(w.right() - x), 1);
                f.render_stateful_widget(checkbox_at(name.clone(), cb, t), cb, c);
                x += cw;
            }
        } else {
            label(f, l, "Members", t);
            f.render_widget(
                Paragraph::new("everyone, while the board is public").style(t.surface_dim()),
                w,
            );
        }

        let err_row = rows[i];
        i += 1;
        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(e.clone()).style(t.error()), err_row);
        }
        let (save, cancel) = button_row(rows[i], " Save ", " Cancel ", t);
        render_button(f, save, " Save ", &mut self.save, t);
        render_button(f, cancel, " Cancel ", &mut self.cancel, t);

        for (popup, area, which) in popups {
            let target = match which {
                0 => &mut self.started,
                1 => &mut self.paused,
                _ => &mut self.finished,
            };
            f.render_stateful_widget(popup, area, target);
            dropdown_popup_hover(f, target, t);
        }

        if let Some(p) = [
            self.name.screen_cursor(),
            self.description.screen_cursor(),
            self.columns.screen_cursor(),
            self.archive_days.screen_cursor(),
            self.retention_days.screen_cursor(),
        ]
        .into_iter()
        .flatten()
        .next()
        {
            f.set_cursor_position(p);
        }
    }

    fn user_rows(&self, width: usize) -> usize {
        let mut rows = 1;
        let mut x = 0;
        for (_, name, _) in &self.users {
            let cw = name.chars().count() + 5;
            if x + cw > width && x > 0 {
                rows += 1;
                x = 0;
            }
            x += cw;
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taskologic_core::board::test_support::board_with_members;

    #[test]
    fn create_defaults_produce_a_valid_request() {
        let mut form = BoardForm::create(1);
        form.name.set_text("Kitchen");
        match form.values().unwrap() {
            BoardOutcome::Create(req) => {
                assert_eq!(req.columns, vec!["Todo", "Doing", "Waiting", "Done"]);
                assert_eq!(req.description, "");
                assert_eq!(
                    (req.started_col, req.paused_col, req.finished_col),
                    (1, 2, 3)
                );
                assert_eq!(req.archive_after_secs, DEFAULT_ARCHIVE_AFTER_SECS);
                assert!(!req.is_private);
                assert!(req.card_fields.due_date);
            }
            other => panic!("{other:?}"),
        }
        form.columns.set_text("Only");
        assert!(
            form.values().unwrap_err().contains("started"),
            "role points past the columns"
        );
    }

    #[test]
    fn edit_sends_only_what_changed() {
        let board = board_with_members(1, &[1, 2]);
        let mut form = BoardForm::edit(&board, 1);
        assert_eq!(form.values().unwrap_err(), "nothing changed");
        form.is_locked.set_checked(true);
        form.finished.set_value(board.paused_col.0);
        match form.values().unwrap() {
            BoardOutcome::Update(req) => {
                assert_eq!(req.is_locked, Some(true));
                assert_eq!(req.name, None);
                assert_eq!(req.card_fields, None);
                assert_eq!(req.roles, vec![(ColumnRole::Finished, board.paused_col)]);
            }
            other => panic!("{other:?}"),
        }
    }
}
