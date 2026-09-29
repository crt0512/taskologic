//! Central state and the update function.
//!
//! Elm style: every input is a `Msg`, `update` mutates the state and hands
//! back `Cmd`s for the outside world. Nothing in here does IO, which is what
//! makes the TestBackend tests possible. The client never enforces a rule
//! itself, it hides what it knows the daemon would refuse and otherwise
//! shows the daemon's answer.

use std::collections::HashMap;

use crossterm::event::{
    Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use taskologic_core::barcode::{Expired, Feed, ScanAction, ScanDetector, ScanPayload};
use taskologic_core::control::{self, Control, Value};

use crate::control::{Armed, ControlState, Waiting, named_key_event, values_date, values_minutes, values_text};
use taskologic_core::board::{Board, ColumnRole};
use taskologic_core::ids::{BoardId, ColumnId, PrintJobId, TaskId, Uid};
use taskologic_core::prefs::{CardFields, CustomColors, ThemePreset};
use taskologic_core::print::PrintJob;
use taskologic_core::program::Question;
use taskologic_core::task::Task;
use taskologic_core::user::User;
use taskologic_print::DeviceProfile;
use taskologic_proto::{BoardChange, BoardDetail, BoardSummary, ClientMessage, ErrorBody, ErrorCode, Event, Hello, Request, RequestId, Response, ScanOutcome, SearchHit, ServerMessage, Severity, TaskChange, Found, LookupKind};

use crate::forms::analytics::{AnalyticsOutcome, AnalyticsPanel};
use crate::forms::archive::{ArchiveOutcome, ArchivePanel};
use crate::forms::board::{BoardForm, BoardOutcome};
use crate::forms::codes::{CodesOutcome, CodesPanel};
use crate::forms::colors::{ColorsForm, ColorsOutcome};
use crate::forms::columns::{ColumnsOutcome, ColumnsPanel};
use crate::forms::confirm::{Confirm, ConfirmOutcome};
use crate::forms::members::{MembersOutcome, MembersPanel};
use crate::forms::print_rules::{PrintRulesForm, PrintRulesOutcome};
use crate::forms::printer::{PrinterForm, PrinterOutcome};
use crate::forms::program::{ProgramForm, ProgramOutcome};
use crate::forms::programs::{ProgramsOutcome, ProgramsPanel};
use crate::forms::repeats::{RepeatsOutcome, RepeatsPanel};
use crate::forms::runs::{RunsOutcome, RunsPanel};
use crate::forms::settings::{SettingsForm, SettingsOutcome};
use crate::forms::start_program::{StartOutcome, StartProgramForm};
use crate::forms::step::{StepForm, StepOutcome};
use crate::forms::task::{FormMode, FormOutcome, TaskForm, TaskSave};
use crate::forms::templates::{TemplatesOutcome, TemplatesPanel};
use crate::forms::users::{UsersOutcome, UsersPanel};
use crate::scan;
use crate::ui::adapter::{
    CheckboxState, HandleEvent, HasFocus, MouseFlags, Regular, TextInputState,
};
use crate::ui::theme::{ColorMode, Theme};

pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;
const TOAST_MS: u64 = 4000;
const FLASH_MS: u64 = 300;

pub enum Msg {
    Term(TermEvent),
    /// Boxed so the whole enum stays small in the channel.
    Server(Box<ServerMessage>),
    Disconnected(String),
    Tick(u64),
    Printed {
        job_id: PrintJobId,
        error: Option<String>,
    },
    TestPrinted {
        error: Option<String>,
    },
    /// The runtime wrote (or failed to write) the printer profile.
    PrinterSaved {
        path: String,
        error: Option<String>,
    },
    /// What `lpstat -e` reported: CUPS queue names on this machine.
    PrintersDetected {
        queues: Vec<String>,
        error: Option<String>,
    },
    /// How wide the media on `queue` is, per `lpoptions`. None when CUPS
    /// names a size with no dimensions in it, or the queue has gone away.
    MediaDetected {
        queue: String,
        mm: Option<u16>,
    },
}

#[derive(Debug, PartialEq)]
pub enum Cmd {
    Send(ClientMessage),
    Print {
        job_id: PrintJobId,
        job: PrintJob,
    },
    /// Render and spool a job locally without involving the daemon, with
    /// the profile as the printer form has it right now, saved or not.
    TestPrint {
        job: PrintJob,
        profile: DeviceProfile,
    },
    /// Write this printer profile to the client config file. None removes
    /// the printer section.
    SavePrinter(Option<DeviceProfile>),
    /// List the CUPS queues on this machine, off the UI thread.
    DetectPrinters,
    /// Ask CUPS how wide this queue's media is, to seed the paper width.
    DetectMedia(String),
    Bell,
    Quit,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Conn {
    Connecting,
    Ready,
    Lost,
}

pub struct Toast {
    pub text: String,
    pub severity: Severity,
    pub until_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    Welcome,
    Boards,
    Users,
    OpenBoard { focus_task: Option<TaskId> },
    RefreshBoard,
    MoveTask {
        task: TaskId,
        to: ColumnId,
        /// The answer that went with a finishing move, kept so an override
        /// after a dependency refusal still carries it.
        answer: Option<String>,
    },
    AnswerQuestion,
    DeleteTask,
    PurgeTask,
    DeleteBoard,
    Restore,
    Archived,
    Scan,
    /// A task scan while a control code is armed: the task and its board
    /// come back and the code acts on them.
    Resolve,
    /// A change made by a control code; the task comes back changed.
    ControlOp,
    /// A targeted control code: what it names comes back, then it acts.
    Lookup(Box<Control>),
    Search,
    Print,
    Ack,
    FormMembers,
    FormDepSearch,
    SaveTask,
    Settings,
    Timezone,
    FormUsers,
    CreateBoardForm,
    BoardOp,
    PanelMembers,
    PanelUsers,
    MemberOp { uid: Uid },
    ColumnOp,
    UsersPanel,
    AdminOp,
    ChecklistOp,
    TemplatesPanel,
    TemplateOp,
    RepeatsPanel,
    StopRepeat,
    Analytics,
    TaskHistory { task: TaskId },
    ExcludeFromStats,
    ProgramsPanel,
    ProgramOp,
    StartProgram,
    RunsPanel,
    CancelRun,
    StepMembers,
}

pub enum Overlay {
    Help,
    /// Every yes/no question, answerable by key, mouse or touch.
    Confirm {
        dialog: Box<Confirm>,
        action: ConfirmAction,
        back: Option<Box<Overlay>>,
    },
    TaskForm(Box<TaskForm>),
    /// Print rules, opened from the task form or a program step and
    /// returning to whichever it came from.
    PrintRules {
        form: Box<PrintRulesForm>,
        back: PrintRulesBack,
    },
    Programs(Box<ProgramsPanel>),
    ProgramForm(Box<ProgramForm>),
    /// One step, opened from the program editor and returning to it.
    StepForm {
        form: Box<StepForm>,
        back: Box<ProgramForm>,
    },
    StartProgram(Box<StartProgramForm>),
    Runs(Box<RunsPanel>),
    Settings(Box<SettingsForm>),
    /// The colour editor, opened from settings and returning to it.
    Colors {
        form: Box<ColorsForm>,
        back: Box<SettingsForm>,
    },
    /// The printer setup, opened from settings and returning to it.
    Printer {
        form: Box<PrinterForm>,
        back: Box<SettingsForm>,
    },
    /// The Print codes panel, opened from settings and returning to it.
    Codes {
        panel: Box<CodesPanel>,
        back: Box<SettingsForm>,
    },
    /// A "move to" scanned without a column asks which, for this task.
    ColumnPick {
        task: Box<Task>,
        options: Vec<(ColumnId, String)>,
        sel: usize,
        /// When it opened, for the control timeout.
        since_ms: u64,
    },
    BoardForm(Box<BoardForm>),
    Members(Box<MembersPanel>),
    Columns(Box<ColumnsPanel>),
    /// One question of the column removal flow.
    PickColumn {
        panel: Box<ColumnsPanel>,
        title: String,
        options: Vec<(ColumnId, String)>,
        sel: usize,
        flow: RemoveFlow,
    },
    Users(Box<UsersPanel>),
    Templates(Box<TemplatesPanel>),
    Repeats(Box<RepeatsPanel>),
    /// The More menu: everything used now and then rather than constantly.
    Menu {
        items: Vec<(String, char)>,
        sel: Option<usize>,
        areas: Vec<Rect>,
    },
    /// A save hit a stale version. The form is kept so nothing typed is lost.
    Conflict {
        form: Box<TaskForm>,
        current: Box<Task>,
    },
    /// The read only task view. The checklist in it is live: items are
    /// selectable and can be ticked. The rects are recorded by the renderer.
    TaskDetail {
        task: Task,
        sel: usize,
        item_areas: Vec<Rect>,
        area: Rect,
    },
    Archive(Box<ArchivePanel>),
    /// A step's question, asked before a finishing move goes out, or for a
    /// finished task still waiting on one. The rects are recorded by the
    /// renderer for clicks.
    Question {
        task: TaskId,
        title: String,
        question: Question,
        sel: usize,
        then: QuestionThen,
        areas: Vec<Rect>,
        area: Rect,
    },
}

/// What happens once a question is answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestionThen {
    /// Send the finishing move with the answer.
    Move {
        to_column: ColumnId,
        position: Option<i64>,
        override_deps: bool,
    },
    /// The task is finished already: just answer.
    Answer,
}

/// Where the print rules window goes back to.
pub enum PrintRulesBack {
    Task(Box<TaskForm>),
    Step {
        form: Box<StepForm>,
        program: Box<ProgramForm>,
    },
}

/// What a confirmed dialog does.
pub enum ConfirmAction {
    Quit,
    Send { request: Request, pending: Pending },
}

/// Answers collected before a column can be removed.
#[derive(Clone, Debug)]
pub struct RemoveFlow {
    pub column: ColumnId,
    pub has_tasks: bool,
    pub destination: Option<ColumnId>,
    pub roles: Vec<ColumnRole>,
    pub replacements: Vec<(ColumnRole, ColumnId)>,
}

impl RemoveFlow {
    fn next_role(&self) -> Option<ColumnRole> {
        self.roles
            .iter()
            .find(|r| !self.replacements.iter().any(|(rr, _)| rr == *r))
            .copied()
    }
}

/// The open board plus everything the view records for mouse hit testing.
pub struct BoardView {
    pub detail: BoardDetail,
    pub col: usize,
    pub row: usize,
    pub picked: Option<TaskId>,
    pub col_offset: usize,
    pub area: Rect,
    pub column_areas: Vec<Rect>,
    /// Per column click targets, as (column index, area).
    pub plus_areas: Vec<(usize, Rect)>,
    pub sort_areas: Vec<(usize, Rect)>,
    pub up_areas: Vec<(usize, Rect)>,
    pub down_areas: Vec<(usize, Rect)>,
    pub left_arrow: Option<Rect>,
    pub right_arrow: Option<Rect>,
    /// First visible card per column.
    pub col_scroll: Vec<usize>,
    pub task_areas: Vec<Vec<(TaskId, Rect)>>,
    pub mouse: MouseFlags,
    pub drag: Option<(TaskId, usize)>,
    pub hover_col: Option<usize>,
}

impl BoardView {
    fn new(detail: BoardDetail) -> Self {
        Self {
            detail,
            col: 0,
            row: 0,
            picked: None,
            col_offset: 0,
            area: Rect::default(),
            column_areas: Vec::new(),
            plus_areas: Vec::new(),
            sort_areas: Vec::new(),
            up_areas: Vec::new(),
            down_areas: Vec::new(),
            left_arrow: None,
            right_arrow: None,
            col_scroll: Vec::new(),
            task_areas: Vec::new(),
            mouse: MouseFlags::default(),
            drag: None,
            hover_col: None,
        }
    }

    pub fn column_count(&self) -> usize {
        self.detail.board.columns.len()
    }

    pub fn column_id(&self, idx: usize) -> Option<ColumnId> {
        self.detail.board.columns.get(idx).map(|c| c.id)
    }

    /// Tasks of a column in display order.
    pub fn tasks_in(&self, idx: usize) -> Vec<&Task> {
        let Some(column) = self.detail.board.columns.get(idx) else {
            return Vec::new();
        };
        let mut v: Vec<&Task> = self
            .detail
            .tasks
            .iter()
            .filter(|t| t.column_id == column.id && !t.is_archived())
            .collect();
        if column.sort_by_due {
            // Tasks without a due date sink to the bottom.
            v.sort_by_key(|t| (t.due_at.is_none(), t.due_at, t.position, t.id));
        } else {
            v.sort_by_key(|t| (t.position, t.id));
        }
        v
    }

    /// Where a card dropped at `y` lands in column `idx`, and the position
    /// value that puts it there.
    fn drop_position(&self, idx: usize, y: u16, moving: TaskId) -> Option<i64> {
        let others: Vec<i64> = self
            .tasks_in(idx)
            .into_iter()
            .filter(|t| t.id != moving)
            .map(|t| t.position)
            .collect();
        if others.is_empty() {
            return Some(0);
        }
        let slot = self.task_areas.get(idx.checked_sub(self.col_offset)?)?;
        let mut index = slot
            .iter()
            .filter(|(id, area)| *id != moving && y >= area.y + area.height / 2)
            .count()
            + self.col_scroll.get(idx).copied().unwrap_or(0);
        index = index.min(others.len());
        Some(match index {
            0 => others[0] - 10,
            i if i >= others.len() => others[others.len() - 1] + 10,
            i => {
                let (before, after) = (others[i - 1], others[i]);
                if after - before > 1 {
                    before + (after - before) / 2
                } else {
                    before + 1
                }
            }
        })
    }

    pub fn selected_task(&self) -> Option<&Task> {
        self.tasks_in(self.col).get(self.row).copied()
    }

    fn clamp(&mut self) {
        let cols = self.column_count();
        if cols == 0 {
            self.col = 0;
            self.row = 0;
            return;
        }
        self.col = self.col.min(cols - 1);
        let n = self.tasks_in(self.col).len();
        self.row = if n == 0 { 0 } else { self.row.min(n - 1) };
    }

    fn select_task(&mut self, id: TaskId) {
        for c in 0..self.column_count() {
            if let Some(r) = self.tasks_in(c).iter().position(|t| t.id == id) {
                self.col = c;
                self.row = r;
                return;
            }
        }
    }

    fn upsert(&mut self, task: Task) {
        match self.detail.tasks.iter_mut().find(|t| t.id == task.id) {
            Some(slot) => *slot = task,
            None => self.detail.tasks.push(task),
        }
        self.clamp();
    }

    fn remove(&mut self, id: TaskId) {
        self.detail.tasks.retain(|t| t.id != id);
        if self.picked == Some(id) {
            self.picked = None;
        }
        self.clamp();
    }

    fn column_at(&self, x: u16, y: u16) -> Option<usize> {
        self.column_areas
            .iter()
            .position(|a| a.contains(Position::new(x, y)))
            .map(|i| i + self.col_offset)
    }

    fn hit(targets: &[(usize, Rect)], x: u16, y: u16) -> Option<usize> {
        targets
            .iter()
            .find(|(_, a)| a.contains(Position::new(x, y)))
            .map(|(i, _)| *i)
    }

    fn task_at(&self, x: u16, y: u16) -> Option<(usize, usize, TaskId)> {
        for (i, col) in self.task_areas.iter().enumerate() {
            let idx = i + self.col_offset;
            // task_areas only holds the cards that are actually on screen, and
            // those start at the column's scroll offset. Without adding it back
            // a click in a scrolled column selects the card that many places
            // above the one under the pointer.
            let scrolled = self.col_scroll.get(idx).copied().unwrap_or(0);
            for (row, (id, area)) in col.iter().enumerate() {
                if area.contains(Position::new(x, y)) {
                    return Some((idx, row + scrolled, *id));
                }
            }
        }
        None
    }
}

pub struct App {
    pub size: (u16, u16),
    pub ascii: bool,
    pub conn: Conn,
    pub conn_error: Option<String>,
    pub user: Option<User>,
    pub users: HashMap<Uid, String>,
    pub boards: Vec<BoardSummary>,
    /// The analytics screen, when it is the one showing. It sits beside the
    /// dashboard and the board rather than over them: it is a place you go,
    /// not a dialog you answer.
    pub analytics: Option<Box<AnalyticsPanel>>,
    pub board_sel: usize,
    pub board_areas: Vec<Rect>,
    pub dash_scroll: usize,
    pub tab_areas: Vec<(Rect, taskologic_core::ids::BoardId)>,
    pub search: TextInputState,
    pub search_area: Rect,
    pub include_archived: CheckboxState,
    pub hits: Vec<SearchHit>,
    pub hit_sel: usize,
    pub hit_areas: Vec<Rect>,
    pub board: Option<BoardView>,
    pub overlay: Option<Overlay>,
    pub toast: Option<Toast>,
    pub menu_areas: Vec<(Rect, char)>,
    pub printer: Option<DeviceProfile>,
    /// What the last local print was, for its toast: a test print or a
    /// codes card.
    local_print: &'static str,
    pub color_mode: ColorMode,
    pub manual_scan: bool,
    /// What control codes are waiting for between scans.
    pub control: ControlState,
    /// Screen inverts until this instant, the visual half of the scan bell.
    pub flash_until_ms: u64,
    pub mouse_seen: bool,
    /// Last pointer position, so the theme can light up what is under it.
    pub mouse_pos: Option<(u16, u16)>,
    pub now_ms: u64,
    pending: HashMap<RequestId, Pending>,
    next_id: RequestId,
    scan: ScanDetector,
}

impl App {
    pub fn new(ascii: bool, color_mode: ColorMode, printer: Option<DeviceProfile>) -> App {
        App {
            size: (MIN_WIDTH, MIN_HEIGHT),
            ascii,
            conn: Conn::Connecting,
            conn_error: None,
            user: None,
            users: HashMap::new(),
            boards: Vec::new(),
            analytics: None,
            board_sel: 0,
            board_areas: Vec::new(),
            dash_scroll: 0,
            tab_areas: Vec::new(),
            search: TextInputState::named("search"),
            search_area: Rect::default(),
            include_archived: CheckboxState::named("include_archived"),
            hits: Vec::new(),
            hit_sel: 0,
            hit_areas: Vec::new(),
            board: None,
            overlay: None,
            toast: None,
            menu_areas: Vec::new(),
            printer,
            color_mode,
            manual_scan: false,
            flash_until_ms: 0,
            mouse_seen: false,
            mouse_pos: None,
            now_ms: 0,
            pending: HashMap::new(),
            next_id: 1,
            scan: ScanDetector::new(),
            control: ControlState::default(),
            local_print: "test print",
        }
    }

    pub fn start(&mut self, hello: Hello) -> Vec<Cmd> {
        vec![self.send(Request::Hello(hello), Pending::Welcome)]
    }

    /// Colours and glyphs, rebuilt per frame so a prefs change shows at once.
    /// While the settings form is open it previews what is picked there.
    pub fn theme(&self) -> Theme {
        let (preset, colors) = match &self.overlay {
            Some(Overlay::Settings(form)) => form.preview(),
            Some(Overlay::Colors { back, .. }) => back.preview(),
            Some(Overlay::Printer { back, .. }) => back.preview(),
            Some(Overlay::Codes { back, .. }) => back.preview(),
            _ => match &self.user {
                Some(u) => (u.prefs.ui.theme, u.prefs.ui.custom_colors.clone()),
                None => (ThemePreset::default(), CustomColors::default()),
            },
        };
        Theme::preset(preset, &colors, self.color_mode, self.ascii)
            .with_touch(self.touchscreen())
            .with_mouse(self.mouse_pos)
    }

    pub fn touchscreen(&self) -> bool {
        self.user.as_ref().is_some_and(|u| u.prefs.ui.touchscreen)
    }

    pub fn show_tabs(&self) -> bool {
        // Board tabs would offer to switch to a board while you are reading
        // numbers across all of them, which is not what the click would do.
        self.analytics.is_none()
            && self.board.is_some()
            && self
                .user
                .as_ref()
                .is_some_and(|u| u.prefs.ui.show_board_tabs)
    }

    /// What task cards show: the user's own choice, else the board's.
    pub fn card_fields(&self) -> CardFields {
        match self.user.as_ref().and_then(|u| u.prefs.ui.card_fields) {
            Some(f) => f,
            None => self
                .board
                .as_ref()
                .map(|b| b.detail.board.card_fields)
                .unwrap_or_default(),
        }
    }

    pub fn too_small(&self) -> bool {
        self.size.0 < MIN_WIDTH || self.size.1 < MIN_HEIGHT
    }

    pub fn scanner_enabled(&self) -> bool {
        self.user
            .as_ref()
            .is_some_and(|u| u.prefs.ui.scanner_enabled)
    }

    pub fn scanner_manual_only(&self) -> bool {
        self.user
            .as_ref()
            .is_some_and(|u| u.prefs.scanner.manual_only)
    }

    /// The full width scan button shows whenever manual mode could be toggled.
    pub fn show_scan_button(&self) -> bool {
        self.scanner_enabled() && self.scanner_manual_only()
    }

    /// The bell plus a short screen flash, because some terminals swallow BEL.
    fn bell(&mut self) -> Cmd {
        self.flash_until_ms = self.now_ms + FLASH_MS;
        Cmd::Bell
    }

    pub fn flash_active(&self) -> bool {
        self.now_ms < self.flash_until_ms
    }

    /// True while the scan detector is watching the keystroke stream. Never
    /// while a text field has focus, never under an overlay.
    pub fn scanner_listening(&self) -> bool {
        self.scanner_enabled()
            && (!self.scanner_manual_only() || self.manual_scan)
            && self.overlay.is_none()
            && !self.text_focused()
    }

    pub fn text_focused(&self) -> bool {
        self.search.is_focused()
            || self.is_widget_overlay()
            || matches!(&self.overlay, Some(Overlay::Conflict { .. }))
    }

    /// Overlays that own widgets and get raw events.
    fn is_widget_overlay(&self) -> bool {
        matches!(
            self.overlay,
            Some(
                Overlay::Confirm { .. }
                    | Overlay::Menu { .. }
                    | Overlay::Archive(_)
                    | Overlay::TaskForm(_)
                    | Overlay::PrintRules { .. }
                    | Overlay::Programs(_)
                    | Overlay::ProgramForm(_)
                    | Overlay::StepForm { .. }
                    | Overlay::StartProgram(_)
                    | Overlay::Runs(_)
                    | Overlay::Settings(_)
                    | Overlay::Colors { .. }
                    | Overlay::Printer { .. }
                    | Overlay::Codes { .. }
                    | Overlay::BoardForm(_)
                    | Overlay::Members(_)
                    | Overlay::Columns(_)
                    | Overlay::Users(_)
                    | Overlay::Templates(_)
                    | Overlay::Repeats(_)
            )
        )
    }

    /// The user's zone, for anything that renders a timestamp.
    fn timezone(&self) -> chrono_tz::Tz {
        self.user
            .as_ref()
            .map(|u| u.timezone)
            .unwrap_or(chrono_tz::UTC)
    }

    fn board_name(&self, id: BoardId) -> Option<String> {
        self.boards
            .iter()
            .find(|b| b.id == id)
            .map(|b| b.name.clone())
    }

    fn user_name(&self, uid: Uid) -> String {
        self.users
            .get(&uid)
            .cloned()
            .unwrap_or_else(|| format!("uid {uid}"))
    }

    fn me(&self) -> Uid {
        self.user.as_ref().map(|u| u.uid).unwrap_or(0)
    }

    /// Owner of the open board, or an admin. What the archive purge and the
    /// board delete are gated on; the daemon decides for real.
    pub fn privileged_on_open_board(&self) -> bool {
        match (&self.user, &self.board) {
            (Some(u), Some(b)) => u.is_admin || b.detail.board.owner_uid == u.uid,
            _ => false,
        }
    }

    pub fn show_print(&self) -> bool {
        self.printer.is_some()
            && self
                .user
                .as_ref()
                .is_some_and(|u| u.prefs.print.show_print_button)
    }

    fn send(&mut self, request: Request, pending: Pending) -> Cmd {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, pending);
        Cmd::Send(ClientMessage { id, request })
    }

    pub fn toast(&mut self, severity: Severity, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            severity,
            until_ms: self.now_ms + TOAST_MS,
        });
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Cmd> {
        match msg {
            Msg::Tick(now) => {
                self.now_ms = now;
                if self.toast.as_ref().is_some_and(|t| t.until_ms <= now) {
                    self.toast = None;
                }
                let mut out = Vec::new();
                match self.scan.expire(now) {
                    Expired::Nothing | Expired::Scan => {}
                    Expired::Keys(keys) => {
                        // Typing that looked like the start of a code and was not.
                        for key in keys {
                            out.extend(self.route_key(scan::key_event(key)));
                        }
                    }
                    Expired::Frame => {
                        self.toast(Severity::Warning, "control code timed out before it was closed");
                    }
                }
                let timeout = self.control_timeout_ms();
                if timeout > 0
                    && let Some(Overlay::ColumnPick { since_ms, .. }) = &self.overlay
                    && now.saturating_sub(*since_ms) > timeout
                {
                    self.overlay = None;
                    self.toast(Severity::Info, "no column picked in time, move cancelled");
                }
                if self.control.stale(now, self.control_timeout_ms()) {
                    let w = self.control.clear().unwrap_or_else(|| "it".into());
                    self.toast(Severity::Info, format!("nothing came for {w}, cancelled"));
                }
                out
            }
            Msg::Disconnected(reason) => {
                self.conn = Conn::Lost;
                self.conn_error = Some(reason);
                Vec::new()
            }
            Msg::Server(msg) => match *msg {
                ServerMessage::Ok { id, response } => {
                    let pending = self.pending.remove(&id);
                    self.on_response(pending, response)
                }
                ServerMessage::Err { id, error } => {
                    let pending = self.pending.remove(&id);
                    self.on_error(pending, error)
                }
                ServerMessage::Event { event } => self.on_event(event),
            },
            Msg::TestPrinted { error } => {
                let what = self.local_print;
                match error {
                    Some(e) => self.toast(Severity::Error, format!("{what} failed: {e}")),
                    None => self.toast(Severity::Success, format!("{what} sent to the printer")),
                }
                Vec::new()
            }
            Msg::PrinterSaved { path, error } => {
                match error {
                    Some(e) => self.toast(Severity::Error, format!("could not write {path}: {e}")),
                    None => self.toast(
                        Severity::Success,
                        format!("printer profile written to {path}"),
                    ),
                }
                Vec::new()
            }
            Msg::MediaDetected { queue, mm } => {
                if let Some(Overlay::Printer { form, .. }) = &mut self.overlay {
                    form.set_media(&queue, mm);
                }
                Vec::new()
            }
            Msg::PrintersDetected { queues, error } => {
                if let Some(Overlay::Printer { form, .. }) = &mut self.overlay {
                    form.set_detected(queues, error);
                }
                Vec::new()
            }
            Msg::Printed { job_id, error } => {
                if let Some(e) = &error {
                    self.toast(Severity::Error, format!("print failed: {e}"));
                }
                vec![self.send(Request::AckPrintJob { job_id, error }, Pending::Ack)]
            }
            Msg::Term(ev) => self.on_term(ev),
        }
    }

    fn on_response(&mut self, pending: Option<Pending>, response: Response) -> Vec<Cmd> {
        match (pending, response) {
            (Some(Pending::Welcome), Response::Welcome(w)) => {
                self.conn = Conn::Ready;
                self.scan = scan::detector_for(&w.user.prefs.scanner);
                self.users.insert(w.user.uid, w.user.username.clone());
                self.user = Some(w.user);
                self.boards = w.boards;
                vec![self.send(Request::ListUsers, Pending::Users)]
            }
            (Some(Pending::Users), Response::Users { users }) => {
                for u in users {
                    self.users.insert(u.uid, u.username);
                }
                Vec::new()
            }
            (Some(Pending::Boards), Response::Boards { boards }) => {
                self.boards = boards;
                self.board_sel = self.board_sel.min(self.boards.len().saturating_sub(1));
                if let Some(b) = &self.board
                    && !self.boards.iter().any(|s| s.id == b.detail.board.id)
                {
                    self.board = None;
                    self.toast(Severity::Warning, "that board is gone");
                }
                Vec::new()
            }
            (Some(Pending::OpenBoard { focus_task }), Response::Board(detail)) => {
                let mut view = BoardView::new(detail);
                if let Some(t) = focus_task {
                    view.select_task(t);
                }
                // A question nobody answered, finished by a plain scan, is
                // asked the moment the board is open again.
                let waiting = view
                    .detail
                    .tasks
                    .iter()
                    .find(|t| t.program.as_ref().is_some_and(|p| p.pending_question))
                    .cloned();
                self.board = Some(view);
                self.hits.clear();
                if let Some(task) = waiting
                    && self.overlay.is_none()
                {
                    self.ask_question(&task, QuestionThen::Answer);
                }
                Vec::new()
            }
            (Some(Pending::RefreshBoard), Response::Board(detail)) => {
                self.refresh_panels(&detail.board);
                if let Some(b) = &mut self.board
                    && b.detail.board.id == detail.board.id
                {
                    b.detail = detail;
                    b.clamp();
                }
                Vec::new()
            }
            (Some(Pending::Restore), Response::Task { task }) => {
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    b.upsert(task);
                }
                Vec::new()
            }
            (Some(Pending::MoveTask { task: moved, .. }), Response::Task { task }) => {
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    b.upsert(task);
                    // Keep the cursor on the picked up card, or moving it
                    // twice in a row works on the wrong task.
                    if b.picked == Some(moved) {
                        b.select_task(moved);
                    }
                }
                Vec::new()
            }
            (Some(Pending::AnswerQuestion), Response::Task { task }) => {
                self.toast(Severity::Success, format!("answered for {}", task.title));
                if let Some(Overlay::TaskDetail { task: shown, .. }) = &mut self.overlay
                    && shown.id == task.id
                {
                    *shown = task.clone();
                }
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    b.upsert(task);
                }
                Vec::new()
            }
            (Some(Pending::ChecklistOp), Response::Task { task }) => {
                if let Some(Overlay::TaskDetail {
                    task: shown, sel, ..
                }) = &mut self.overlay
                    && shown.id == task.id
                {
                    *shown = task.clone();
                    *sel = (*sel).min(shown.checklist.len().saturating_sub(1));
                }
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    b.upsert(task);
                }
                Vec::new()
            }
            (Some(Pending::FormMembers), Response::Members { members }) => {
                if let Some(Overlay::TaskForm(form)) = &mut self.overlay {
                    form.set_members(&members);
                }
                Vec::new()
            }
            (Some(Pending::FormDepSearch), Response::SearchResults { hits }) => {
                if let Some(Overlay::TaskForm(form)) = &mut self.overlay {
                    form.set_dep_hits(hits);
                }
                Vec::new()
            }
            (Some(Pending::SaveTask), Response::Task { task }) => {
                if matches!(self.overlay, Some(Overlay::TaskForm(_))) {
                    self.overlay = None;
                }
                self.toast(Severity::Success, format!("saved {}", task.title));
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    b.upsert(task);
                }
                Vec::new()
            }
            (Some(Pending::Settings), Response::Done) => {
                if matches!(self.overlay, Some(Overlay::Settings(_))) {
                    self.overlay = None;
                }
                self.toast(Severity::Success, "settings saved");
                Vec::new()
            }
            (Some(Pending::Timezone), Response::Done) => Vec::new(),
            (Some(Pending::FormUsers), Response::Users { users }) => {
                if let Some(Overlay::BoardForm(form)) = &mut self.overlay {
                    form.set_users(&users);
                }
                Vec::new()
            }
            (Some(Pending::CreateBoardForm), Response::Board(detail)) => {
                self.overlay = None;
                self.toast(
                    Severity::Success,
                    format!("created board {}", detail.board.name),
                );
                self.board = Some(BoardView::new(detail));
                vec![self.send(Request::ListBoards, Pending::Boards)]
            }
            (Some(Pending::BoardOp), Response::Board(detail)) => {
                if matches!(self.overlay, Some(Overlay::BoardForm(_))) {
                    self.overlay = None;
                }
                self.toast(Severity::Success, "board settings saved");
                if let Some(b) = &mut self.board
                    && b.detail.board.id == detail.board.id
                {
                    b.detail = detail;
                    b.clamp();
                }
                vec![self.send(Request::ListBoards, Pending::Boards)]
            }
            (Some(Pending::PanelMembers), Response::Members { members }) => {
                if let Some(Overlay::Members(panel)) = &mut self.overlay {
                    panel.set_members(members);
                }
                Vec::new()
            }
            (Some(Pending::PanelUsers), Response::Users { users }) => {
                if let Some(Overlay::Members(panel)) = &mut self.overlay {
                    panel.set_users(users);
                }
                Vec::new()
            }
            (Some(Pending::MemberOp { .. }), Response::Done) => match &self.overlay {
                Some(Overlay::Members(panel)) => {
                    let board_id = panel.board_id;
                    vec![self.send(Request::ListMembers { board_id }, Pending::PanelMembers)]
                }
                _ => Vec::new(),
            },
            (Some(Pending::ColumnOp), Response::Board(detail)) => {
                let board = detail.board.clone();
                if let Some(b) = &mut self.board
                    && b.detail.board.id == detail.board.id
                {
                    b.detail = detail;
                    b.clamp();
                }
                // Only refresh an open panel. Sort toggles come through here
                // too, and those must not pop the panel open.
                if let Some(Overlay::Columns(panel)) = &mut self.overlay {
                    panel.refresh(&board);
                }
                Vec::new()
            }
            (Some(Pending::UsersPanel), Response::Users { users }) => {
                if let Some(Overlay::Users(panel)) = &mut self.overlay {
                    panel.set_users(users);
                }
                Vec::new()
            }
            (Some(Pending::AdminOp), Response::Done) => {
                if matches!(self.overlay, Some(Overlay::Users(_))) {
                    vec![self.send(Request::ListUsers, Pending::UsersPanel)]
                } else {
                    Vec::new()
                }
            }
            (Some(Pending::Archived), Response::Tasks { tasks }) => {
                let tz = self
                    .user
                    .as_ref()
                    .map(|u| u.timezone)
                    .unwrap_or(chrono_tz::UTC);
                let privileged = self.privileged_on_open_board();
                self.overlay = Some(Overlay::Archive(Box::new(ArchivePanel::new(
                    tasks, privileged, tz,
                ))));
                Vec::new()
            }
            (Some(Pending::TemplatesPanel), Response::Templates { templates }) => {
                let Some(board_id) = self.board.as_ref().map(|b| b.detail.board.id) else {
                    return Vec::new();
                };
                let names: HashMap<Uid, String> = self.users.clone();
                let name_of = move |uid: Uid| {
                    names
                        .get(&uid)
                        .cloned()
                        .unwrap_or_else(|| format!("uid {uid}"))
                };
                match &mut self.overlay {
                    Some(Overlay::Templates(panel)) if panel.board_id == board_id => {
                        panel.set_templates(templates, &name_of);
                    }
                    _ => {
                        let me = self.me();
                        let privileged = self.privileged_on_open_board();
                        let mut panel = TemplatesPanel::new(board_id, me, privileged);
                        panel.set_templates(templates, &name_of);
                        self.overlay = Some(Overlay::Templates(Box::new(panel)));
                    }
                }
                Vec::new()
            }
            (Some(Pending::TemplateOp), Response::Template { template }) => {
                if matches!(self.overlay, Some(Overlay::TaskForm(_))) {
                    self.overlay = None;
                }
                self.toast(
                    Severity::Success,
                    format!("saved template {}", template.name),
                );
                let board_id = template.board_id;
                vec![self.send(Request::ListTemplates { board_id }, Pending::TemplatesPanel)]
            }
            (Some(Pending::TemplateOp), Response::Done) => {
                self.toast(Severity::Info, "template deleted");
                match self.board.as_ref().map(|b| b.detail.board.id) {
                    Some(board_id) => vec![
                        self.send(Request::ListTemplates { board_id }, Pending::TemplatesPanel),
                    ],
                    None => Vec::new(),
                }
            }
            (Some(Pending::ProgramsPanel), Response::Programs { programs }) => {
                let Some(board_id) = self.board.as_ref().map(|b| b.detail.board.id) else {
                    return Vec::new();
                };
                let names: HashMap<Uid, String> = self.users.clone();
                let name_of = move |uid: Uid| {
                    names
                        .get(&uid)
                        .cloned()
                        .unwrap_or_else(|| format!("uid {uid}"))
                };
                match &mut self.overlay {
                    Some(Overlay::Programs(panel)) if panel.board_id == board_id => {
                        panel.set_programs(programs, &name_of);
                    }
                    _ => {
                        let me = self.me();
                        let privileged = self.privileged_on_open_board();
                        let mut panel = ProgramsPanel::new(board_id, me, privileged);
                        panel.set_programs(programs, &name_of);
                        self.overlay = Some(Overlay::Programs(Box::new(panel)));
                    }
                }
                Vec::new()
            }
            (Some(Pending::ProgramOp), Response::Program { program }) => {
                if matches!(self.overlay, Some(Overlay::ProgramForm(_))) {
                    self.overlay = None;
                }
                self.toast(Severity::Success, format!("saved program {}", program.name));
                let board_id = program.board_id;
                vec![self.send(Request::ListPrograms { board_id }, Pending::ProgramsPanel)]
            }
            (Some(Pending::ProgramOp), Response::Done) => {
                self.toast(Severity::Info, "program deleted, runs already going carry on");
                match self.board.as_ref().map(|b| b.detail.board.id) {
                    Some(board_id) => {
                        vec![self.send(Request::ListPrograms { board_id }, Pending::ProgramsPanel)]
                    }
                    None => Vec::new(),
                }
            }
            (Some(Pending::StartProgram), Response::Task { task }) => {
                if matches!(self.overlay, Some(Overlay::StartProgram(_))) {
                    self.overlay = None;
                }
                self.toast(
                    Severity::Success,
                    format!("started {}, move it to doing when you begin", task.title),
                );
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    b.upsert(task);
                }
                Vec::new()
            }
            (Some(Pending::RunsPanel), Response::Runs { runs }) => {
                let Some(board_id) = self.board.as_ref().map(|b| b.detail.board.id) else {
                    return Vec::new();
                };
                match &mut self.overlay {
                    Some(Overlay::Runs(panel)) if panel.board_id == board_id => {
                        panel.set_entries(runs)
                    }
                    _ => {
                        let mut panel = RunsPanel::new(board_id, self.timezone(), self.users.clone());
                        panel.set_entries(runs);
                        self.overlay = Some(Overlay::Runs(Box::new(panel)));
                    }
                }
                Vec::new()
            }
            (Some(Pending::CancelRun), Response::Done) => {
                self.toast(Severity::Info, "run cancelled");
                match self.board.as_ref().map(|b| b.detail.board.id) {
                    Some(board_id) => vec![self.send(Request::ListRuns { board_id }, Pending::RunsPanel)],
                    None => Vec::new(),
                }
            }
            (Some(Pending::StepMembers), Response::Members { members }) => {
                if let Some(Overlay::StepForm { form, .. }) = &mut self.overlay {
                    form.set_members(&members);
                }
                Vec::new()
            }
            (Some(Pending::Analytics), Response::Analytics { rows }) => {
                if let Some(panel) = &mut self.analytics {
                    panel.set_rows(rows);
                }
                Vec::new()
            }
            (
                Some(Pending::TaskHistory { task }),
                Response::History { entries, columns },
            ) => {
                if let Some(panel) = &mut self.analytics {
                    panel.set_history(task, entries, columns);
                }
                Vec::new()
            }
            (Some(Pending::ExcludeFromStats), Response::Task { .. }) => {
                // The flag changes what the averages say, so the whole table
                // is asked for again rather than patched in place.
                match &mut self.analytics {
                    Some(panel) => {
                        panel.loading = true;
                        let (board_id, filter) = (panel.requested_board(), panel.filter.clone());
                        vec![self.send(
                            Request::Analytics { board_id, filter },
                            Pending::Analytics,
                        )]
                    }
                    None => Vec::new(),
                }
            }
            (Some(Pending::RepeatsPanel), Response::Repeats { entries }) => {
                let Some(board_id) = self.board.as_ref().map(|b| b.detail.board.id) else {
                    return Vec::new();
                };
                let tz = self
                    .user
                    .as_ref()
                    .map(|u| u.timezone)
                    .unwrap_or(chrono_tz::UTC);
                match &mut self.overlay {
                    Some(Overlay::Repeats(panel)) if panel.board_id == board_id => {
                        panel.set_entries(entries)
                    }
                    _ => {
                        let mut panel = RepeatsPanel::new(board_id, tz);
                        panel.set_entries(entries);
                        self.overlay = Some(Overlay::Repeats(Box::new(panel)));
                    }
                }
                Vec::new()
            }
            (Some(Pending::StopRepeat), Response::Task { task }) => {
                self.toast(
                    Severity::Success,
                    format!("{} no longer repeats", task.title),
                );
                if let Some(Overlay::Repeats(panel)) = &mut self.overlay {
                    panel.remove(task.id);
                }
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    b.upsert(task);
                }
                Vec::new()
            }
            (Some(Pending::Search), Response::SearchResults { hits }) => {
                if hits.is_empty() {
                    self.toast(Severity::Info, "no matching tasks");
                }
                self.hits = hits;
                self.hit_sel = 0;
                Vec::new()
            }
            (Some(Pending::Scan), Response::Scan(outcome)) => self.on_scan(outcome),
            (Some(Pending::Resolve), Response::Resolved { task, board }) => self.on_resolved(*task, *board),
            (Some(Pending::ControlOp), Response::Task { task }) => {
                self.toast(Severity::Success, format!("{}: done", task.title));
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    b.upsert(task);
                }
                Vec::new()
            }
            (Some(Pending::ControlOp), Response::Done) => {
                self.toast(Severity::Success, "done");
                Vec::new()
            }
            (Some(Pending::Lookup(cmd)), Response::Found(found)) => self.on_found(*cmd, found),
            (Some(Pending::Print), Response::Done) => {
                self.toast(Severity::Info, "print job queued");
                Vec::new()
            }
            (Some(Pending::DeleteTask), Response::Task { task }) => {
                let days = self
                    .board
                    .as_ref()
                    .map(|b| b.detail.board.purge_deleted_after_secs / 86_400)
                    .unwrap_or(0);
                self.toast(
                    Severity::Info,
                    format!("moved to the archive, restore it from there within {days} days"),
                );
                if let Some(b) = &mut self.board {
                    b.remove(task.id);
                }
                Vec::new()
            }
            (Some(Pending::PurgeTask), Response::Done) => {
                self.toast(Severity::Info, "deleted for good");
                Vec::new()
            }
            (Some(Pending::DeleteBoard), Response::Done) => {
                self.toast(Severity::Info, "board deleted");
                vec![self.send(Request::ListBoards, Pending::Boards)]
            }
            _ => Vec::new(),
        }
    }

    fn on_scan(&mut self, outcome: ScanOutcome) -> Vec<Cmd> {
        match outcome {
            ScanOutcome::Applied {
                task, column_name, ..
            } => {
                self.toast(
                    Severity::Success,
                    format!("{} moved to {column_name}", task.title),
                );
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    b.upsert(*task);
                }
                Vec::new()
            }
            ScanOutcome::UnknownTask => {
                self.toast(Severity::Error, "scan: unknown task");
                vec![self.bell()]
            }
            ScanOutcome::NoPermission => {
                self.toast(Severity::Error, "scan: you cannot see that task");
                vec![self.bell()]
            }
            ScanOutcome::Refused { reason } => {
                self.toast(Severity::Warning, format!("scan refused: {reason}"));
                vec![self.bell()]
            }
            ScanOutcome::BadScan { reason } => {
                self.toast(Severity::Error, format!("bad scan: {reason}"));
                vec![self.bell()]
            }
            ScanOutcome::BlockedByDependencies { task_id, open } => {
                match self.board.as_ref().map(|b| b.detail.board.finished_col) {
                    Some(to) => self.blocked_dialog(task_id, to, &open, None),
                    None => self.toast(
                        Severity::Warning,
                        "scan: open dependencies, open the board to override",
                    ),
                }
                vec![self.bell()]
            }
            ScanOutcome::Answered { task, answer } => {
                self.toast(
                    Severity::Success,
                    format!("{}: answered {answer}", task.title),
                );
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    b.upsert(*task);
                }
                Vec::new()
            }
            ScanOutcome::FinishedChildren { root, finished } => {
                self.toast(
                    Severity::Success,
                    format!(
                        "{}: finished {finished} running task{}",
                        root.title,
                        if finished == 1 { "" } else { "s" }
                    ),
                );
                if let Some(b) = &mut self.board
                    && b.detail.board.id == root.board_id
                {
                    b.upsert(*root);
                }
                Vec::new()
            }
        }
    }

    fn on_error(&mut self, pending: Option<Pending>, error: ErrorBody) -> Vec<Cmd> {
        match (pending, error.code) {
            (Some(Pending::Welcome), _) => {
                self.conn = Conn::Lost;
                self.conn_error = Some(error.reason);
            }
            (Some(Pending::MoveTask { task, to, answer }), ErrorCode::BlockedByDependencies) => {
                let open: Vec<TaskId> = error
                    .detail
                    .and_then(|d| serde_json::from_value(d).ok())
                    .unwrap_or_default();
                self.blocked_dialog(task, to, &open, answer);
            }
            (Some(Pending::SaveTask), ErrorCode::Conflict) => {
                let current: Option<Task> =
                    error.detail.and_then(|d| serde_json::from_value(d).ok());
                match (self.overlay.take(), current) {
                    (Some(Overlay::TaskForm(mut form)), Some(current)) => {
                        form.saving = false;
                        self.overlay = Some(Overlay::Conflict {
                            form,
                            current: Box::new(current),
                        });
                    }
                    (other, _) => {
                        self.overlay = other;
                        self.toast(Severity::Error, error.reason);
                    }
                }
            }
            (Some(Pending::MemberOp { uid }), ErrorCode::MemberHasTasks) => {
                let count = error
                    .detail
                    .and_then(|d| serde_json::from_value::<Vec<TaskId>>(d).ok())
                    .map(|v| v.len())
                    .unwrap_or(0);
                let name = self.user_name(uid);
                if let Some(Overlay::Members(panel)) = self.overlay.take() {
                    let board_id = panel.board_id;
                    let plural = if count == 1 { "" } else { "s" };
                    self.confirm(
                        "Member has tasks",
                        format!("{name} still has {count} task{plural} assigned here.\nUnassign them and remove the member?"),
                        ConfirmAction::Send {
                            request: Request::RemoveMember { board_id, uid, unassign: true },
                            pending: Pending::MemberOp { uid },
                        },
                        Some(Box::new(Overlay::Members(panel))),
                    );
                } else {
                    self.toast(Severity::Error, error.reason);
                }
            }
            (Some(Pending::Settings | Pending::Timezone), _) => {
                if let Some(Overlay::Settings(form)) = &mut self.overlay {
                    form.saving = false;
                    form.error = Some(error.reason);
                } else {
                    self.toast(Severity::Error, error.reason);
                }
            }
            (Some(Pending::BoardOp | Pending::CreateBoardForm), _) => {
                if let Some(Overlay::BoardForm(form)) = &mut self.overlay {
                    form.saving = false;
                    form.error = Some(error.reason);
                } else {
                    self.toast(Severity::Error, error.reason);
                }
            }
            (Some(Pending::MemberOp { .. } | Pending::ColumnOp | Pending::AdminOp), _) => {
                match &mut self.overlay {
                    Some(Overlay::Members(p)) => p.error = Some(error.reason),
                    Some(Overlay::Columns(p)) => p.error = Some(error.reason),
                    Some(Overlay::Users(p)) => p.error = Some(error.reason),
                    _ => self.toast(Severity::Error, error.reason),
                }
            }
            (Some(Pending::SaveTask), _) => {
                if let Some(Overlay::TaskForm(form)) = &mut self.overlay {
                    form.saving = false;
                    form.error = Some(error.reason);
                } else {
                    self.toast(Severity::Error, error.reason);
                }
            }
            (Some(Pending::TemplateOp), _) => match &mut self.overlay {
                Some(Overlay::TaskForm(form)) => {
                    form.saving = false;
                    form.error = Some(error.reason);
                }
                Some(Overlay::Templates(panel)) => panel.error = Some(error.reason),
                _ => self.toast(Severity::Error, error.reason),
            },
            (Some(Pending::StopRepeat), _) => match &mut self.overlay {
                Some(Overlay::Repeats(panel)) => panel.error = Some(error.reason),
                _ => self.toast(Severity::Error, error.reason),
            },
            (Some(Pending::ProgramOp), _) => match &mut self.overlay {
                Some(Overlay::ProgramForm(form)) => {
                    form.saving = false;
                    form.error = Some(error.reason);
                }
                Some(Overlay::Programs(panel)) => panel.error = Some(error.reason),
                _ => self.toast(Severity::Error, error.reason),
            },
            (Some(Pending::StartProgram), _) => match &mut self.overlay {
                Some(Overlay::StartProgram(form)) => {
                    form.saving = false;
                    form.error = Some(error.reason);
                }
                _ => self.toast(Severity::Error, error.reason),
            },
            (Some(Pending::CancelRun), _) => match &mut self.overlay {
                Some(Overlay::Runs(panel)) => panel.error = Some(error.reason),
                _ => self.toast(Severity::Error, error.reason),
            },
            (_, _) => self.toast(Severity::Error, error.reason),
        }
        Vec::new()
    }

    fn on_event(&mut self, event: Event) -> Vec<Cmd> {
        match event {
            Event::BoardChanged { board_id, change } => {
                let mut cmds = vec![self.send(Request::ListBoards, Pending::Boards)];
                if self
                    .board
                    .as_ref()
                    .is_some_and(|b| b.detail.board.id == board_id)
                {
                    match change {
                        BoardChange::Deleted | BoardChange::AccessRevoked => {
                            self.board = None;
                            self.toast(
                                Severity::Warning,
                                "the board you were on is no longer available",
                            );
                        }
                        _ => cmds
                            .push(self.send(Request::GetBoard { board_id }, Pending::RefreshBoard)),
                    }
                }
                cmds
            }
            Event::TaskChanged { task, change, .. } => {
                if let Some(Overlay::TaskDetail {
                    task: shown, sel, ..
                }) = &mut self.overlay
                    && shown.id == task.id
                {
                    *shown = task.clone();
                    *sel = (*sel).min(shown.checklist.len().saturating_sub(1));
                }
                if let Some(Overlay::Archive(panel)) = &mut self.overlay
                    && self
                        .board
                        .as_ref()
                        .is_some_and(|b| b.detail.board.id == task.board_id)
                {
                    match change {
                        TaskChange::Purged | TaskChange::Restored => panel.remove(task.id),
                        TaskChange::Deleted | TaskChange::Archived => panel.upsert(task.clone()),
                        _ => {}
                    }
                }
                if let Some(b) = &mut self.board
                    && b.detail.board.id == task.board_id
                {
                    match change {
                        TaskChange::Deleted | TaskChange::Archived | TaskChange::Purged => {
                            b.remove(task.id)
                        }
                        _ => b.upsert(task),
                    }
                }
                Vec::new()
            }
            Event::PrintJob { job_id, job } => vec![Cmd::Print { job_id, job }],
            Event::Notice { text, severity } => {
                self.toast(severity, text);
                Vec::new()
            }
            Event::PrefsChanged { prefs } => {
                self.scan = scan::detector_for(&prefs.scanner);
                if let Some(u) = &mut self.user {
                    u.prefs = prefs;
                }
                Vec::new()
            }
        }
    }

    fn on_term(&mut self, ev: TermEvent) -> Vec<Cmd> {
        match ev {
            TermEvent::Resize(w, h) => {
                self.size = (w, h);
                Vec::new()
            }
            TermEvent::Key(k) if k.kind == KeyEventKind::Release => Vec::new(),
            TermEvent::Key(k) => self.on_key(k),
            TermEvent::Mouse(m) => {
                self.mouse_seen = true;
                self.mouse_pos = Some((m.column, m.row));
                self.on_mouse(m)
            }
            _ => Vec::new(),
        }
    }

    fn on_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        let ctrl_c = k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL);
        if self.too_small() || self.conn == Conn::Lost {
            // Nothing can be drawn properly, so any of the quit keys just quits.
            return if ctrl_c || matches!(k.code, KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter)
            {
                vec![Cmd::Quit]
            } else {
                Vec::new()
            };
        }
        if ctrl_c {
            self.confirm(
                "Quit",
                "Quit Taskologic? This closes your session.",
                ConfirmAction::Quit,
                None,
            );
            return Vec::new();
        }
        // Esc ends a control code in progress before anything else sees it.
        if k.code == KeyCode::Esc && (self.scan.frame_open() || self.control.busy()) {
            self.scan.cancel();
            self.control.clear();
            self.toast(Severity::Info, "control code cancelled");
            return Vec::new();
        }
        // The detector sees every character on every screen, so control
        // codes work wherever the cursor is. A partial match is handed back
        // as typing the moment it stops looking like a code.
        if self.scanner_feeding()
            && let Some(sk) = scan::scan_key(&k)
        {
            return match self.scan.push(sk, self.now_ms) {
                Feed::Held => Vec::new(),
                Feed::Scan(Ok(p)) => self.system_scan(p),
                Feed::Scan(Err(e)) => {
                    self.toast(Severity::Error, format!("bad scan: {e}"));
                    vec![self.bell()]
                }
                Feed::Frame(body) => self.control_frame(&body),
                Feed::Pass(keys) => keys
                    .into_iter()
                    .flat_map(|key| self.route_key(scan::key_event(key)))
                    .collect(),
            };
        }
        self.route_key(k)
    }

    /// A key that is not part of a barcode, to whatever is showing.
    fn route_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        if self.is_widget_overlay() {
            return self.widget_overlay_event(TermEvent::Key(k));
        }
        match &self.overlay {
            Some(Overlay::Conflict { .. }) => return self.conflict_key(k),
            Some(Overlay::PickColumn { .. }) => return self.pick_column_key(k),
            Some(Overlay::ColumnPick { .. }) => return self.column_pick_key(k),
            Some(Overlay::TaskDetail { .. }) => return self.task_detail_key(k),
            Some(Overlay::Question { .. }) => return self.question_key(k),
            Some(_) => return self.overlay_key(k),
            None => {}
        }
        if self.analytics.is_some() {
            return self.analytics_event(TermEvent::Key(k));
        }
        if self.search.is_focused() {
            return self.search_key(k);
        }
        self.normal_key(k)
    }

    /// Whether keystrokes go through the scan detector at all: the scanner
    /// is on, and in manual mode the button was pressed.
    fn scanner_feeding(&self) -> bool {
        self.scanner_enabled() && (!self.scanner_manual_only() || self.manual_scan)
    }

    /// Milliseconds a control code waits for what comes next; 0 is forever.
    fn control_timeout_ms(&self) -> u64 {
        self.user
            .as_ref()
            .map(|u| u64::from(u.prefs.scanner.control_timeout_secs) * 1000)
            .unwrap_or(20_000)
    }

    /// A task's code arrived. On the board it does what it says; under a
    /// window it would act on something the screen is not showing.
    fn system_scan(&mut self, p: ScanPayload) -> Vec<Cmd> {
        if let Some(ms) = self.scan.last_scan_duration_ms() {
            tracing::debug!(duration_ms = ms, "scan captured");
        }
        if self.control.armed.is_some() {
            return vec![self.send(Request::Resolve { short_id: p.short_id }, Pending::Resolve)];
        }
        if !self.scanner_listening() {
            if self.text_focused() {
                // A code where text goes is text, the way it always was:
                // somebody may well search for a short id.
                return p
                    .encode()
                    .chars()
                    .flat_map(|c| self.route_key(scan::key_event(taskologic_core::barcode::ScanKey::Char(c))))
                    .collect();
            }
            self.toast(Severity::Warning, "close what is open, then scan the task again");
            return vec![self.bell()];
        }
        let payload = p.encode();
        vec![self.send(Request::Scan { payload }, Pending::Scan)]
    }

    /// The body of a control frame, between `--1` and `--`.
    fn control_frame(&mut self, body: &str) -> Vec<Cmd> {
        match control::parse(body) {
            Ok(commands) => self.run_controls(commands),
            Err(e) => {
                self.toast(Severity::Error, format!("bad control code: {e}"));
                vec![self.bell()]
            }
        }
    }

    /// Commands in order, each taking the values that follow it. A value on
    /// its own goes to whatever is waiting for one.
    fn run_controls(&mut self, commands: Vec<Control>) -> Vec<Cmd> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < commands.len() {
            let mut values = Vec::new();
            let mut j = i + 1;
            while let Some(Control::Value(v)) = commands.get(j) {
                values.push(v.clone());
                j += 1;
            }
            match &commands[i] {
                Control::Value(v) => {
                    values.insert(0, v.clone());
                    out.extend(self.control_values(values));
                }
                other => out.extend(self.control_command(other.clone(), values)),
            }
            i = j;
        }
        out
    }

    /// Values for whatever was left waiting.
    fn control_values(&mut self, values: Vec<Value>) -> Vec<Cmd> {
        match self.control.take_waiting() {
            Some(Waiting::Insert) => self.control_insert(&values),
            Some(Waiting::Replace) => self.control_replace(&values),
            Some(Waiting::Search { archive }) => self.control_search(archive, &values),
            None => {
                self.toast(Severity::Warning, "nothing is waiting for a value; scan a command first");
                vec![self.bell()]
            }
        }
    }

    fn control_command(&mut self, cmd: Control, values: Vec<Value>) -> Vec<Cmd> {
        use Control::*;
        let busy = |app: &mut Self| {
            app.toast(Severity::Warning, "close what is open first");
            vec![app.bell()]
        };
        match cmd {
            Key(c) => self.route_key(scan::key_event(taskologic_core::barcode::ScanKey::Char(c))),
            Named(k) => {
                if k == taskologic_core::control::NamedKey::Esc && self.control.clear().is_some() {
                    self.toast(Severity::Info, "control code cancelled");
                    return Vec::new();
                }
                self.route_key(named_key_event(k))
            }
            Dashboard => {
                if self.overlay.is_some() {
                    return busy(self);
                }
                self.route_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE))
            }
            NextBoard | PrevBoard => {
                if self.overlay.is_some() {
                    return busy(self);
                }
                self.control_step_board(matches!(cmd, NextBoard))
            }
            Search { archive } => {
                if values.is_empty() {
                    self.control.wait_for(Waiting::Search { archive }, self.now_ms);
                    return Vec::new();
                }
                self.control_search(archive, &values)
            }
            Ping => {
                self.toast(Severity::Info, "scanner ok");
                Vec::new()
            }
            Insert => {
                if values.is_empty() {
                    self.control.wait_for(Waiting::Insert, self.now_ms);
                    return Vec::new();
                }
                self.control_insert(&values)
            }
            Replace => {
                if values.is_empty() {
                    self.control.wait_for(Waiting::Replace, self.now_ms);
                    return Vec::new();
                }
                self.control_replace(&values)
            }
            Value(_) => unreachable!("values are gathered by run_controls"),
            Sticky => {
                let label = self.control.armed.as_mut().map(|a| {
                    a.sticky = true;
                    a.label()
                });
                match label {
                    Some(l) => self.toast(Severity::Info, format!("every scan: {l}")),
                    None => self.toast(Severity::Warning, "sticky goes after a command: scan the command first"),
                }
                Vec::new()
            }
            Selected => self.control_selected(),
            Show(Some(id)) => {
                self.control.arm(Armed { command: Show(None), values: Vec::new(), sticky: false }, self.now_ms);
                vec![self.send(Request::Resolve { short_id: id }, Pending::Resolve)]
            }
            MoveLeft | MoveRight | MoveUp | MoveDown | MoveTop | MoveBottom | MoveTo(_) | Delete
            | Print(_) | AssignMe | UnassignMe | ToggleAssign | Set(_) | Show(None) => {
                let armed = Armed { command: cmd, values, sticky: false };
                let label = armed.label();
                self.control.arm(armed, self.now_ms);
                self.toast(Severity::Info, format!("next scan: {label}"));
                Vec::new()
            }
            ShowBoard(id) | Analytics(id) => {
                vec![self.send(
                    Request::Lookup { kind: LookupKind::Board, short_id: id },
                    Pending::Lookup(Box::new(cmd)),
                )]
            }
            StartProgram(id) | StartProgramNow(id) => {
                vec![self.send(
                    Request::Lookup { kind: LookupKind::Program, short_id: id },
                    Pending::Lookup(Box::new(cmd)),
                )]
            }
            NewFromTemplate(id, _) | NewFromTemplateAsk(id, _) => {
                vec![self.send(
                    Request::Lookup { kind: LookupKind::Template, short_id: id },
                    Pending::Lookup(Box::new(cmd)),
                )]
            }
        }
    }

    /// What a targeted code named came back; do what the code says with it.
    fn on_found(&mut self, cmd: Control, found: Found) -> Vec<Cmd> {
        use Control::*;
        let tz = self.timezone();
        match (cmd, found) {
            (ShowBoard(_), Found::Board { board }) => {
                if self.overlay.is_some() {
                    self.toast(Severity::Warning, "close what is open first");
                    return vec![self.bell()];
                }
                vec![self.send(
                    Request::GetBoard { board_id: board.id },
                    Pending::OpenBoard { focus_task: None },
                )]
            }
            (Analytics(_), Found::Board { board }) => {
                if self.overlay.is_some() {
                    self.toast(Severity::Warning, "close what is open first");
                    return vec![self.bell()];
                }
                self.open_analytics(Some(board.id))
            }
            (StartProgram(_), Found::Program { program, board }) => {
                if self.overlay.is_some() {
                    self.toast(Severity::Warning, "close what is open first");
                    return vec![self.bell()];
                }
                let Some(col) = Self::column_for(&board, control::ColumnRef::Todo) else {
                    return Vec::new();
                };
                let name = board.columns.iter().find(|c| c.id == col).map(|c| c.name.clone()).unwrap_or_default();
                let form = StartProgramForm::new(*program, col, name, tz);
                self.overlay = Some(Overlay::StartProgram(Box::new(form)));
                Vec::new()
            }
            (StartProgramNow(_), Found::Program { program, board }) => {
                let column_id = Self::column_for(&board, control::ColumnRef::Todo);
                self.toast(Severity::Info, format!("starting {}", program.name));
                vec![self.send(
                    Request::StartProgram {
                        program_id: program.id,
                        column_id,
                        start_at: None,
                        fan_out: Vec::new(),
                    },
                    Pending::StartProgram,
                )]
            }
            (NewFromTemplate(_, col), Found::Template { template, board }) => {
                let Some(column) = Self::column_for(&board, col) else {
                    self.toast(Severity::Warning, "that board has no such column");
                    return vec![self.bell()];
                };
                // The form knows how a template prefills a task; it is
                // filled and saved without being shown.
                let form = TaskForm::create_from_template(board.id, column, tz, &template);
                match form.values() {
                    Ok(save) => self.save_task(form.mode.clone(), save),
                    Err(e) => {
                        self.toast(Severity::Warning, e);
                        vec![self.bell()]
                    }
                }
            }
            (NewFromTemplateAsk(_, col), Found::Template { template, board }) => {
                if self.overlay.is_some() {
                    self.toast(Severity::Warning, "close what is open first");
                    return vec![self.bell()];
                }
                let Some(column) = Self::column_for(&board, col) else {
                    self.toast(Severity::Warning, "that board has no such column");
                    return vec![self.bell()];
                };
                let form = TaskForm::create_from_template(board.id, column, tz, &template);
                self.overlay = Some(Overlay::TaskForm(Box::new(form)));
                vec![self.send(
                    Request::ListMembers { board_id: board.id },
                    Pending::FormMembers,
                )]
            }
            _ => Vec::new(),
        }
    }

    /// The card with a board's, template's or program's own codes.
    fn print_codes_for(&mut self, heading: &str, codes: Vec<(String, Control)>) -> Vec<Cmd> {
        let codes = codes.into_iter().map(|(l, c)| (l, vec![c])).collect();
        self.print_codes(heading, codes)
    }

    /// The selected task stands in for a scan.
    fn control_selected(&mut self) -> Vec<Cmd> {
        let Some(armed) = self.control.use_armed(self.now_ms) else {
            self.toast(Severity::Warning, "nothing is armed; scan a command first");
            return vec![self.bell()];
        };
        let Some((task, board)) = self
            .board
            .as_ref()
            .and_then(|b| b.selected_task().cloned().map(|t| (t, b.detail.board.clone())))
        else {
            self.toast(Severity::Warning, "no task is selected on screen");
            return vec![self.bell()];
        };
        self.apply_armed(armed, task, board)
    }

    /// The task a code named came back; the armed command acts on it.
    fn on_resolved(&mut self, task: Task, board: Board) -> Vec<Cmd> {
        let Some(armed) = self.control.use_armed(self.now_ms) else {
            // Cancelled while the answer was on its way.
            return Vec::new();
        };
        self.apply_armed(armed, task, board)
    }

    /// Which column a code means on this board. Todo is the first column
    /// without a role; numbers count from the left.
    fn column_for(board: &Board, col: control::ColumnRef) -> Option<ColumnId> {
        use control::ColumnRef::*;
        let roles = [board.started_col, board.paused_col, board.finished_col];
        match col {
            Todo => board
                .columns
                .iter()
                .find(|c| !roles.contains(&c.id))
                .or(board.columns.first())
                .map(|c| c.id),
            Paused => Some(board.paused_col),
            Doing => Some(board.started_col),
            Finished => Some(board.finished_col),
            Index(n) => board.columns.get(usize::from(n).saturating_sub(1)).map(|c| c.id),
            Last => board.columns.last().map(|c| c.id),
        }
    }

    /// Do to `task` what the armed command says, through the same requests
    /// the keyboard sends. The board came with the task, so this works for
    /// a task on a board that is not the one open.
    fn apply_armed(&mut self, armed: Armed, task: Task, board: Board) -> Vec<Cmd> {
        use Control::*;
        let task_id = task.id;
        let col_idx = board.columns.iter().position(|c| c.id == task.column_id).unwrap_or(0);
        let refuse = |app: &mut Self, why: &str| {
            app.toast(Severity::Warning, why.to_string());
            vec![app.bell()]
        };
        match armed.command {
            MoveLeft | MoveRight => {
                let to = if armed.command == MoveLeft { col_idx.checked_sub(1) } else { Some(col_idx + 1) };
                match to.and_then(|i| board.columns.get(i)) {
                    Some(c) => vec![self.send_move(task_id, c.id, None, false, None)],
                    None => refuse(self, "already in the outermost column"),
                }
            }
            MoveUp | MoveDown | MoveTop | MoveBottom => {
                // Positions come from the board on screen; another board's
                // order is not known here.
                let siblings: Vec<(TaskId, i64)> = match &self.board {
                    Some(b) if b.detail.board.id == board.id => {
                        b.tasks_in(col_idx).iter().map(|t| (t.id, t.position)).collect()
                    }
                    _ => return refuse(self, "open that board to move a task within its column"),
                };
                let idx = siblings.iter().position(|(id, _)| *id == task_id).unwrap_or(0);
                let position = match armed.command {
                    MoveUp if idx > 0 => Some(siblings[idx - 1].1 - 1),
                    MoveDown if idx + 1 < siblings.len() => Some(siblings[idx + 1].1 + 1),
                    MoveTop if idx > 0 => Some(siblings[0].1 - 1),
                    MoveBottom if idx + 1 < siblings.len() => Some(siblings[siblings.len() - 1].1 + 1),
                    _ => return refuse(self, "it is there already"),
                };
                vec![self.send_move(task_id, task.column_id, position, false, None)]
            }
            MoveTo(None) => {
                let roles = [
                    (board.started_col, "doing"),
                    (board.paused_col, "paused"),
                    (board.finished_col, "done"),
                ];
                let options = board
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let role = roles.iter().find(|(id, _)| *id == c.id).map(|(_, r)| format!(", {r}")).unwrap_or_default();
                        (c.id, format!("{}  {}{role}", i + 1, c.name))
                    })
                    .collect();
                self.overlay = Some(Overlay::ColumnPick {
                    task: Box::new(task),
                    options,
                    sel: col_idx,
                    since_ms: self.now_ms,
                });
                Vec::new()
            }
            MoveTo(Some(col)) => match Self::column_for(&board, col) {
                Some(c) => vec![self.send_move(task_id, c, None, false, None)],
                None => refuse(self, "this board has no such column"),
            },
            Delete => vec![self.send(Request::DeleteTask { task_id }, Pending::DeleteTask)],
            Print(slip) => vec![self.send(Request::PrintTask { task_id, slip: Some(slip) }, Pending::Print)],
            AssignMe | UnassignMe | ToggleAssign => {
                let Some(me) = self.user.as_ref().map(|u| u.uid) else {
                    return Vec::new();
                };
                let mut draft = task.draft();
                let has = draft.assignees.contains(&me);
                match armed.command {
                    AssignMe if !has => draft.assignees.push(me),
                    UnassignMe if has => draft.assignees.retain(|u| *u != me),
                    ToggleAssign if has => draft.assignees.retain(|u| *u != me),
                    ToggleAssign => draft.assignees.push(me),
                    _ => return refuse(self, "nothing to change"),
                }
                vec![self.send(
                    Request::UpdateTask { task_id, version: task.version, draft },
                    Pending::ControlOp,
                )]
            }
            Set(field) if armed.values.is_empty() => {
                let cmds = self.open_task_form(
                    FormMode::Edit { task: task.id, version: task.version },
                    Some(&task),
                );
                if let Some(Overlay::TaskForm(form)) = &mut self.overlay {
                    form.focus_field(field);
                }
                cmds
            }
            Set(field) => self.set_field(task, field, &armed.values),
            Show(_) => {
                self.open_detail(task);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Keys of the column pick a bare "move to" opened.
    fn column_pick_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        let Some(Overlay::ColumnPick { task, options, mut sel, since_ms }) = self.overlay.take() else {
            return Vec::new();
        };
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => sel = sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => sel = (sel + 1).min(options.len().saturating_sub(1)),
            KeyCode::Char(c @ '1'..='9') => {
                let i = usize::from(c as u8 - b'1');
                if let Some((col, _)) = options.get(i) {
                    return vec![self.send_move(task.id, *col, None, false, None)];
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.toast(Severity::Info, "move cancelled");
                return Vec::new();
            }
            KeyCode::Enter => {
                if let Some((col, _)) = options.get(sel) {
                    return vec![self.send_move(task.id, *col, None, false, None)];
                }
            }
            _ => {}
        }
        self.overlay = Some(Overlay::ColumnPick { task, options, sel, since_ms });
        Vec::new()
    }

    /// One field of a task set by scan, the rest untouched.
    fn set_field(&mut self, task: Task, field: control::Field, values: &[Value]) -> Vec<Cmd> {
        use control::Field::*;
        let task_id = task.id;
        let (tz, me) = self
            .user
            .as_ref()
            .map(|u| (u.timezone, u.uid))
            .unwrap_or((chrono_tz::UTC, 0));
        let now = chrono::Utc::now();
        let mut draft = task.draft();
        let text = self.control_text(values);
        let fail = |app: &mut Self, why: String| {
            app.toast(Severity::Warning, why);
            vec![app.bell()]
        };
        match field {
            Title => {
                if text.trim().is_empty() {
                    return fail(self, "a title cannot be empty".into());
                }
                draft.title = text;
            }
            Description => draft.description = text,
            Start => match values_date(values, task.start_at, now, tz) {
                Ok(d) => draft.start_at = d,
                Err(e) => return fail(self, e),
            },
            Due => match values_date(values, task.due_at, now, tz) {
                Ok(d) => draft.due_at = d,
                Err(e) => return fail(self, e),
            },
            RemindStart => match values_minutes(values) {
                Ok(m) => draft.reminder_start_minutes = m,
                Err(e) => return fail(self, e),
            },
            RemindDue => match values_minutes(values) {
                Ok(m) => draft.reminder_due_minutes = m,
                Err(e) => return fail(self, e),
            },
            Assign => {
                for v in values {
                    match v {
                        Value::Clear => draft.assignees.clear(),
                        Value::Me => {
                            if !draft.assignees.contains(&me) {
                                draft.assignees.push(me);
                            }
                        }
                        Value::Text(name) => {
                            let found = self
                                .users
                                .iter()
                                .find(|(_, n)| n.eq_ignore_ascii_case(name.trim()))
                                .map(|(uid, _)| *uid);
                            match found {
                                Some(uid) if !draft.assignees.contains(&uid) => draft.assignees.push(uid),
                                Some(_) => {}
                                None => return fail(self, format!("nobody called {name:?} here")),
                            }
                        }
                        _ => return fail(self, "an assignee is a name, my name, or clear".into()),
                    }
                }
            }
            Checklist => {
                if text.trim().is_empty() {
                    return fail(self, "a checklist item needs text".into());
                }
                draft.checklist.push(taskologic_core::task::ChecklistItem {
                    text: text.trim().to_string(),
                    done: false,
                });
            }
            ChecklistItem(n) => {
                let index = usize::from(n).saturating_sub(1);
                let Some(item) = task.checklist.get(index) else {
                    return fail(self, format!("the task has no checklist item {n}"));
                };
                return vec![self.send(
                    Request::SetChecklistItem { task_id, index, done: !item.done },
                    Pending::ChecklistOp,
                )];
            }
            ExcludeFromStats => {
                return vec![self.send(
                    Request::SetExcludeFromStats { task_id, excluded: !task.exclude_from_stats },
                    Pending::ExcludeFromStats,
                )];
            }
        }
        vec![self.send(
            Request::UpdateTask { task_id, version: task.version, draft },
            Pending::ControlOp,
        )]
    }

    /// What a run of values spells, in the user's zone.
    fn control_text(&self, values: &[Value]) -> String {
        let (tz, name) = self
            .user
            .as_ref()
            .map(|u| (u.timezone, u.username.as_str()))
            .unwrap_or((chrono_tz::UTC, ""));
        values_text(values, chrono::Utc::now(), tz, name)
    }

    /// Type into whatever has the focus, one key at a time, the way a wedge
    /// scanner would.
    fn control_insert(&mut self, values: &[Value]) -> Vec<Cmd> {
        let text = self.control_text(values);
        if !self.text_focused() {
            self.toast(Severity::Warning, "nothing to type into: put the cursor in a field first");
            return vec![self.bell()];
        }
        let mut out = Vec::new();
        for c in text.chars() {
            out.extend(self.route_key(scan::key_event(taskologic_core::barcode::ScanKey::Char(c))));
        }
        out
    }

    /// Replace what the focused field holds: select it all, as Ctrl+A does
    /// in every text widget, then type. An empty value deletes it.
    fn control_replace(&mut self, values: &[Value]) -> Vec<Cmd> {
        if !self.text_focused() {
            self.toast(Severity::Warning, "nothing to replace: put the cursor in a field first");
            return vec![self.bell()];
        }
        let text = self.control_text(values);
        let mut out = self.route_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        if text.is_empty() {
            out.extend(self.route_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
            return out;
        }
        for c in text.chars() {
            out.extend(self.route_key(scan::key_event(taskologic_core::barcode::ScanKey::Char(c))));
        }
        out
    }

    fn control_search(&mut self, archive: bool, values: &[Value]) -> Vec<Cmd> {
        if self.overlay.is_some() {
            self.toast(Severity::Warning, "close what is open first");
            return vec![self.bell()];
        }
        let text = self.control_text(values);
        self.board = None;
        self.analytics = None;
        self.search.set_text(text);
        self.include_archived.set_checked(archive);
        self.search.focus().set(true);
        self.search_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    }

    /// The next or previous board in dashboard order, of the ones the user
    /// is a member of; from the dashboard, the first or the last.
    fn control_step_board(&mut self, forward: bool) -> Vec<Cmd> {
        let ids: Vec<taskologic_core::ids::BoardId> =
            self.boards.iter().filter(|b| b.is_member).map(|b| b.id).collect();
        if ids.is_empty() {
            self.toast(Severity::Warning, "no boards to go to");
            return vec![self.bell()];
        }
        let current = self
            .board
            .as_ref()
            .and_then(|b| ids.iter().position(|id| *id == b.detail.board.id));
        let next = match (current, forward) {
            (Some(i), true) => (i + 1) % ids.len(),
            (Some(i), false) => (i + ids.len() - 1) % ids.len(),
            (None, true) => 0,
            (None, false) => ids.len() - 1,
        };
        let board_id = ids[next];
        vec![self.send(
            Request::GetBoard { board_id },
            Pending::OpenBoard { focus_task: None },
        )]
    }

    /// What the status line says about control codes, if anything.
    pub fn control_status(&self) -> Option<String> {
        if self.scan.frame_open() {
            return Some("reading a long code, scan its next piece (Esc cancels)".into());
        }
        self.control.status()
    }

    fn normal_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Char('q') => {
                self.confirm(
                    "Quit",
                    "Quit Taskologic? This closes your session.",
                    ConfirmAction::Quit,
                    None,
                );
                Vec::new()
            }
            KeyCode::Char('?') => {
                self.overlay = Some(Overlay::Help);
                Vec::new()
            }
            KeyCode::Char('/') => {
                self.board = None;
                self.search.focus().set(true);
                Vec::new()
            }
            KeyCode::Char('b') => {
                self.board = None;
                Vec::new()
            }
            KeyCode::Char('M') => {
                let items = crate::ui::more_items(self);
                self.overlay = Some(Overlay::Menu {
                    items,
                    sel: None,
                    areas: Vec::new(),
                });
                Vec::new()
            }
            KeyCode::Char('S') => self.open_settings(),
            KeyCode::Char('s') => {
                if !self.scanner_enabled() {
                    self.toast(
                        Severity::Info,
                        "barcode scanner integration is off in your settings",
                    );
                } else if !self.scanner_manual_only() {
                    self.toast(
                        Severity::Info,
                        "the scanner is always listening on boards and the dashboard",
                    );
                } else {
                    self.manual_scan = !self.manual_scan;
                    let state = if self.manual_scan { "on" } else { "off" };
                    self.toast(Severity::Info, format!("manual scan mode {state}"));
                }
                Vec::new()
            }
            _ if self.board.is_some() => self.board_key(k, shift),
            _ => self.dashboard_key(k),
        }
    }

    fn dashboard_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        let searching = !self.hits.is_empty();
        let len = if searching {
            self.hits.len()
        } else {
            self.boards.len()
        };
        let sel = if searching {
            &mut self.hit_sel
        } else {
            &mut self.board_sel
        };
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => *sel = (*sel + 1).min(len.saturating_sub(1)),
            KeyCode::Esc if searching => {
                self.hits.clear();
                self.search.clear();
            }
            KeyCode::Enter if searching => {
                if let Some(hit) = self.hits.get(self.hit_sel) {
                    let (board_id, task) = (hit.board_id, hit.task_id);
                    return vec![self.send(
                        Request::GetBoard { board_id },
                        Pending::OpenBoard {
                            focus_task: Some(task),
                        },
                    )];
                }
            }
            KeyCode::Enter => return self.open_selected_board(),
            KeyCode::Char('u') => return self.open_users(),
            KeyCode::Char('i') => {
                self.include_archived.flip_checked();
            }
            KeyCode::Char('D') => {
                if let Some(b) = self.boards.get(self.board_sel) {
                    let privileged = self
                        .user
                        .as_ref()
                        .is_some_and(|u| u.is_admin || u.uid == b.owner_uid);
                    if !privileged {
                        self.toast(
                            Severity::Info,
                            "only the board owner or an admin can delete a board",
                        );
                    } else if b.is_locked {
                        self.toast(
                            Severity::Info,
                            "this board is locked, unlock it in board settings first",
                        );
                    } else {
                        let (board_id, name) = (b.id, b.name.clone());
                        self.confirm(
                            "Delete board",
                            format!("Delete the board \"{name}\" and everything on it?"),
                            ConfirmAction::Send {
                                request: Request::DeleteBoard { board_id },
                                pending: Pending::DeleteBoard,
                            },
                            None,
                        );
                    }
                }
            }
            KeyCode::Char('N') => return self.open_board_form_create(),
            // From the dashboard the table covers every board in reach.
            KeyCode::Char('A') => return self.open_analytics(None),
            _ => {}
        }
        Vec::new()
    }

    fn open_selected_board(&mut self) -> Vec<Cmd> {
        match self.boards.get(self.board_sel) {
            Some(b) if !b.is_member => {
                self.toast(Severity::Info, "you are not a member of this private board; as an admin you can only delete it (D)");
                Vec::new()
            }
            Some(b) => {
                let board_id = b.id;
                vec![self.send(
                    Request::GetBoard { board_id },
                    Pending::OpenBoard { focus_task: None },
                )]
            }
            None => Vec::new(),
        }
    }

    fn search_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        match k.code {
            KeyCode::Esc => {
                self.search.focus().set(false);
                Vec::new()
            }
            KeyCode::Enter => {
                self.search.focus().set(false);
                let query = self.search.text().trim().to_string();
                if query.is_empty() {
                    return Vec::new();
                }
                let include_archived = self.include_archived.checked();
                vec![self.send(
                    Request::Search {
                        query,
                        include_archived,
                    },
                    Pending::Search,
                )]
            }
            _ => {
                self.search.handle(&TermEvent::Key(k), Regular);
                Vec::new()
            }
        }
    }

    fn board_key(&mut self, k: KeyEvent, shift: bool) -> Vec<Cmd> {
        let can_print = self.show_print();
        let Some(b) = &mut self.board else {
            return Vec::new();
        };
        let cols = b.column_count();
        let picked = b.picked.is_some();
        match k.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down if shift => {
                return self.move_picked(k.code);
            }
            // While a task is picked up the arrows carry it, no Shift needed.
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down if picked => {
                return self.move_picked(k.code);
            }
            KeyCode::Char('h') if picked => return self.move_picked(KeyCode::Left),
            KeyCode::Char('l') if picked => return self.move_picked(KeyCode::Right),
            KeyCode::Char('k') if picked => return self.move_picked(KeyCode::Up),
            KeyCode::Char('j') if picked => return self.move_picked(KeyCode::Down),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
                b.col = b.col.saturating_sub(1)
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                b.col = (b.col + 1).min(cols.saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => b.row = b.row.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => b.row += 1,
            KeyCode::Enter => {
                if let Some(t) = b.selected_task().cloned() {
                    self.open_detail(t);
                }
            }
            KeyCode::Char(' ') => {
                if b.picked.is_some() {
                    b.picked = None;
                } else if let Some(t) = b.selected_task() {
                    b.picked = Some(t.id);
                }
            }
            KeyCode::Esc => {
                if b.picked.is_some() {
                    b.picked = None;
                } else {
                    self.board = None;
                }
            }
            KeyCode::Char('n') => {
                if let Some(column) = b.column_id(b.col) {
                    let board = b.detail.board.id;
                    return self.open_task_form(
                        FormMode::Create {
                            board,
                            column,
                            from_template: None,
                        },
                        None,
                    );
                }
            }
            KeyCode::Char('e') => {
                if let Some(t) = b.selected_task().cloned() {
                    return self.open_task_form(
                        FormMode::Edit {
                            task: t.id,
                            version: t.version,
                        },
                        Some(&t),
                    );
                }
            }
            KeyCode::Char('d') => {
                if let Some(t) = b.selected_task() {
                    let (task_id, title) = (t.id, t.title.clone());
                    let days = b.detail.board.purge_deleted_after_secs / 86_400;
                    self.confirm(
                        "Delete task",
                        format!("Move \"{title}\" to the archive?\nIt can be restored from there for {days} days."),
                        ConfirmAction::Send { request: Request::DeleteTask { task_id }, pending: Pending::DeleteTask },
                        None,
                    );
                }
            }
            KeyCode::Char('t') => {
                if let Some(col) = b.detail.board.columns.get(b.col) {
                    let (column_id, sort_by_due) = (col.id, !col.sort_by_due);
                    return vec![self.send(
                        Request::SetColumnSort {
                            column_id,
                            sort_by_due,
                        },
                        Pending::ColumnOp,
                    )];
                }
            }
            KeyCode::Char('B') => return self.open_board_settings(),
            KeyCode::Char('m') => return self.open_members(),
            KeyCode::Char('c') => return self.open_columns(),
            KeyCode::Char('a') => {
                let board_id = b.detail.board.id;
                return vec![self.send(Request::ListArchived { board_id }, Pending::Archived)];
            }
            KeyCode::Char('u') => return self.open_users(),
            KeyCode::Char('T') => {
                let board_id = b.detail.board.id;
                return vec![
                    self.send(Request::ListTemplates { board_id }, Pending::TemplatesPanel),
                ];
            }
            KeyCode::Char('P') => {
                let board_id = b.detail.board.id;
                return vec![self.send(Request::ListPrograms { board_id }, Pending::ProgramsPanel)];
            }
            KeyCode::Char('R') => {
                let board_id = b.detail.board.id;
                return vec![self.send(Request::ListRepeats { board_id }, Pending::RepeatsPanel)];
            }
            KeyCode::Char('A') => {
                let board_id = b.detail.board.id;
                return self.open_analytics(Some(board_id));
            }
            KeyCode::Char('p') => {
                if !can_print {
                    self.toast(Severity::Info, "no printer configured on this client");
                } else if let Some(t) = b.selected_task() {
                    let task_id = t.id;
                    return vec![self.send(Request::PrintTask { task_id, slip: None }, Pending::Print)];
                }
            }
            _ => {}
        }
        if let Some(b) = &mut self.board {
            b.clamp();
        }
        Vec::new()
    }

    /// Shift+arrows: move the picked up task between columns or within one.
    fn move_picked(&mut self, code: KeyCode) -> Vec<Cmd> {
        let Some(b) = &mut self.board else {
            return Vec::new();
        };
        let Some(task_id) = b.picked else {
            self.toast(
                Severity::Info,
                "press Space to pick up the selected task first",
            );
            return Vec::new();
        };
        let Some(task) = b.detail.tasks.iter().find(|t| t.id == task_id).cloned() else {
            return Vec::new();
        };
        let current_col = b
            .detail
            .board
            .columns
            .iter()
            .position(|c| c.id == task.column_id)
            .unwrap_or(0);
        let siblings = b.tasks_in(current_col);
        let idx = siblings.iter().position(|t| t.id == task_id).unwrap_or(0);
        let (to_column, position) = match code {
            KeyCode::Left => match b.column_id(current_col.wrapping_sub(1)) {
                Some(c) => (c, None),
                None => return Vec::new(),
            },
            KeyCode::Right => match b.column_id(current_col + 1) {
                Some(c) => (c, None),
                None => return Vec::new(),
            },
            KeyCode::Up if idx > 0 => (task.column_id, Some(siblings[idx - 1].position - 1)),
            KeyCode::Down if idx + 1 < siblings.len() => {
                (task.column_id, Some(siblings[idx + 1].position + 1))
            }
            _ => return Vec::new(),
        };
        self.request_move(task_id, to_column, position, false)
    }

    /// Move a task, asking its question first when the move finishes a
    /// step that has one and no default. Nothing is sent until the question
    /// is answered, so leaving it unanswered leaves the task where it was.
    fn request_move(
        &mut self,
        task_id: TaskId,
        to_column: ColumnId,
        position: Option<i64>,
        override_deps: bool,
    ) -> Vec<Cmd> {
        let asked = self.board.as_ref().and_then(|b| {
            let task = b.detail.tasks.iter().find(|t| t.id == task_id)?;
            let finishing = to_column == b.detail.board.finished_col
                && task.column_id != b.detail.board.finished_col;
            let q = task.program.as_ref()?.question.as_ref()?;
            finishing.then(|| (task.clone(), q.clone()))
        });
        match asked {
            Some((_, q)) if q.default.is_some() => {
                vec![self.send_move(task_id, to_column, position, override_deps, q.default)]
            }
            Some((task, _)) => {
                self.ask_question(
                    &task,
                    QuestionThen::Move {
                        to_column,
                        position,
                        override_deps,
                    },
                );
                Vec::new()
            }
            None => vec![self.send_move(task_id, to_column, position, override_deps, None)],
        }
    }

    fn send_move(
        &mut self,
        task_id: TaskId,
        to_column: ColumnId,
        position: Option<i64>,
        override_deps: bool,
        answer: Option<String>,
    ) -> Cmd {
        self.send(
            Request::MoveTask {
                task_id,
                to_column,
                position,
                override_deps,
                answer: answer.clone(),
            },
            Pending::MoveTask {
                task: task_id,
                to: to_column,
                answer,
            },
        )
    }

    fn ask_question(&mut self, task: &Task, then: QuestionThen) {
        let Some(question) = task.program.as_ref().and_then(|p| p.question.clone()) else {
            return;
        };
        self.overlay = Some(Overlay::Question {
            task: task.id,
            title: task.title.clone(),
            question,
            sel: 0,
            then,
            areas: Vec::new(),
            area: Rect::default(),
        });
    }

    /// The question popup: Up/Down or a digit picks, Enter answers, Esc
    /// leaves the task exactly where it was.
    fn question_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        let Some(Overlay::Question { question, sel, .. }) = &mut self.overlay else {
            return Vec::new();
        };
        let n = question.answers().len();
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => {
                *sel = sel.saturating_sub(1);
                Vec::new()
            }
            KeyCode::Down | KeyCode::Char('j') => {
                *sel = (*sel + 1).min(n.saturating_sub(1));
                Vec::new()
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let i = c.to_digit(10).unwrap_or(0) as usize;
                if (1..=n).contains(&i) {
                    *sel = i - 1;
                    return self.answer_selected();
                }
                Vec::new()
            }
            KeyCode::Enter => self.answer_selected(),
            _ => {
                self.overlay = None;
                Vec::new()
            }
        }
    }

    fn answer_selected(&mut self) -> Vec<Cmd> {
        let Some(Overlay::Question {
            task,
            question,
            sel,
            then,
            ..
        }) = self.overlay.take()
        else {
            return Vec::new();
        };
        let Some(answer) = question.answers().get(sel).cloned() else {
            return Vec::new();
        };
        match then {
            QuestionThen::Move {
                to_column,
                position,
                override_deps,
            } => vec![self.send_move(task, to_column, position, override_deps, Some(answer))],
            QuestionThen::Answer => vec![self.send(
                Request::AnswerQuestion {
                    task_id: task,
                    answer,
                },
                Pending::AnswerQuestion,
            )],
        }
    }

    fn overlay_key(&mut self, _k: KeyEvent) -> Vec<Cmd> {
        let Some(overlay) = self.overlay.take() else {
            return Vec::new();
        };
        match overlay {
            // Any key closes the ones that are only there to be read.
            Overlay::Help => Vec::new(),
            // Widget overlays and the flow dialogs have their own handlers
            // and never reach this function. Put the overlay back untouched.
            other => {
                self.overlay = Some(other);
                Vec::new()
            }
        }
    }

    fn open_detail(&mut self, task: Task) {
        self.overlay = Some(Overlay::TaskDetail {
            task,
            sel: 0,
            item_areas: Vec::new(),
            area: Rect::default(),
        });
    }

    /// The task viewer: Up/Down walk the checklist, Space or Enter tick the
    /// selected item, anything else closes.
    fn task_detail_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        let Some(Overlay::TaskDetail { task, sel, .. }) = &mut self.overlay else {
            return Vec::new();
        };
        let items = task.checklist.len();
        let pending = task.program.as_ref().filter(|p| p.pending_question);
        if let (KeyCode::Char(c), Some(p)) = (k.code, pending)
            && let Some(i) = c.to_digit(10)
            && let Some(q) = &p.question
            && let Some(answer) = q.answers().get((i as usize).wrapping_sub(1)).cloned()
        {
            let task_id = task.id;
            return vec![self.send(
                Request::AnswerQuestion { task_id, answer },
                Pending::AnswerQuestion,
            )];
        }
        match k.code {
            KeyCode::Up | KeyCode::Char('k') if items > 0 => {
                *sel = sel.saturating_sub(1);
                Vec::new()
            }
            KeyCode::Down | KeyCode::Char('j') if items > 0 => {
                *sel = (*sel + 1).min(items - 1);
                Vec::new()
            }
            KeyCode::Char(' ') | KeyCode::Enter if items > 0 => {
                let (task_id, index) = (task.id, *sel);
                let done = !task.checklist[index].done;
                vec![self.send(
                    Request::SetChecklistItem {
                        task_id,
                        index,
                        done,
                    },
                    Pending::ChecklistOp,
                )]
            }
            _ => {
                self.overlay = None;
                Vec::new()
            }
        }
    }

    /// Open the task form and ask for the member list to fill the
    /// assignee picker. `task` is the one being edited.
    fn open_task_form(&mut self, mode: FormMode, task: Option<&Task>) -> Vec<Cmd> {
        let tz = self
            .user
            .as_ref()
            .map(|u| u.timezone)
            .unwrap_or(chrono_tz::UTC);
        let board_id = match (&mode, task) {
            (FormMode::Create { board, .. }, _) => *board,
            (FormMode::TemplateNew { board } | FormMode::TemplateEdit { board, .. }, _) => *board,
            (FormMode::Edit { .. }, Some(t)) => t.board_id,
            (FormMode::Edit { .. }, None) => return Vec::new(),
        };
        let form = match task {
            Some(t) => {
                let titles: Vec<(TaskId, String)> = self
                    .board
                    .as_ref()
                    .map(|b| {
                        b.detail
                            .tasks
                            .iter()
                            .map(|x| (x.id, x.title.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                TaskForm::edit(t, tz, &|id| {
                    titles
                        .iter()
                        .find(|(i, _)| *i == id)
                        .map(|(_, t)| t.clone())
                })
            }
            None => match mode {
                // A form stamped from a template is built by the templates
                // panel, which has the template to hand; this path only ever
                // opens a blank one.
                FormMode::Create { board, column, .. } => TaskForm::create(board, column, tz),
                FormMode::TemplateNew { board } => TaskForm::template_new(board, tz),
                FormMode::Edit { .. } | FormMode::TemplateEdit { .. } => return Vec::new(),
            },
        };
        self.overlay = Some(Overlay::TaskForm(Box::new(form)));
        vec![self.send(Request::ListMembers { board_id }, Pending::FormMembers)]
    }

    fn form_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::TaskForm(form)) = &mut self.overlay else {
            return Vec::new();
        };
        match form.handle(&ev) {
            FormOutcome::Continue | FormOutcome::Changed => Vec::new(),
            FormOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            FormOutcome::SearchDeps(query) => {
                vec![self.send(
                    Request::Search {
                        query,
                        include_archived: false,
                    },
                    Pending::FormDepSearch,
                )]
            }
            FormOutcome::Save(save) => {
                form.saving = true;
                let mode = form.mode.clone();
                self.save_task(mode, *save)
            }
            FormOutcome::EditPrinting(rules) => {
                let Some(Overlay::TaskForm(back)) = self.overlay.take() else {
                    return Vec::new();
                };
                self.open_print_rules(rules, false, PrintRulesBack::Task(back));
                Vec::new()
            }
        }
    }

    fn open_print_rules(
        &mut self,
        rules: Vec<taskologic_core::print::PrintRule>,
        sheets: bool,
        back: PrintRulesBack,
    ) {
        let me = self.me();
        let names = self.users.clone();
        self.overlay = Some(Overlay::PrintRules {
            form: Box::new(PrintRulesForm::new(rules, me, names, sheets)),
            back,
        });
    }

    fn print_rules_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::PrintRules { form, .. }) = &mut self.overlay else {
            return Vec::new();
        };
        match form.handle(&ev) {
            PrintRulesOutcome::Changed => Vec::new(),
            PrintRulesOutcome::Done(rules) => {
                let Some(Overlay::PrintRules { back, .. }) = self.overlay.take() else {
                    return Vec::new();
                };
                self.overlay = Some(match back {
                    PrintRulesBack::Task(mut form) => {
                        form.set_print_rules(rules);
                        Overlay::TaskForm(form)
                    }
                    PrintRulesBack::Step { mut form, program } => {
                        form.set_print_rules(rules);
                        Overlay::StepForm {
                            form,
                            back: program,
                        }
                    }
                });
                Vec::new()
            }
        }
    }

    fn programs_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Programs(panel)) = &mut self.overlay else {
            return Vec::new();
        };
        let board_id = panel.board_id;
        match panel.handle(&ev) {
            ProgramsOutcome::Changed => Vec::new(),
            ProgramsOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            ProgramsOutcome::New => {
                self.overlay = Some(Overlay::ProgramForm(Box::new(ProgramForm::create(board_id))));
                Vec::new()
            }
            ProgramsOutcome::Edit(program) => {
                self.overlay = Some(Overlay::ProgramForm(Box::new(ProgramForm::edit(&program))));
                Vec::new()
            }
            ProgramsOutcome::Runs => vec![self.send(Request::ListRuns { board_id }, Pending::RunsPanel)],
            ProgramsOutcome::Start(program) => {
                // The root lands in the selected column, like a new task.
                let Some(b) = &self.board else {
                    return Vec::new();
                };
                let Some(column) = b.detail.board.columns.get(b.col) else {
                    return Vec::new();
                };
                let form =
                    StartProgramForm::new(*program, column.id, column.name.clone(), self.timezone());
                self.overlay = Some(Overlay::StartProgram(Box::new(form)));
                Vec::new()
            }
            ProgramsOutcome::PrintCode(program) => {
                let sid = program.short_id;
                self.print_codes_for(
                    &format!("Program {}", program.name),
                    vec![
                        (format!("start {}", program.name), Control::StartProgram(sid)),
                        (format!("start {} now", program.name), Control::StartProgramNow(sid)),
                    ],
                )
            }
            ProgramsOutcome::Delete(program) => {
                let Some(Overlay::Programs(panel)) = self.overlay.take() else {
                    return Vec::new();
                };
                self.confirm(
                    "Delete program",
                    format!(
                        "Delete the program \"{}\"? Runs already going carry on with the steps they started with.",
                        program.name
                    ),
                    ConfirmAction::Send {
                        request: Request::DeleteProgram {
                            program_id: program.id,
                        },
                        pending: Pending::ProgramOp,
                    },
                    Some(Box::new(Overlay::Programs(panel))),
                );
                Vec::new()
            }
        }
    }

    fn program_form_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::ProgramForm(form)) = &mut self.overlay else {
            return Vec::new();
        };
        let board_id = form.board_id;
        match form.handle(&ev) {
            ProgramOutcome::Continue | ProgramOutcome::Changed => Vec::new(),
            ProgramOutcome::Cancel => {
                self.overlay = None;
                vec![self.send(Request::ListPrograms { board_id }, Pending::ProgramsPanel)]
            }
            ProgramOutcome::EditStep { index, step } => {
                let Some(Overlay::ProgramForm(back)) = self.overlay.take() else {
                    return Vec::new();
                };
                let form = StepForm::new(index, &step, back.other_keys(index));
                self.overlay = Some(Overlay::StepForm {
                    form: Box::new(form),
                    back,
                });
                vec![self.send(Request::ListMembers { board_id }, Pending::StepMembers)]
            }
            ProgramOutcome::Save(draft) => {
                form.saving = true;
                let req = match form.program_id {
                    Some(program_id) => Request::UpdateProgram {
                        program_id,
                        draft: *draft,
                    },
                    None => Request::CreateProgram {
                        board_id,
                        draft: *draft,
                    },
                };
                vec![self.send(req, Pending::ProgramOp)]
            }
        }
    }

    fn step_form_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::StepForm { form, .. }) = &mut self.overlay else {
            return Vec::new();
        };
        match form.handle(&ev) {
            StepOutcome::Changed => Vec::new(),
            StepOutcome::Cancel => {
                let Some(Overlay::StepForm { back, .. }) = self.overlay.take() else {
                    return Vec::new();
                };
                self.overlay = Some(Overlay::ProgramForm(back));
                Vec::new()
            }
            StepOutcome::Save(step) => {
                let Some(Overlay::StepForm { form, back }) = self.overlay.take() else {
                    return Vec::new();
                };
                let mut back = back;
                back.set_step(form.index, *step);
                self.overlay = Some(Overlay::ProgramForm(back));
                Vec::new()
            }
            StepOutcome::EditPrinting { rules, sheets } => {
                let Some(Overlay::StepForm { form, back }) = self.overlay.take() else {
                    return Vec::new();
                };
                self.open_print_rules(
                    rules,
                    sheets,
                    PrintRulesBack::Step {
                        form,
                        program: back,
                    },
                );
                Vec::new()
            }
        }
    }

    fn start_program_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::StartProgram(form)) = &mut self.overlay else {
            return Vec::new();
        };
        match form.handle(&ev) {
            StartOutcome::Continue | StartOutcome::Changed => Vec::new(),
            StartOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            StartOutcome::Start {
                program_id,
                column_id,
                start_at,
                fan_out,
            } => {
                form.saving = true;
                vec![self.send(
                    Request::StartProgram {
                        program_id,
                        column_id: Some(column_id),
                        start_at,
                        fan_out,
                    },
                    Pending::StartProgram,
                )]
            }
        }
    }

    fn runs_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Runs(panel)) = &mut self.overlay else {
            return Vec::new();
        };
        match panel.handle(&ev) {
            RunsOutcome::Changed => Vec::new(),
            RunsOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            RunsOutcome::View(task) => {
                self.open_detail(*task);
                Vec::new()
            }
            RunsOutcome::CancelRun {
                run_id,
                delete_open,
            } => {
                let Some(Overlay::Runs(panel)) = self.overlay.take() else {
                    return Vec::new();
                };
                let text = if delete_open {
                    "Cancel this run and move the open tasks it made to the archive?\nFinished ones stay."
                } else {
                    "Cancel this run? The tasks it made stay on the board as ordinary tasks."
                };
                self.confirm(
                    "Cancel run",
                    text,
                    ConfirmAction::Send {
                        request: Request::CancelRun {
                            run_id,
                            delete_open,
                        },
                        pending: Pending::CancelRun,
                    },
                    Some(Box::new(Overlay::Runs(panel))),
                );
                Vec::new()
            }
        }
    }

    fn save_task(&mut self, mode: FormMode, save: TaskSave) -> Vec<Cmd> {
        let TaskSave { draft, options } = save;
        let req = match mode {
            // Stamped from a template: the daemon stamps out the templates
            // this one depends on first and links the new task to them, which
            // it can only do atomically on its own side.
            FormMode::Create {
                column,
                from_template: Some(template),
                ..
            } => Request::CreateFromTemplate {
                template_id: template,
                column_id: Some(column),
                draft,
            },
            FormMode::Create {
                board,
                column,
                from_template: None,
            } => Request::CreateTask {
                board_id: board,
                column_id: Some(column),
                draft,
            },
            FormMode::Edit { task, version } => Request::UpdateTask {
                task_id: task,
                version,
                draft,
            },
            // The template's name is its title; one less field to fill in.
            FormMode::TemplateNew { board } => {
                let name = draft.title.clone();
                return vec![self.send(
                    Request::CreateTemplate {
                        board_id: board,
                        name,
                        draft,
                        options,
                    },
                    Pending::TemplateOp,
                )];
            }
            FormMode::TemplateEdit { template, .. } => {
                let name = draft.title.clone();
                return vec![self.send(
                    Request::UpdateTemplate {
                        template_id: template,
                        name,
                        draft,
                        options,
                    },
                    Pending::TemplateOp,
                )];
            }
        };
        vec![self.send(req, Pending::SaveTask)]
    }

    /// The conflict dialog: `r` reloads the form from the server's version,
    /// `o` overwrites it with what is in the form, Esc goes back to editing
    /// against the current version.
    fn conflict_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        let Some(Overlay::Conflict { mut form, current }) = self.overlay.take() else {
            return Vec::new();
        };
        match k.code {
            KeyCode::Char('r') => {
                let t = *current;
                self.open_task_form(
                    FormMode::Edit {
                        task: t.id,
                        version: t.version,
                    },
                    Some(&t),
                )
            }
            KeyCode::Char('o') => {
                form.set_version(current.version);
                match form.values() {
                    Ok(draft) => {
                        form.saving = true;
                        let mode = form.mode.clone();
                        self.overlay = Some(Overlay::TaskForm(form));
                        self.save_task(mode, draft)
                    }
                    Err(e) => {
                        form.error = Some(e);
                        self.overlay = Some(Overlay::TaskForm(form));
                        Vec::new()
                    }
                }
            }
            _ => {
                form.set_version(current.version);
                self.overlay = Some(Overlay::TaskForm(form));
                Vec::new()
            }
        }
    }

    fn confirm(
        &mut self,
        title: &str,
        text: impl Into<String>,
        action: ConfirmAction,
        back: Option<Box<Overlay>>,
    ) {
        self.overlay = Some(Overlay::Confirm {
            dialog: Box::new(Confirm::new(title, text)),
            action,
            back,
        });
    }

    fn blocked_dialog(
        &mut self,
        task_id: TaskId,
        to: ColumnId,
        open: &[TaskId],
        answer: Option<String>,
    ) {
        let titles: Vec<String> = self
            .board
            .as_ref()
            .map(|b| {
                open.iter()
                    .map(|d| match b.detail.tasks.iter().find(|t| t.id == *d) {
                        Some(t) => t.title.clone(),
                        None => "a task on another board".to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.confirm(
            "Open dependencies",
            format!(
                "Still open:\n{}\nFinish anyway? The override is recorded.",
                titles.join("\n")
            ),
            ConfirmAction::Send {
                request: Request::MoveTask {
                    task_id,
                    to_column: to,
                    position: None,
                    override_deps: true,
                    answer: answer.clone(),
                },
                pending: Pending::MoveTask {
                    task: task_id,
                    to,
                    answer,
                },
            },
            None,
        );
    }

    /// The More menu. Picking an entry presses the key it stands for, so
    /// there is one implementation per action rather than two.
    fn menu_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Menu { items, sel, areas }) = &mut self.overlay else {
            return Vec::new();
        };
        let chosen = match ev {
            TermEvent::Key(k) if k.kind != KeyEventKind::Release => match k.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('M') => {
                    self.overlay = None;
                    return Vec::new();
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    *sel = Some(sel.map_or(items.len().saturating_sub(1), |i| i.saturating_sub(1)));
                    return Vec::new();
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *sel = Some(sel.map_or(0, |i| (i + 1).min(items.len().saturating_sub(1))));
                    return Vec::new();
                }
                KeyCode::Enter => sel.and_then(|i| items.get(i)).map(|(_, key)| *key),
                // Typing an entry's own key works too.
                KeyCode::Char(c) => items.iter().find(|(_, key)| *key == c).map(|(_, key)| *key),
                _ => None,
            },
            TermEvent::Mouse(m) => {
                let at = areas
                    .iter()
                    .position(|a| a.contains(Position::new(m.column, m.row)));
                match m.kind {
                    // Follow the pointer, so the highlight means "this is
                    // what you are about to press".
                    MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                        *sel = at;
                        return Vec::new();
                    }
                    MouseEventKind::Down(MouseButton::Left) => match at {
                        Some(i) => {
                            *sel = Some(i);
                            items.get(i).map(|(_, key)| *key)
                        }
                        None => {
                            // A click outside dismisses it.
                            self.overlay = None;
                            return Vec::new();
                        }
                    },
                    _ => None,
                }
            }
            _ => None,
        };
        match chosen {
            Some(key) => {
                self.overlay = None;
                self.normal_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE))
            }
            None => Vec::new(),
        }
    }

    fn archive_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Archive(panel)) = &mut self.overlay else {
            return Vec::new();
        };
        match panel.handle(&ev) {
            ArchiveOutcome::Changed => Vec::new(),
            ArchiveOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            ArchiveOutcome::Restore(task_id) => {
                vec![self.send(Request::RestoreTask { task_id }, Pending::Restore)]
            }
            ArchiveOutcome::View(task) => {
                self.open_detail(*task);
                Vec::new()
            }
            ArchiveOutcome::Purge(task) => {
                let Some(Overlay::Archive(panel)) = self.overlay.take() else {
                    return Vec::new();
                };
                let (task_id, title) = (task.id, task.title.clone());
                self.confirm(
                    "Delete permanently",
                    format!("Delete \"{title}\" for good? This cannot be undone."),
                    ConfirmAction::Send {
                        request: Request::PurgeTask { task_id },
                        pending: Pending::PurgeTask,
                    },
                    Some(Box::new(Overlay::Archive(panel))),
                );
                Vec::new()
            }
        }
    }

    fn templates_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Templates(panel)) = &mut self.overlay else {
            return Vec::new();
        };
        let tz = self
            .user
            .as_ref()
            .map(|u| u.timezone)
            .unwrap_or(chrono_tz::UTC);
        match panel.handle(&ev) {
            TemplatesOutcome::Changed => Vec::new(),
            TemplatesOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            TemplatesOutcome::New => {
                let board = panel.board_id;
                // Grab the neighbours before opening the form: doing so
                // replaces the overlay, and the panel goes with it.
                let choices = panel.all();
                let mut cmds = self.open_task_form(FormMode::TemplateNew { board }, None);
                if let Some(Overlay::TaskForm(form)) = &mut self.overlay {
                    form.set_template_choices(&choices);
                } else {
                    cmds.clear();
                }
                cmds
            }
            TemplatesOutcome::Edit(tpl) => {
                let choices = panel.all();
                let mut form = TaskForm::template_edit(&tpl, tz);
                form.set_template_choices(&choices);
                self.overlay = Some(Overlay::TaskForm(Box::new(form)));
                vec![self.send(
                    Request::ListMembers {
                        board_id: tpl.board_id,
                    },
                    Pending::FormMembers,
                )]
            }
            TemplatesOutcome::Use(tpl) => {
                // The new task lands in the currently selected column, the
                // same place a plain new task would.
                let Some(b) = &self.board else {
                    return Vec::new();
                };
                let Some(column) = b.detail.board.columns.get(b.col).map(|c| c.id) else {
                    return Vec::new();
                };
                let form = TaskForm::create_from_template(tpl.board_id, column, tz, &tpl);
                self.overlay = Some(Overlay::TaskForm(Box::new(form)));
                vec![self.send(
                    Request::ListMembers {
                        board_id: tpl.board_id,
                    },
                    Pending::FormMembers,
                )]
            }
            TemplatesOutcome::PrintCode(tpl) => {
                let sid = tpl.short_id;
                self.print_codes_for(
                    &format!("Template {}", tpl.name),
                    vec![
                        (format!("new task from {}, in todo", tpl.name), Control::NewFromTemplate(sid, control::ColumnRef::Todo)),
                        (format!("new task from {}, ask first", tpl.name), Control::NewFromTemplateAsk(sid, control::ColumnRef::Todo)),
                    ],
                )
            }
            TemplatesOutcome::Delete(tpl) => {
                let Some(Overlay::Templates(panel)) = self.overlay.take() else {
                    return Vec::new();
                };
                self.confirm(
                    "Delete template",
                    format!(
                        "Delete the template \"{}\"? Tasks made from it stay.",
                        tpl.name
                    ),
                    ConfirmAction::Send {
                        request: Request::DeleteTemplate {
                            template_id: tpl.id,
                        },
                        pending: Pending::TemplateOp,
                    },
                    Some(Box::new(Overlay::Templates(panel))),
                );
                Vec::new()
            }
        }
    }

    fn repeats_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Repeats(panel)) = &mut self.overlay else {
            return Vec::new();
        };
        match panel.handle(&ev) {
            RepeatsOutcome::Changed => Vec::new(),
            RepeatsOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            RepeatsOutcome::Stop(task_id) => {
                vec![self.send(Request::StopRepeat { task_id }, Pending::StopRepeat)]
            }
        }
    }

    /// Open the analytics table. `board` None means every board the user can
    /// see, which is what opening it from the dashboard means.
    fn open_analytics(&mut self, board: Option<BoardId>) -> Vec<Cmd> {
        let scope = match board.and_then(|id| self.board_name(id)) {
            Some(name) => name,
            None => "all boards".to_string(),
        };
        let mut panel = AnalyticsPanel::new(board, scope, self.timezone());
        panel.set_boards(
            self.boards
                .iter()
                .filter(|b| b.is_member)
                .map(|b| (b.id, b.name.clone()))
                .collect(),
        );
        let filter = panel.filter.clone();
        self.analytics = Some(Box::new(panel));
        vec![self.send(
            Request::Analytics {
                board_id: board,
                filter,
            },
            Pending::Analytics,
        )]
    }

    fn analytics_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(panel) = &mut self.analytics else {
            return Vec::new();
        };
        let outcome = panel.handle(&ev);
        self.analytics_outcome(outcome)
    }

    /// A press of one of the analytics buttons in the menu bar.
    fn analytics_menu_key(&mut self, key: char) -> Vec<Cmd> {
        let Some(panel) = &mut self.analytics else {
            return Vec::new();
        };
        let outcome = panel.menu_key(key);
        self.analytics_outcome(outcome)
    }

    fn analytics_outcome(&mut self, outcome: AnalyticsOutcome) -> Vec<Cmd> {
        let Some(panel) = &mut self.analytics else {
            return Vec::new();
        };
        match outcome {
            AnalyticsOutcome::Changed => Vec::new(),
            AnalyticsOutcome::Cancel => {
                self.analytics = None;
                Vec::new()
            }
            AnalyticsOutcome::Reload => {
                let (board_id, filter) = (panel.requested_board(), panel.filter.clone());
                vec![self.send(Request::Analytics { board_id, filter }, Pending::Analytics)]
            }
            AnalyticsOutcome::OpenDetail(task_id) => {
                vec![self.send(
                    Request::TaskHistory { task_id },
                    Pending::TaskHistory { task: task_id },
                )]
            }
            AnalyticsOutcome::SetExcluded(task_id, excluded) => {
                vec![self.send(
                    Request::SetExcludeFromStats { task_id, excluded },
                    Pending::ExcludeFromStats,
                )]
            }
        }
    }

    fn confirm_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Confirm { dialog, .. }) = &mut self.overlay else {
            return Vec::new();
        };
        let answer = dialog.handle(&ev);
        if answer == ConfirmOutcome::Continue || answer == ConfirmOutcome::Changed {
            return Vec::new();
        }
        let Some(Overlay::Confirm { action, back, .. }) = self.overlay.take() else {
            return Vec::new();
        };
        self.overlay = back.map(|b| *b);
        match (answer, action) {
            (ConfirmOutcome::Yes, ConfirmAction::Quit) => vec![Cmd::Quit],
            (ConfirmOutcome::Yes, ConfirmAction::Send { request, pending }) => {
                vec![self.send(request, pending)]
            }
            _ => Vec::new(),
        }
    }

    fn widget_overlay_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        match &self.overlay {
            Some(Overlay::Confirm { .. }) => self.confirm_event(ev),
            Some(Overlay::Menu { .. }) => self.menu_event(ev),
            Some(Overlay::Archive(_)) => self.archive_event(ev),
            Some(Overlay::TaskForm(_)) => self.form_event(ev),
            Some(Overlay::PrintRules { .. }) => self.print_rules_event(ev),
            Some(Overlay::Programs(_)) => self.programs_event(ev),
            Some(Overlay::ProgramForm(_)) => self.program_form_event(ev),
            Some(Overlay::StepForm { .. }) => self.step_form_event(ev),
            Some(Overlay::StartProgram(_)) => self.start_program_event(ev),
            Some(Overlay::Runs(_)) => self.runs_event(ev),
            Some(Overlay::Settings(_)) => self.settings_event(ev),
            Some(Overlay::Colors { .. }) => self.colors_event(ev),
            Some(Overlay::Printer { .. }) => self.printer_event(ev),
            Some(Overlay::Codes { .. }) => self.codes_event(ev),
            Some(Overlay::BoardForm(_)) => self.board_form_event(ev),
            Some(Overlay::Members(_)) => self.members_event(ev),
            Some(Overlay::Columns(_)) => self.columns_event(ev),
            Some(Overlay::Users(_)) => self.users_event(ev),
            Some(Overlay::Templates(_)) => self.templates_event(ev),
            Some(Overlay::Repeats(_)) => self.repeats_event(ev),
            _ => Vec::new(),
        }
    }

    fn open_settings(&mut self) -> Vec<Cmd> {
        let Some(user) = &self.user else {
            return Vec::new();
        };
        let form = SettingsForm::new(user, self.printer.as_ref(), self.mouse_seen);
        self.overlay = Some(Overlay::Settings(Box::new(form)));
        Vec::new()
    }

    fn settings_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Settings(form)) = &mut self.overlay else {
            return Vec::new();
        };
        match form.handle(&ev) {
            SettingsOutcome::Continue | SettingsOutcome::Changed => Vec::new(),
            SettingsOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            SettingsOutcome::EditColors(colors) => {
                let Some(Overlay::Settings(back)) = self.overlay.take() else {
                    return Vec::new();
                };
                self.overlay = Some(Overlay::Colors {
                    form: Box::new(ColorsForm::new(&colors)),
                    back,
                });
                Vec::new()
            }
            SettingsOutcome::PrintCodes => {
                let Some(Overlay::Settings(back)) = self.overlay.take() else {
                    return Vec::new();
                };
                self.overlay = Some(Overlay::Codes {
                    panel: Box::new(CodesPanel::new()),
                    back,
                });
                Vec::new()
            }
            SettingsOutcome::Printer => {
                let Some(Overlay::Settings(back)) = self.overlay.take() else {
                    return Vec::new();
                };
                let form = Box::new(PrinterForm::new(self.printer.as_ref()));
                let mut cmds = vec![Cmd::DetectPrinters];
                // Ask about the configured queue's media too, so an existing
                // profile can show what the printer itself is set up for.
                cmds.extend(form.initial_queue().map(Cmd::DetectMedia));
                self.overlay = Some(Overlay::Printer { form, back });
                cmds
            }
            SettingsOutcome::Save { prefs, timezone } => {
                form.saving = true;
                // Apply locally right away; the daemon answer only confirms it.
                self.scan = scan::detector_for(&prefs.scanner);
                if let Some(u) = &mut self.user {
                    u.prefs = prefs.clone();
                    if let Some(tz) = timezone {
                        u.timezone = tz;
                    }
                }
                let mut cmds = vec![self.send(Request::UpdatePrefs { prefs }, Pending::Settings)];
                if let Some(tz) = timezone {
                    cmds.push(self.send(
                        Request::SetTimezone {
                            timezone: tz.name().to_string(),
                        },
                        Pending::Timezone,
                    ));
                }
                cmds
            }
        }
    }

    fn colors_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Colors { form, .. }) = &mut self.overlay else {
            return Vec::new();
        };
        let answer = form.handle(&ev);
        match answer {
            ColorsOutcome::Changed => Vec::new(),
            ColorsOutcome::Cancel | ColorsOutcome::Save(_) => {
                let Some(Overlay::Colors { back, .. }) = self.overlay.take() else {
                    return Vec::new();
                };
                let mut back = back;
                if let ColorsOutcome::Save(colors) = answer {
                    back.set_colors(*colors);
                }
                self.overlay = Some(Overlay::Settings(back));
                Vec::new()
            }
        }
    }

    fn codes_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Codes { panel, .. }) = &mut self.overlay else {
            return Vec::new();
        };
        match panel.handle(&ev) {
            CodesOutcome::Changed => Vec::new(),
            CodesOutcome::Cancel => {
                let Some(Overlay::Codes { back, .. }) = self.overlay.take() else {
                    return Vec::new();
                };
                self.overlay = Some(Overlay::Settings(back));
                Vec::new()
            }
            CodesOutcome::Print { heading, codes } => self.print_codes(&heading, codes),
        }
    }

    /// A codes card to this client's printer, built here: no task, no
    /// daemon. Each code picks CODE39 or CODE128 for the paper it goes on,
    /// and one too long for the paper even so is split over several.
    fn print_codes(&mut self, heading: &str, codes: Vec<(String, Vec<Control>)>) -> Vec<Cmd> {
        let Some(profile) = self.printer.clone() else {
            self.toast(Severity::Warning, "no printer on this client; set one up first");
            return vec![self.bell()];
        };
        let Some(user) = self.user.clone() else {
            return Vec::new();
        };
        let dots = profile.dots();
        let mut lines = Vec::new();
        for (label, commands) in codes {
            let symbology = control::symbology_for(&commands, dots);
            let payload = control::encode(&commands);
            let pieces = control::chunks(&payload, dots);
            let n = pieces.len();
            let is_ping = commands == [Control::Ping];
            for (i, piece) in pieces.into_iter().enumerate() {
                let label = if n == 1 {
                    label.clone()
                } else {
                    format!("{label} ({} of {n})", i + 1)
                };
                lines.push(taskologic_core::print::CodeLine {
                    label: label.clone(),
                    payload: piece.clone(),
                    symbology,
                    narrow: false,
                });
                // The scanner check is a test strip: the same code with
                // thin bars under it says what the scanner still reads.
                if is_ping {
                    lines.push(taskologic_core::print::CodeLine {
                        label: format!("{label}, thin bars"),
                        payload: piece,
                        symbology,
                        narrow: true,
                    });
                }
            }
        }
        let job = taskologic_core::print::build_codes_job(heading, lines, &user, chrono::Utc::now());
        self.local_print = "codes card";
        vec![Cmd::TestPrint { job, profile }]
    }

    fn printer_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Printer { form, .. }) = &mut self.overlay else {
            return Vec::new();
        };
        match form.handle(&ev) {
            PrinterOutcome::Changed => Vec::new(),
            PrinterOutcome::QueueChanged(queue) => vec![Cmd::DetectMedia(queue)],
            PrinterOutcome::Refresh => vec![Cmd::DetectPrinters],
            PrinterOutcome::TestPrint(profile) => match self.test_print_job() {
                Some(job) => {
                    self.local_print = "test print";
                    vec![Cmd::TestPrint {
                        job,
                        profile: *profile,
                    }]
                }
                None => Vec::new(),
            },
            PrinterOutcome::Cancel => {
                let Some(Overlay::Printer { back, .. }) = self.overlay.take() else {
                    return Vec::new();
                };
                self.overlay = Some(Overlay::Settings(back));
                Vec::new()
            }
            PrinterOutcome::Save(profile) => {
                let Some(Overlay::Printer { back, .. }) = self.overlay.take() else {
                    return Vec::new();
                };
                let mut back = back;
                back.set_printer(profile.as_ref());
                self.overlay = Some(Overlay::Settings(back));
                if profile != self.printer {
                    self.printer = profile.clone();
                    vec![Cmd::SavePrinter(profile)]
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// A slip that exercises every field of the local printer profile.
    fn test_print_job(&self) -> Option<PrintJob> {
        let user = self.user.as_ref()?;
        let now = chrono::Utc::now();
        let board = Board {
            id: taskologic_core::ids::BoardId(0),
            short_id: taskologic_core::ids::ShortId::from_index(0),
            name: "Taskologic".into(),
            description: String::new(),
            owner_uid: user.uid,
            is_locked: false,
            is_private: false,
            archive_after_secs: 0,
            purge_deleted_after_secs: 0,
            card_fields: CardFields::default(),
            started_col: ColumnId(2),
            paused_col: ColumnId(3),
            finished_col: ColumnId(4),
            created_at: now,
            columns: Vec::new(),
            members: vec![user.uid],
        };
        let task = Task {
            id: TaskId(0),
            short_id: taskologic_core::ids::ShortId::from_index(0),
            board_id: board.id,
            column_id: ColumnId(1),
            position: 0,
            title: "Test print".into(),
            description: "If you can read this, the printer profile works. Umlauts: äöü ß".into(),
            start_at: Some(now),
            due_at: Some(now + chrono::TimeDelta::hours(1)),
            reminder_start_minutes: None,
            reminder_due_minutes: None,
            created_by: user.uid,
            created_at: now,
            finished_at: None,
            version: 1,
            archived_at: None,
            archived_from_col: None,
            deleted_at: None,
            assignees: vec![user.uid],
            depends_on: Vec::new(),
            checklist: Vec::new(),
            repeat: None,
            template_id: None,
            exclude_from_stats: false,
            print_rules: Vec::new(),
            program: None,
            auto_start: false,
        };
        let name = user.username.clone();
        let mut job = taskologic_core::print::build_task_job(
            &task,
            &board,
            &[],
            &|_| name.clone(),
            user,
            now,
        );
        // A test print is about the printer, not the prefs: put a barcode on
        // the slip whatever the scanner settings say, so there is something to
        // hold a scanner up to. Text output cannot draw one and prints the
        // payload instead, which still says whether it got that far.
        // One per action, so whichever barcode the slip layout shows carries
        // the sample rather than a code that means something.
        let sym = user.prefs.scanner.format;
        job.barcodes = vec![
            taskologic_core::print::sample_barcode(sym, ScanAction::StartPause),
            taskologic_core::print::sample_barcode(sym, ScanAction::Finish),
        ];
        Some(job)
    }

    fn open_board_form_create(&mut self) -> Vec<Cmd> {
        let me = self.me();
        self.overlay = Some(Overlay::BoardForm(Box::new(BoardForm::create(me))));
        vec![self.send(Request::ListUsers, Pending::FormUsers)]
    }

    fn open_board_settings(&mut self) -> Vec<Cmd> {
        if !self.privileged_on_open_board() {
            self.toast(
                Severity::Info,
                "only the board owner or an admin can change board settings",
            );
            return Vec::new();
        }
        let me = self.me();
        let Some(b) = &self.board else {
            return Vec::new();
        };
        let form = BoardForm::edit(&b.detail.board, me);
        self.overlay = Some(Overlay::BoardForm(Box::new(form)));
        Vec::new()
    }

    fn board_form_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::BoardForm(form)) = &mut self.overlay else {
            return Vec::new();
        };
        match form.handle(&ev) {
            BoardOutcome::Continue | BoardOutcome::Changed => Vec::new(),
            BoardOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            BoardOutcome::OpenMembers => self.open_members(),
            BoardOutcome::OpenColumns => self.open_columns(),
            BoardOutcome::PrintCode => {
                let Some(board) = self.board.as_ref().map(|b| b.detail.board.clone()) else {
                    return Vec::new();
                };
                let sid = board.short_id;
                self.print_codes_for(
                    &format!("Board {}", board.name),
                    vec![
                        (format!("show board {}", board.name), Control::ShowBoard(sid)),
                        (format!("analytics of {}", board.name), Control::Analytics(sid)),
                    ],
                )
            }
            BoardOutcome::Create(req) => {
                form.saving = true;
                vec![self.send(Request::CreateBoard(*req), Pending::CreateBoardForm)]
            }
            BoardOutcome::Update(req) => {
                let req = *req;
                // Going private keeps everyone with a task on as a member. Say
                // who that is before it happens.
                if req.is_private == Some(true)
                    && let Some(b) = &self.board
                {
                    let mut assigned: Vec<Uid> = b
                        .detail
                        .tasks
                        .iter()
                        .filter(|t| !t.is_archived())
                        .flat_map(|t| t.assignees.iter().copied())
                        .collect();
                    assigned.sort_unstable();
                    assigned.dedup();
                    let carry: Vec<String> = b
                        .detail
                        .board
                        .would_lose_access(&assigned)
                        .into_iter()
                        .map(|u| self.user_name(u))
                        .collect();
                    if !carry.is_empty() {
                        self.confirm(
                            "Going private",
                            format!(
                                "These people have tasks here and stay on as members:\n{}\nContinue?",
                                carry.join(", ")
                            ),
                            ConfirmAction::Send { request: Request::UpdateBoard(req), pending: Pending::BoardOp },
                            None,
                        );
                        return Vec::new();
                    }
                }
                if let Some(Overlay::BoardForm(form)) = &mut self.overlay {
                    form.saving = true;
                }
                vec![self.send(Request::UpdateBoard(req), Pending::BoardOp)]
            }
        }
    }

    fn open_members(&mut self) -> Vec<Cmd> {
        let privileged = self.privileged_on_open_board();
        let Some(b) = &self.board else {
            return Vec::new();
        };
        let board_id = b.detail.board.id;
        let panel = MembersPanel::new(&b.detail.board, privileged);
        self.overlay = Some(Overlay::Members(Box::new(panel)));
        vec![
            self.send(Request::ListMembers { board_id }, Pending::PanelMembers),
            self.send(Request::ListUsers, Pending::PanelUsers),
        ]
    }

    fn members_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Members(panel)) = &mut self.overlay else {
            return Vec::new();
        };
        let board_id = panel.board_id;
        match panel.handle(&ev) {
            MembersOutcome::Changed => Vec::new(),
            MembersOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            MembersOutcome::Add(uid) => vec![self.send(
                Request::AddMember { board_id, uid },
                Pending::MemberOp { uid },
            )],
            MembersOutcome::Remove(uid) => {
                vec![self.send(
                    Request::RemoveMember {
                        board_id,
                        uid,
                        unassign: false,
                    },
                    Pending::MemberOp { uid },
                )]
            }
        }
    }

    fn open_columns(&mut self) -> Vec<Cmd> {
        if !self.privileged_on_open_board() {
            self.toast(
                Severity::Info,
                "only the board owner or an admin can change columns",
            );
            return Vec::new();
        }
        let Some(b) = &self.board else {
            return Vec::new();
        };
        self.overlay = Some(Overlay::Columns(Box::new(ColumnsPanel::new(
            &b.detail.board,
        ))));
        Vec::new()
    }

    fn columns_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Columns(panel)) = &mut self.overlay else {
            return Vec::new();
        };
        let board_id = panel.board.id;
        match panel.handle(&ev) {
            ColumnsOutcome::Changed => Vec::new(),
            ColumnsOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            ColumnsOutcome::Add(name) => {
                vec![self.send(Request::AddColumn { board_id, name }, Pending::ColumnOp)]
            }
            ColumnsOutcome::Rename(column_id, name) => {
                vec![self.send(Request::RenameColumn { column_id, name }, Pending::ColumnOp)]
            }
            ColumnsOutcome::Reorder(order) => vec![self.send(
                Request::ReorderColumns { board_id, order },
                Pending::ColumnOp,
            )],
            ColumnsOutcome::SetRole(role, id) => {
                let req = taskologic_proto::UpdateBoard {
                    board_id,
                    name: None,
                    description: None,
                    is_locked: None,
                    is_private: None,
                    archive_after_secs: None,
                    purge_deleted_after_secs: None,
                    card_fields: None,
                    roles: vec![(role, id)],
                };
                vec![self.send(Request::UpdateBoard(req), Pending::ColumnOp)]
            }
            ColumnsOutcome::Remove(id) => self.start_remove_flow(id),
        }
    }

    fn start_remove_flow(&mut self, column: ColumnId) -> Vec<Cmd> {
        let Some(Overlay::Columns(panel)) = self.overlay.take() else {
            return Vec::new();
        };
        let has_tasks = self.board.as_ref().is_some_and(|b| {
            b.detail
                .tasks
                .iter()
                .any(|t| t.column_id == column && !t.is_archived())
        });
        let roles = panel.board.roles_of(column);
        let flow = RemoveFlow {
            column,
            has_tasks,
            destination: None,
            roles,
            replacements: Vec::new(),
        };
        self.advance_remove_flow(panel, flow)
    }

    /// Ask the next open question, or confirm once everything is answered.
    fn advance_remove_flow(&mut self, panel: Box<ColumnsPanel>, flow: RemoveFlow) -> Vec<Cmd> {
        let name = panel
            .board
            .column(flow.column)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let options: Vec<(ColumnId, String)> = panel
            .board
            .columns
            .iter()
            .filter(|c| c.id != flow.column)
            .map(|c| (c.id, c.name.clone()))
            .collect();
        if flow.has_tasks && flow.destination.is_none() {
            let title = format!("Where should the tasks in \"{name}\" go?");
            self.overlay = Some(Overlay::PickColumn {
                panel,
                title,
                options,
                sel: 0,
                flow,
            });
            return Vec::new();
        }
        if let Some(role) = flow.next_role() {
            let title = format!(
                "\"{name}\" is the {} column. Which column takes over?",
                role.label()
            );
            self.overlay = Some(Overlay::PickColumn {
                panel,
                title,
                options,
                sel: 0,
                flow,
            });
            return Vec::new();
        }
        let request = Request::RemoveColumn {
            column_id: flow.column,
            move_tasks_to: flow.destination,
            replacements: flow.replacements.clone(),
        };
        self.confirm(
            "Remove column",
            format!("Remove the column \"{name}\"?"),
            ConfirmAction::Send {
                request,
                pending: Pending::ColumnOp,
            },
            Some(Box::new(Overlay::Columns(panel))),
        );
        Vec::new()
    }

    fn pick_column_key(&mut self, k: KeyEvent) -> Vec<Cmd> {
        let Some(Overlay::PickColumn {
            panel,
            title,
            options,
            mut sel,
            mut flow,
        }) = self.overlay.take()
        else {
            return Vec::new();
        };
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => sel = sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                sel = (sel + 1).min(options.len().saturating_sub(1))
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.overlay = Some(Overlay::Columns(panel));
                return Vec::new();
            }
            KeyCode::Enter => {
                if let Some((id, _)) = options.get(sel) {
                    if flow.has_tasks && flow.destination.is_none() {
                        flow.destination = Some(*id);
                    } else if let Some(role) = flow.next_role() {
                        flow.replacements.push((role, *id));
                    }
                }
                return self.advance_remove_flow(panel, flow);
            }
            _ => {}
        }
        self.overlay = Some(Overlay::PickColumn {
            panel,
            title,
            options,
            sel,
            flow,
        });
        Vec::new()
    }

    fn open_users(&mut self) -> Vec<Cmd> {
        if !self.user.as_ref().is_some_and(|u| u.is_admin) {
            self.toast(Severity::Info, "only admins can manage users");
            return Vec::new();
        }
        let me = self.me();
        self.overlay = Some(Overlay::Users(Box::new(UsersPanel::new(me))));
        vec![self.send(Request::ListUsers, Pending::UsersPanel)]
    }

    fn users_event(&mut self, ev: TermEvent) -> Vec<Cmd> {
        let Some(Overlay::Users(panel)) = &mut self.overlay else {
            return Vec::new();
        };
        match panel.handle(&ev) {
            UsersOutcome::Changed => Vec::new(),
            UsersOutcome::Cancel => {
                self.overlay = None;
                Vec::new()
            }
            UsersOutcome::SetAdmin { uid, is_admin } => {
                vec![self.send(Request::SetAdmin { uid, is_admin }, Pending::AdminOp)]
            }
        }
    }

    /// The board changed underneath an open panel or form.
    fn refresh_panels(&mut self, board: &Board) {
        match &mut self.overlay {
            Some(Overlay::Columns(p)) if p.board.id == board.id => p.refresh(board),
            Some(Overlay::BoardForm(f)) if f.board_id() == Some(board.id) => f.refresh(board),
            _ => {}
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) -> Vec<Cmd> {
        if self.too_small() || self.conn != Conn::Ready {
            return Vec::new();
        }
        if self.is_widget_overlay() {
            return self.widget_overlay_event(TermEvent::Mouse(m));
        }
        if let Some(overlay) = &self.overlay {
            // The task viewer answers clicks: an item toggles, outside closes.
            if let Overlay::TaskDetail {
                task,
                item_areas,
                area,
                ..
            } = overlay
            {
                if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                    let pos = Position::new(m.column, m.row);
                    if let Some(i) = item_areas.iter().position(|a| a.contains(pos)) {
                        let task_id = task.id;
                        let done = !task.checklist.get(i).map(|c| c.done).unwrap_or(true);
                        if let Some(Overlay::TaskDetail { sel, .. }) = &mut self.overlay {
                            *sel = i;
                        }
                        return vec![self.send(
                            Request::SetChecklistItem {
                                task_id,
                                index: i,
                                done,
                            },
                            Pending::ChecklistOp,
                        )];
                    }
                    if !area.contains(pos) {
                        self.overlay = None;
                    }
                }
                return Vec::new();
            }
            if let Overlay::Question { areas, area, .. } = overlay
                && let MouseEventKind::Down(MouseButton::Left) = m.kind
            {
                let pos = Position::new(m.column, m.row);
                if let Some(i) = areas.iter().position(|a| a.contains(pos)) {
                    if let Some(Overlay::Question { sel, .. }) = &mut self.overlay {
                        *sel = i;
                    }
                    return self.answer_selected();
                }
                if !area.contains(pos) {
                    self.overlay = None;
                }
                return Vec::new();
            }
            // Overlays that are only there to be read close on any click.
            let dismissable = matches!(overlay, Overlay::Help | Overlay::PickColumn { .. });
            if dismissable && matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.overlay = None;
            }
            return Vec::new();
        }
        let (x, y) = (m.column, m.row);
        if let MouseEventKind::Down(MouseButton::Left) = m.kind
            && let Some((_, key)) = self
                .menu_areas
                .iter()
                .find(|(a, _)| a.contains(Position::new(x, y)))
        {
            let key = *key;
            // The analytics buttons are its own, and 'M' still opens More.
            if self.analytics.is_some() && key != 'M' {
                return self.analytics_menu_key(key);
            }
            return self.normal_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
        }
        // Everything else on the analytics screen belongs to the panel.
        if self.analytics.is_some() {
            return self.analytics_event(TermEvent::Mouse(m));
        }
        if let MouseEventKind::Down(MouseButton::Left) = m.kind
            && let Some((_, id)) = self
                .tab_areas
                .iter()
                .find(|(a, _)| a.contains(Position::new(x, y)))
        {
            let board_id = *id;
            if self
                .board
                .as_ref()
                .is_some_and(|b| b.detail.board.id == board_id)
            {
                return Vec::new();
            }
            return vec![self.send(
                Request::GetBoard { board_id },
                Pending::OpenBoard { focus_task: None },
            )];
        }
        if self.board.is_some() {
            return self.board_mouse(m);
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.include_archived.handle(&TermEvent::Mouse(m), Regular);
                if self.search_area.contains(Position::new(x, y)) {
                    self.search.focus().set(true);
                    return Vec::new();
                }
                self.search.focus().set(false);
                if let Some(i) = self
                    .hit_areas
                    .iter()
                    .position(|a| a.contains(Position::new(x, y)))
                {
                    self.hit_sel = i;
                    return self.dashboard_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                }
                if let Some(i) = self
                    .board_areas
                    .iter()
                    .position(|a| a.contains(Position::new(x, y)))
                {
                    self.board_sel = i;
                    return self.open_selected_board();
                }
                Vec::new()
            }
            MouseEventKind::ScrollUp => {
                self.dashboard_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
            }
            MouseEventKind::ScrollDown => {
                self.dashboard_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            }
            _ => Vec::new(),
        }
    }

    fn board_mouse(&mut self, m: MouseEvent) -> Vec<Cmd> {
        let Some(b) = &mut self.board else {
            return Vec::new();
        };
        let (x, y) = (m.column, m.row);
        // MouseFlags remembers whether the button went down inside the board,
        // so a drag that wanders outside still counts.
        let dragging = b.mouse.drag(b.area, &m);
        // Double click on a card opens it, before the drop logic below eats
        // the second Up event.
        if let Some((col, row, id)) = b.task_at(x, y) {
            let rect = b
                .task_areas
                .get(col - b.col_offset)
                .and_then(|slot| slot.iter().find(|(t, _)| *t == id))
                .map(|(_, r)| *r);
            if let Some(rect) = rect
                && b.mouse.doubleclick(rect, &m)
            {
                b.drag = None;
                b.hover_col = None;
                b.col = col;
                b.row = row;
                if let Some(t) = b.selected_task().cloned() {
                    self.open_detail(t);
                }
                return Vec::new();
            }
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Buttons drawn on the column frames come first.
                if b.left_arrow
                    .is_some_and(|a| a.contains(Position::new(x, y)))
                {
                    b.col = b.col.saturating_sub(1);
                    b.clamp();
                    return Vec::new();
                }
                if b.right_arrow
                    .is_some_and(|a| a.contains(Position::new(x, y)))
                {
                    b.col = (b.col + 1).min(b.column_count().saturating_sub(1));
                    b.clamp();
                    return Vec::new();
                }
                if let Some(idx) = BoardView::hit(&b.up_areas, x, y) {
                    if let Some(s) = b.col_scroll.get_mut(idx) {
                        *s = s.saturating_sub(1);
                    }
                    return Vec::new();
                }
                if let Some(idx) = BoardView::hit(&b.down_areas, x, y) {
                    if let Some(s) = b.col_scroll.get_mut(idx) {
                        *s += 1;
                    }
                    return Vec::new();
                }
                if let Some(idx) = BoardView::hit(&b.sort_areas, x, y) {
                    if let Some(col) = b.detail.board.columns.get(idx) {
                        let (column_id, sort_by_due) = (col.id, !col.sort_by_due);
                        return vec![self.send(
                            Request::SetColumnSort {
                                column_id,
                                sort_by_due,
                            },
                            Pending::ColumnOp,
                        )];
                    }
                    return Vec::new();
                }
                if let Some(idx) = BoardView::hit(&b.plus_areas, x, y) {
                    b.col = idx;
                    b.clamp();
                    if let Some(column) = b.column_id(idx) {
                        let board = b.detail.board.id;
                        return self.open_task_form(
                            FormMode::Create {
                                board,
                                column,
                                from_template: None,
                            },
                            None,
                        );
                    }
                    return Vec::new();
                }
                if let Some((col, row, id)) = b.task_at(x, y) {
                    b.col = col;
                    b.row = row;
                    b.drag = Some((id, col));
                } else if let Some(col) = b.column_at(x, y) {
                    b.col = col;
                    b.clamp();
                }
                Vec::new()
            }
            MouseEventKind::Drag(MouseButton::Left) if dragging => {
                b.hover_col = b.column_at(x, y);
                Vec::new()
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let drop = b.drag.take();
                let hover = b.column_at(x, y).or_else(|| b.hover_col.take());
                b.hover_col = None;
                match (drop, hover) {
                    (Some((task_id, from)), Some(to)) => {
                        // Same column means a reorder, so work out where in
                        // the list the card was let go.
                        let position = b.drop_position(to, y, task_id);
                        if to == from && position.is_none() {
                            return Vec::new();
                        }
                        match b.column_id(to) {
                            Some(to_column) => {
                                self.request_move(task_id, to_column, position, false)
                            }
                            None => Vec::new(),
                        }
                    }
                    _ => Vec::new(),
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if let Some(col) = b.column_at(x, y) {
                    b.col = col;
                }
                if m.kind == MouseEventKind::ScrollUp {
                    b.row = b.row.saturating_sub(1);
                } else {
                    b.row += 1;
                }
                b.clamp();
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use taskologic_core::board::test_support::board_with_members;
    use taskologic_core::ids::BoardId;
    use taskologic_core::task::test_support::task_on;
    use taskologic_core::user::User;
    use taskologic_proto::Welcome;

    pub fn user(scanner: bool) -> User {
        let mut prefs = taskologic_core::prefs::UserPrefs::default();
        prefs.ui.scanner_enabled = scanner;
        User {
            uid: 1,
            username: "alice".into(),
            is_admin: true,
            timezone: chrono_tz::UTC,
            prefs,
            has_pin: false,
            created_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        }
    }

    pub fn ready_app(scanner: bool) -> App {
        // Full colour and unicode, so the snapshots show the real thing.
        let mut app = App::new(false, ColorMode::Full, None);
        app.size = (80, 24);
        let cmds = app.start(Hello {
            protocol_version: 1,
            client_version: "test".into(),
            has_printer: false,
            print_priority: 0,
            capabilities: vec![],
        });
        let Cmd::Send(hello) = &cmds[0] else { panic!() };
        let welcome = Welcome {
            server_version: "test".into(),
            protocol_version: 1,
            user: user(scanner),
            boards: vec![
                BoardSummary {
                    id: BoardId(1),
                    name: "Kitchen".into(),
                    description: "Shopping and chores".into(),
                    owner_uid: 1,
                    is_locked: false,
                    is_private: false,
                    is_member: true,
                    task_count: 3,
                },
                BoardSummary {
                    id: BoardId(2),
                    name: "Secret plans".into(),
                    description: String::new(),
                    owner_uid: 2,
                    is_locked: true,
                    is_private: true,
                    is_member: true,
                    task_count: 0,
                },
            ],
        };
        app.update(server(ServerMessage::Ok {
            id: hello.id,
            response: Response::Welcome(welcome),
        }));
        app
    }

    pub fn open_board(app: &mut App) {
        let board = board_with_members(1, &[1, 2]);
        let mut tasks = Vec::new();
        for (i, (title, col)) in [
            ("Water the plants", 1),
            ("Buy soap", 1),
            ("Dishes", 2),
            ("Taxes", 4),
        ]
        .iter()
        .enumerate()
        {
            let mut t = task_on(&board, 1);
            t.id = TaskId(10 + i as i64);
            t.short_id = taskologic_core::ids::ShortId::from_index(1000 + i as u64);
            t.title = title.to_string();
            t.column_id = ColumnId(*col);
            t.position = i as i64 * 10;
            tasks.push(t);
        }
        let cmds = app.update(Msg::Term(TermEvent::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        ))));
        let Some(Cmd::Send(req)) = cmds.first() else {
            panic!("{cmds:?}")
        };
        app.update(server(ServerMessage::Ok {
            id: req.id,
            response: Response::Board(BoardDetail {
                board,
                tasks,
                estimates: Vec::new(),
            }),
        }));
    }

    pub fn press(app: &mut App, code: KeyCode) -> Vec<Cmd> {
        app.update(Msg::Term(TermEvent::Key(KeyEvent::new(
            code,
            KeyModifiers::NONE,
        ))))
    }

    pub fn server(m: ServerMessage) -> Msg {
        Msg::Server(Box::new(m))
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use taskologic_core::barcode::{ScanAction, ScanPayload};
    use taskologic_core::board::test_support::board_with_members;
    use taskologic_core::ids::{BoardId, ShortId};
    use taskologic_core::task::test_support::task_on;

    /// A column taller than the screen used to hand clicks the wrong task:
    /// the hit test counted rows from the top of the drawn cards but the
    /// drawn cards start at the scroll offset, so every click landed that
    /// many places too high.
    #[test]
    fn clicking_a_card_in_a_scrolled_column_selects_the_card_under_the_pointer() {
        let mut app = ready_app(false);
        let board = board_with_members(1, &[1, 2]);
        let tasks: Vec<Task> = (0..12)
            .map(|i| {
                let mut t = task_on(&board, 1);
                t.id = TaskId(100 + i);
                t.short_id = ShortId::from_index(2000 + i as u64);
                t.title = format!("Task {i}");
                t.column_id = ColumnId(1);
                t.position = i * 10;
                t
            })
            .collect();
        let cmds = press(&mut app, KeyCode::Enter);
        let Some(Cmd::Send(req)) = cmds.first() else {
            panic!("{cmds:?}")
        };
        app.update(server(ServerMessage::Ok {
            id: req.id,
            response: Response::Board(BoardDetail {
                board,
                tasks,
                estimates: Vec::new(),
            }),
        }));

        // The view is what records where the cards landed, so it has to run.
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        term.draw(|f| crate::ui::view(&mut app, f)).unwrap();
        // Walk to the bottom of the column, dragging the view down with it.
        for _ in 0..11 {
            press(&mut app, KeyCode::Down);
        }
        term.draw(|f| crate::ui::view(&mut app, f)).unwrap();

        let b = app.board.as_ref().unwrap();
        assert!(b.col_scroll[0] > 0, "the column should have scrolled");
        let (want, rect) = b.task_areas[0][0];
        assert_ne!(want, TaskId(100), "the first card has scrolled off");

        app.update(Msg::Term(TermEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 1,
            row: rect.y + 1,
            modifiers: KeyModifiers::NONE,
        })));
        assert_eq!(
            app.board.as_ref().unwrap().selected_task().map(|t| t.id),
            Some(want),
            "a click must select the card under the pointer"
        );
    }

    fn payload() -> String {
        ScanPayload {
            action: ScanAction::StartPause,
            short_id: ShortId::parse("K4M9Q2").unwrap(),
        }
        .encode()
    }

    #[test]
    fn the_task_viewer_ticks_checklist_items() {
        let mut app = ready_app(false);
        open_board(&mut app);
        {
            let b = app.board.as_mut().unwrap();
            b.detail.tasks[0].checklist = vec![
                taskologic_core::task::ChecklistItem {
                    text: "soil".into(),
                    done: false,
                },
                taskologic_core::task::ChecklistItem {
                    text: "water".into(),
                    done: false,
                },
            ];
        }
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.overlay, Some(Overlay::TaskDetail { .. })));
        press(&mut app, KeyCode::Down);
        let cmds = press(&mut app, KeyCode::Char(' '));
        let Cmd::Send(req) = &cmds[0] else {
            panic!("{cmds:?}")
        };
        assert!(matches!(
            req.request,
            Request::SetChecklistItem {
                task_id: TaskId(10),
                index: 1,
                done: true
            }
        ));
        // The answer updates the open viewer in place.
        let mut task = app.board.as_ref().unwrap().detail.tasks[0].clone();
        task.checklist[1].done = true;
        task.version += 1;
        app.update(server(ServerMessage::Ok {
            id: req.id,
            response: Response::Task { task },
        }));
        match &app.overlay {
            Some(Overlay::TaskDetail { task, .. }) => assert!(task.checklist[1].done),
            other => panic!("viewer should stay open, got {:?}", other.is_some()),
        }
        // A key that is not part of the checklist closes it.
        press(&mut app, KeyCode::Esc);
        assert!(app.overlay.is_none());
    }

    #[test]
    fn quit_asks_first() {
        let mut app = ready_app(false);
        assert!(press(&mut app, KeyCode::Char('q')).is_empty());
        assert!(matches!(
            app.overlay,
            Some(Overlay::Confirm {
                action: ConfirmAction::Quit,
                ..
            })
        ));
        assert!(press(&mut app, KeyCode::Char('n')).is_empty());
        assert!(app.overlay.is_none());
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(press(&mut app, KeyCode::Char('y')), vec![Cmd::Quit]);
    }

    #[test]
    fn injected_scan_becomes_a_scan_request() {
        let mut app = ready_app(true);
        let mut cmds = Vec::new();
        for ev in scan::inject(&payload(), false) {
            cmds.extend(app.update(Msg::Term(ev)));
        }
        assert_eq!(cmds.len(), 1, "{cmds:?}");
        match &cmds[0] {
            Cmd::Send(ClientMessage {
                request: Request::Scan { payload: p },
                ..
            }) => assert_eq!(p, &payload()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn scanner_off_means_keys_are_just_keys() {
        let mut app = ready_app(false);
        open_board(&mut app);
        let mut cmds = Vec::new();
        // No 'S' in the payload: with the scanner off these are ordinary
        // keypresses, and 'S' would simply open the settings screen.
        for ev in scan::inject("..1K", false) {
            cmds.extend(app.update(Msg::Term(ev)));
        }
        assert!(cmds.is_empty());
        assert!(app.overlay.is_none());
        // Plain keys still do their normal job, and the scan key explains itself.
        assert!(press(&mut app, KeyCode::Char('s')).is_empty());
        assert!(
            app.toast
                .as_ref()
                .is_some_and(|t| t.text.contains("off in your settings"))
        );
    }

    fn inject_all(app: &mut App, text: &str) -> Vec<Cmd> {
        let mut cmds = Vec::new();
        for ev in scan::inject(text, false) {
            cmds.extend(app.update(Msg::Term(ev)));
        }
        cmds
    }

    #[test]
    fn a_control_code_is_read_on_any_screen_and_two_dashes_from_a_person_are_typing() {
        let mut app = ready_app(true);
        // From the dashboard, with nothing open.
        let cmds = inject_all(&mut app, "--1PING--");
        assert!(cmds.is_empty(), "{cmds:?}");
        assert_eq!(app.toast.as_ref().map(|t| t.text.as_str()), Some("scanner ok"));
        // Under the help overlay just the same.
        press(&mut app, KeyCode::Char('?'));
        app.toast = None;
        inject_all(&mut app, "--1PING--");
        assert_eq!(app.toast.as_ref().map(|t| t.text.as_str()), Some("scanner ok"));
        press(&mut app, KeyCode::Esc);
        // Two dashes and a letter in the search box are what was typed.
        press(&mut app, KeyCode::Char('/'));
        inject_all(&mut app, "--x");
        assert_eq!(app.search.text(), "--x");
    }

    #[test]
    fn insert_and_search_codes_type_and_search() {
        let mut app = ready_app(true);
        press(&mut app, KeyCode::Char('/'));
        inject_all(&mut app, "--1I/V.hi there--");
        assert_eq!(app.search.text(), "hi there");
        press(&mut app, KeyCode::Esc);
        // Insert with nothing to type into says so.
        let cmds = inject_all(&mut app, "--1I/V.x--");
        assert!(matches!(cmds.as_slice(), [Cmd::Bell]), "{cmds:?}");
        // Insert alone waits, and the value comes on its own scan.
        press(&mut app, KeyCode::Char('/'));
        app.search.set_text(String::new());
        inject_all(&mut app, "--1I--");
        assert!(app.control_status().unwrap().contains("insert"));
        inject_all(&mut app, "--1ME--");
        assert_eq!(app.search.text(), "alice");
        assert!(app.control_status().is_none());
        press(&mut app, KeyCode::Esc);
        // A search code runs the search straight away.
        let cmds = inject_all(&mut app, "--1QA/V.soap--");
        assert!(
            matches!(
                cmds.last(),
                Some(Cmd::Send(ClientMessage {
                    request: Request::Search { query, include_archived: true },
                    ..
                })) if query == "soap"
            ),
            "{cmds:?}"
        );
    }

    #[test]
    fn next_board_by_code_and_esc_cancels_an_open_frame() {
        let mut app = ready_app(true);
        let cmds = inject_all(&mut app, "--1GN--");
        assert!(
            matches!(
                cmds.as_slice(),
                [Cmd::Send(ClientMessage {
                    request: Request::GetBoard { board_id: BoardId(1) },
                    ..
                })]
            ),
            "the first board from the dashboard: {cmds:?}"
        );
        let cmds = inject_all(&mut app, "--1GP--");
        assert!(
            matches!(
                cmds.as_slice(),
                [Cmd::Send(ClientMessage {
                    request: Request::GetBoard { board_id: BoardId(2) },
                    ..
                })]
            ),
            "the last one backwards: {cmds:?}"
        );
        // A frame left half read is cancelled by Esc, with a word.
        inject_all(&mut app, "--1ML");
        assert!(app.scan.frame_open());
        assert!(app.control_status().unwrap().contains("long code"));
        press(&mut app, KeyCode::Esc);
        assert!(!app.scan.frame_open());
        assert_eq!(app.toast.as_ref().map(|t| t.text.as_str()), Some("control code cancelled"));
        // A task code under a window beeps instead of acting or typing.
        press(&mut app, KeyCode::Char('?'));
        let cmds = inject_all(&mut app, &payload());
        assert!(matches!(cmds.as_slice(), [Cmd::Bell]), "{cmds:?}");
    }

    fn task_and_board() -> (Task, Board) {
        let board = taskologic_core::board::test_support::board_with_members(1, &[1]);
        let task = taskologic_core::task::test_support::task_on(&board, 1);
        (task, board)
    }

    /// Answer the Resolve the app just sent with this task and board.
    fn answer_resolve(app: &mut App, cmds: &[Cmd], task: &Task, board: &Board) -> Vec<Cmd> {
        let id = cmds
            .iter()
            .find_map(|c| match c {
                Cmd::Send(ClientMessage { id, request: Request::Resolve { .. } }) => Some(*id),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no Resolve among {cmds:?}"));
        app.update(server(ServerMessage::Ok {
            id,
            response: Response::Resolved {
                task: Box::new(task.clone()),
                board: Box::new(board.clone()),
            },
        }))
    }

    #[test]
    fn an_armed_move_acts_on_the_next_scanned_task_and_sticky_stays() {
        let mut app = ready_app(true);
        let (task, board) = task_and_board();
        let here = board.columns.iter().position(|c| c.id == task.column_id).unwrap();
        let right = board.columns[here + 1].id;
        inject_all(&mut app, "--1MR--");
        assert!(app.control_status().unwrap().starts_with("next scan: move right"));
        let cmds = inject_all(&mut app, &payload());
        let cmds = answer_resolve(&mut app, &cmds, &task, &board);
        assert!(
            matches!(
                cmds.as_slice(),
                [Cmd::Send(ClientMessage { request: Request::MoveTask { task_id, to_column, .. }, .. })]
                    if *task_id == task.id && *to_column == right
            ),
            "{cmds:?}"
        );
        assert!(app.control.armed.is_none(), "one shot");
        // Sticky stays armed after a use, and Esc ends it.
        inject_all(&mut app, "--1MR/STK--");
        assert!(app.control_status().unwrap().starts_with("every scan"));
        let cmds = inject_all(&mut app, &payload());
        answer_resolve(&mut app, &cmds, &task, &board);
        assert!(app.control.armed.is_some(), "sticky");
        press(&mut app, KeyCode::Esc);
        assert!(app.control.armed.is_none());
        // A task scan under a window still completes an armed code.
        press(&mut app, KeyCode::Char('?'));
        inject_all(&mut app, "--1SH--");
        let cmds = inject_all(&mut app, &payload());
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Send(ClientMessage { request: Request::Resolve { .. }, .. }))), "{cmds:?}");
        press(&mut app, KeyCode::Esc);
    }

    #[test]
    fn delete_print_set_and_assign_by_scan_send_the_ordinary_requests() {
        use taskologic_core::control::SlipChoice;
        let mut app = ready_app(true);
        let (task, board) = task_and_board();
        let run = |app: &mut App, code: &str| {
            inject_all(app, code);
            let cmds = inject_all(app, &payload());
            answer_resolve(app, &cmds, &task, &board)
        };
        let cmds = run(&mut app, "--1DEL--");
        assert!(matches!(cmds.as_slice(), [Cmd::Send(ClientMessage { request: Request::DeleteTask { task_id }, .. })] if *task_id == task.id), "{cmds:?}");
        let cmds = run(&mut app, "--1PP--");
        assert!(matches!(cmds.as_slice(), [Cmd::Send(ClientMessage { request: Request::PrintTask { slip: Some(SlipChoice::Pause), .. }, .. })]), "{cmds:?}");
        let cmds = run(&mut app, "--1SU/N/P2H--");
        match cmds.as_slice() {
            [Cmd::Send(ClientMessage { request: Request::UpdateTask { draft, version, .. }, .. })] => {
                let due = draft.due_at.expect("a due date");
                let want = chrono::Utc::now() + chrono::TimeDelta::hours(2);
                assert!((due - want).num_seconds().abs() < 60, "{due} vs {want}");
                assert_eq!(*version, task.version);
                assert_eq!(draft.title, task.title, "the rest untouched");
            }
            other => panic!("{other:?}"),
        }
        let cmds = run(&mut app, "--1AM--");
        assert!(matches!(cmds.as_slice(), [Cmd::Send(ClientMessage { request: Request::UpdateTask { draft, .. }, .. })] if draft.assignees.contains(&1)), "{cmds:?}");
        let cmds = run(&mut app, "--1SC/V.Buy soap--");
        assert!(matches!(cmds.as_slice(), [Cmd::Send(ClientMessage { request: Request::UpdateTask { draft, .. }, .. })] if draft.checklist.last().is_some_and(|i| i.text == "Buy soap")), "{cmds:?}");
        // Set without a value opens the form on the field.
        run(&mut app, "--1ST--");
        assert!(matches!(app.overlay, Some(Overlay::TaskForm(_))), "the task form");
    }

    #[test]
    fn a_bare_move_to_asks_for_the_column_and_replace_swaps_the_field() {
        let mut app = ready_app(true);
        let (task, board) = task_and_board();
        inject_all(&mut app, "--1MC--");
        let cmds = inject_all(&mut app, &payload());
        let cmds = answer_resolve(&mut app, &cmds, &task, &board);
        assert!(cmds.is_empty(), "{cmds:?}");
        assert!(matches!(app.overlay, Some(Overlay::ColumnPick { .. })), "asks on screen");
        let cmds = press(&mut app, KeyCode::Char('3'));
        let third = board.columns[2].id;
        assert!(
            matches!(
                cmds.as_slice(),
                [Cmd::Send(ClientMessage { request: Request::MoveTask { to_column, .. }, .. })] if *to_column == third
            ),
            "{cmds:?}"
        );
        assert!(app.overlay.is_none());
        // Replace selects the field's text and types over it.
        press(&mut app, KeyCode::Char('/'));
        inject_all(&mut app, "--1I/V.abc--");
        assert_eq!(app.search.text(), "abc");
        inject_all(&mut app, "--1R/V.xyz--");
        assert_eq!(app.search.text(), "xyz");
        inject_all(&mut app, "--1R/X--");
        assert_eq!(app.search.text(), "", "replace with clear empties it");
    }

    /// Answer the Lookup the app just sent.
    fn answer_lookup(app: &mut App, cmds: &[Cmd], found: Found) -> Vec<Cmd> {
        let id = cmds
            .iter()
            .find_map(|c| match c {
                Cmd::Send(ClientMessage { id, request: Request::Lookup { .. }, .. }) => Some(*id),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no Lookup among {cmds:?}"));
        app.update(server(ServerMessage::Ok { id, response: Response::Found(found) }))
    }

    #[test]
    fn targeted_codes_look_the_thing_up_then_act() {
        let mut app = ready_app(true);
        let (_, board) = task_and_board();
        let sid = board.short_id;
        // Show board: a lookup, then the board is opened.
        let cmds = inject_all(&mut app, &format!("--1GB{sid}--"));
        assert!(matches!(cmds.as_slice(), [Cmd::Send(ClientMessage { request: Request::Lookup { kind: LookupKind::Board, .. }, .. })]), "{cmds:?}");
        let cmds = answer_lookup(&mut app, &cmds, Found::Board { board: Box::new(board.clone()) });
        assert!(matches!(cmds.as_slice(), [Cmd::Send(ClientMessage { request: Request::GetBoard { board_id }, .. })] if *board_id == board.id), "{cmds:?}");
        // Start a program now: the request goes out with the todo column.
        let program = taskologic_core::program::Program {
            id: taskologic_core::ids::ProgramId(7),
            short_id: taskologic_core::ids::ShortId::parse("PRG000").unwrap(),
            board_id: board.id,
            owner_uid: 1,
            name: "Clean up".into(),
            description: String::new(),
            steps: vec![taskologic_core::program::Step {
                key: taskologic_core::program::ROOT_KEY.into(),
                title: "Clean".into(),
                ..Default::default()
            }],
            min_samples: 3,
        };
        let cmds = inject_all(&mut app, "--1RNPRG000--");
        let cmds = answer_lookup(&mut app, &cmds, Found::Program { program: Box::new(program.clone()), board: Box::new(board.clone()) });
        let todo = App::column_for(&board, taskologic_core::control::ColumnRef::Todo);
        assert!(
            matches!(cmds.as_slice(), [Cmd::Send(ClientMessage { request: Request::StartProgram { program_id, column_id, start_at: None, .. }, .. })] if *program_id == program.id && *column_id == todo),
            "{cmds:?}"
        );
        // Start with the dialog: the start form opens instead.
        let cmds = inject_all(&mut app, "--1RPPRG000--");
        answer_lookup(&mut app, &cmds, Found::Program { program: Box::new(program), board: Box::new(board.clone()) });
        assert!(matches!(app.overlay, Some(Overlay::StartProgram(_))));
        press(&mut app, KeyCode::Esc);
        // New task from a template, straight into column 1.
        let tpl = taskologic_core::template::Template {
            id: taskologic_core::ids::TemplateId(3),
            short_id: taskologic_core::ids::ShortId::parse("TPL000").unwrap(),
            board_id: board.id,
            owner_uid: 1,
            name: "Wash up".into(),
            draft: taskologic_core::task::TaskDraft { title: "Wash up".into(), ..Default::default() },
            options: Default::default(),
        };
        let cmds = inject_all(&mut app, "--1NTTPL0001--");
        let cmds = answer_lookup(&mut app, &cmds, Found::Template { template: Box::new(tpl), board: Box::new(board.clone()) });
        // Stamped the way the templates panel does it, so dependency
        // templates come along.
        assert!(
            matches!(cmds.as_slice(), [Cmd::Send(ClientMessage { request: Request::CreateFromTemplate { template_id, column_id: Some(col), draft }, .. })] if *template_id == taskologic_core::ids::TemplateId(3) && *col == board.columns[0].id && draft.title == "Wash up"),
            "{cmds:?}"
        );
    }

    #[test]
    fn typing_into_the_search_box_never_scans() {
        let mut app = ready_app(true);
        press(&mut app, KeyCode::Char('/'));
        assert!(app.text_focused());
        let mut cmds = Vec::new();
        for ev in scan::inject(&payload(), false) {
            cmds.extend(app.update(Msg::Term(ev)));
        }
        assert!(cmds.is_empty(), "{cmds:?}");
        assert_eq!(app.search.text(), payload());
        let cmds = press(&mut app, KeyCode::Enter);
        assert!(matches!(
            &cmds[0],
            Cmd::Send(ClientMessage {
                request: Request::Search { .. },
                ..
            })
        ));
        assert!(!app.text_focused());
    }

    #[test]
    fn pick_up_and_shift_arrow_sends_a_move() {
        let mut app = ready_app(false);
        open_board(&mut app);
        assert_eq!(
            app.board.as_ref().unwrap().selected_task().unwrap().title,
            "Water the plants"
        );
        press(&mut app, KeyCode::Char(' '));
        let cmds = app.update(Msg::Term(TermEvent::Key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::SHIFT,
        ))));
        match &cmds[0] {
            Cmd::Send(ClientMessage {
                request:
                    Request::MoveTask {
                        task_id, to_column, ..
                    },
                ..
            }) => {
                assert_eq!(*task_id, TaskId(10));
                assert_eq!(*to_column, ColumnId(2));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn blocked_move_offers_an_override() {
        let mut app = ready_app(false);
        open_board(&mut app);
        press(&mut app, KeyCode::Char(' '));
        let cmds = app.update(Msg::Term(TermEvent::Key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::SHIFT,
        ))));
        let Cmd::Send(req) = &cmds[0] else { panic!() };
        let err = ErrorBody::new(
            ErrorCode::BlockedByDependencies,
            "1 dependencies are still open",
        )
        .with_detail(&vec![TaskId(11)]);
        app.update(server(ServerMessage::Err {
            id: req.id,
            error: err,
        }));
        assert!(matches!(&app.overlay, Some(Overlay::Confirm { .. })));
        let cmds = press(&mut app, KeyCode::Char('y'));
        assert!(matches!(
            &cmds[0],
            Cmd::Send(ClientMessage {
                request: Request::MoveTask {
                    override_deps: true,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn delete_moves_to_the_archive_and_purge_needs_the_owner() {
        let mut app = ready_app(false);
        open_board(&mut app);
        press(&mut app, KeyCode::Char('d'));
        assert!(matches!(app.overlay, Some(Overlay::Confirm { .. })));
        let cmds = press(&mut app, KeyCode::Char('y'));
        let Cmd::Send(req) = &cmds[0] else { panic!() };
        assert!(matches!(
            req.request,
            Request::DeleteTask {
                task_id: TaskId(10)
            }
        ));
        let mut deleted = app.board.as_ref().unwrap().detail.tasks[0].clone();
        deleted.archived_at = Some(chrono::Utc::now());
        deleted.deleted_at = deleted.archived_at;
        app.update(server(ServerMessage::Ok {
            id: req.id,
            response: Response::Task {
                task: deleted.clone(),
            },
        }));
        assert_eq!(app.board.as_ref().unwrap().detail.tasks.len(), 3);
        assert!(
            app.toast
                .as_ref()
                .is_some_and(|t| t.text.contains("30 days"))
        );

        // Archive panel: Enter restores, D deletes for good (owner here).
        let cmds = press(&mut app, KeyCode::Char('a'));
        let Cmd::Send(req) = &cmds[0] else { panic!() };
        app.update(server(ServerMessage::Ok {
            id: req.id,
            response: Response::Tasks {
                tasks: vec![deleted.clone()],
            },
        }));
        assert!(matches!(app.overlay, Some(Overlay::Archive(_))));
        press(&mut app, KeyCode::Char('D'));
        assert!(matches!(app.overlay, Some(Overlay::Confirm { .. })));
        let cmds = press(&mut app, KeyCode::Char('y'));
        assert!(matches!(
            &cmds[0],
            Cmd::Send(ClientMessage {
                request: Request::PurgeTask {
                    task_id: TaskId(10)
                },
                ..
            })
        ));
    }

    #[test]
    fn dashboard_delete_board_respects_lock_and_admin_visibility() {
        let mut app = ready_app(false);
        // Second board is locked and owned by someone else; alice is admin.
        press(&mut app, KeyCode::Down);
        assert!(press(&mut app, KeyCode::Char('D')).is_empty());
        assert!(
            app.toast
                .as_ref()
                .is_some_and(|t| t.text.contains("locked"))
        );
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Char('D'));
        assert!(matches!(app.overlay, Some(Overlay::Confirm { .. })));
        let cmds = press(&mut app, KeyCode::Char('y'));
        assert!(matches!(
            &cmds[0],
            Cmd::Send(ClientMessage {
                request: Request::DeleteBoard {
                    board_id: BoardId(1)
                },
                ..
            })
        ));
        // A private board the admin is not on cannot be opened.
        app.boards[1].is_member = false;
        app.board_sel = 1;
        assert!(press(&mut app, KeyCode::Enter).is_empty());
        assert!(
            app.toast
                .as_ref()
                .is_some_and(|t| t.text.contains("not a member"))
        );
    }

    #[test]
    fn n_opens_the_task_form_and_saving_creates_the_task() {
        let mut app = ready_app(false);
        open_board(&mut app);
        let cmds = press(&mut app, KeyCode::Char('n'));
        assert!(matches!(app.overlay, Some(Overlay::TaskForm(_))));
        assert!(matches!(
            &cmds[0],
            Cmd::Send(ClientMessage {
                request: Request::ListMembers { .. },
                ..
            })
        ));
        assert!(
            app.text_focused(),
            "scan detection is off while the form is open"
        );
        for c in "Mop".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        let cmds = press(&mut app, KeyCode::F(2));
        match &cmds[0] {
            Cmd::Send(ClientMessage {
                request:
                    Request::CreateTask {
                        column_id, draft, ..
                    },
                id,
            }) => {
                assert_eq!(*column_id, Some(ColumnId(1)));
                assert_eq!(draft.title, "Mop");
                let board = taskologic_core::board::test_support::board_with_members(1, &[1]);
                let mut task = taskologic_core::task::test_support::task_on(&board, 1);
                task.id = TaskId(99);
                task.title = "Mop".into();
                app.update(server(ServerMessage::Ok {
                    id: *id,
                    response: Response::Task { task },
                }));
            }
            other => panic!("{other:?}"),
        }
        assert!(app.overlay.is_none());
        assert_eq!(app.board.as_ref().unwrap().detail.tasks.len(), 5);
    }

    #[test]
    fn edit_conflict_offers_reload_or_overwrite() {
        let mut app = ready_app(false);
        open_board(&mut app);
        press(&mut app, KeyCode::Char('e'));
        assert!(
            matches!(&app.overlay, Some(Overlay::TaskForm(f)) if f.mode == FormMode::Edit { task: TaskId(10), version: 1 })
        );
        let cmds = press(&mut app, KeyCode::F(2));
        let Cmd::Send(req) = &cmds[0] else { panic!() };
        assert!(matches!(
            req.request,
            Request::UpdateTask { version: 1, .. }
        ));
        let mut current = app.board.as_ref().unwrap().detail.tasks[0].clone();
        current.version = 4;
        current.title = "Water the plants twice".into();
        let err = ErrorBody::new(ErrorCode::Conflict, "task changed since you loaded it")
            .with_detail(&current);
        app.update(server(ServerMessage::Err {
            id: req.id,
            error: err,
        }));
        assert!(matches!(app.overlay, Some(Overlay::Conflict { .. })));
        let cmds = press(&mut app, KeyCode::Char('o'));
        let Cmd::Send(req) = &cmds[0] else { panic!() };
        assert!(
            matches!(req.request, Request::UpdateTask { version: 4, .. }),
            "overwrite uses the current version"
        );
        assert!(matches!(app.overlay, Some(Overlay::TaskForm(_))));
    }

    #[test]
    fn settings_save_updates_prefs_locally_and_sends_them() {
        let mut app = ready_app(false);
        press(&mut app, KeyCode::Char('S'));
        assert!(matches!(app.overlay, Some(Overlay::Settings(_))));
        // First checkbox is "board tabs", on by default. Space flips it.
        press(&mut app, KeyCode::Char(' '));
        let cmds = press(&mut app, KeyCode::F(2));
        assert!(
            matches!(&cmds[0], Cmd::Send(ClientMessage { request: Request::UpdatePrefs { prefs }, .. }) if !prefs.ui.show_board_tabs)
        );
        assert!(!app.user.as_ref().unwrap().prefs.ui.show_board_tabs);
    }

    #[test]
    fn new_board_form_creates_with_defaults() {
        let mut app = ready_app(false);
        let cmds = press(&mut app, KeyCode::Char('N'));
        assert!(matches!(
            &cmds[0],
            Cmd::Send(ClientMessage {
                request: Request::ListUsers,
                ..
            })
        ));
        for c in "Garage".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        let cmds = press(&mut app, KeyCode::F(2));
        match &cmds[0] {
            Cmd::Send(ClientMessage {
                request: Request::CreateBoard(req),
                ..
            }) => {
                assert_eq!(req.name, "Garage");
                assert_eq!(req.columns.len(), 4);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn column_removal_asks_for_a_destination_and_role_replacement() {
        let mut app = ready_app(false);
        open_board(&mut app);
        press(&mut app, KeyCode::Char('c'));
        assert!(matches!(app.overlay, Some(Overlay::Columns(_))));
        // Todo (column 1) has tasks and no role: one question, then confirm.
        press(&mut app, KeyCode::Char('d'));
        assert!(
            matches!(&app.overlay, Some(Overlay::PickColumn { options, .. }) if options.len() == 3)
        );
        press(&mut app, KeyCode::Enter);
        assert!(matches!(&app.overlay, Some(Overlay::Confirm { .. })));
        let cmds = press(&mut app, KeyCode::Char('y'));
        match &cmds[0] {
            Cmd::Send(ClientMessage {
                request:
                    Request::RemoveColumn {
                        column_id,
                        move_tasks_to,
                        replacements,
                    },
                ..
            }) => {
                assert_eq!(*column_id, ColumnId(1));
                assert_eq!(*move_tasks_to, Some(ColumnId(2)));
                assert!(replacements.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn removing_a_member_with_tasks_offers_to_unassign() {
        let mut app = ready_app(false);
        open_board(&mut app);
        press(&mut app, KeyCode::Char('m'));
        assert!(matches!(app.overlay, Some(Overlay::Members(_))));
        if let Some(Overlay::Members(panel)) = &mut app.overlay {
            // Bob first: list navigation needs a render to know its row count.
            panel.set_members(vec![
                taskologic_core::user::UserSummary {
                    uid: 2,
                    username: "bob".into(),
                    is_admin: false,
                },
                taskologic_core::user::UserSummary {
                    uid: 1,
                    username: "alice".into(),
                    is_admin: true,
                },
            ]);
        }
        let cmds = press(&mut app, KeyCode::Char('x'));
        let Cmd::Send(req) = &cmds[0] else {
            panic!("{cmds:?}")
        };
        assert!(matches!(
            req.request,
            Request::RemoveMember {
                uid: 2,
                unassign: false,
                ..
            }
        ));
        let err = ErrorBody::new(ErrorCode::MemberHasTasks, "that member still has 1 tasks")
            .with_detail(&vec![TaskId(10)]);
        app.update(server(ServerMessage::Err {
            id: req.id,
            error: err,
        }));
        assert!(matches!(&app.overlay, Some(Overlay::Confirm { .. })));
        let cmds = press(&mut app, KeyCode::Char('y'));
        assert!(matches!(
            &cmds[0],
            Cmd::Send(ClientMessage {
                request: Request::RemoveMember {
                    uid: 2,
                    unassign: true,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn events_update_the_open_board() {
        let mut app = ready_app(false);
        open_board(&mut app);
        let mut moved = app.board.as_ref().unwrap().detail.tasks[0].clone();
        moved.column_id = ColumnId(4);
        app.update(server(ServerMessage::Event {
            event: Event::TaskChanged {
                task: moved,
                change: TaskChange::Moved {
                    from: ColumnId(1),
                    to: ColumnId(4),
                },
                actor: Some(2),
            },
        }));
        let b = app.board.as_ref().unwrap();
        assert_eq!(b.tasks_in(0).len(), 1);
        assert_eq!(b.tasks_in(3).len(), 2);
        let gone = b.detail.tasks[1].clone();
        app.update(server(ServerMessage::Event {
            event: Event::TaskChanged {
                task: gone,
                change: TaskChange::Deleted,
                actor: None,
            },
        }));
        assert_eq!(app.board.as_ref().unwrap().detail.tasks.len(), 3);
        app.update(server(ServerMessage::Event {
            event: Event::BoardChanged {
                board_id: BoardId(1),
                change: BoardChange::AccessRevoked,
            },
        }));
        assert!(app.board.is_none());
    }

    #[test]
    fn print_job_events_turn_into_print_commands_and_acks() {
        let mut app = ready_app(false);
        open_board(&mut app);
        let board = taskologic_core::board::test_support::board_with_members(1, &[1]);
        let task = taskologic_core::task::test_support::task_on(&board, 1);
        let job = taskologic_core::print::build_reminder_job(
            &task,
            &board,
            &user(false),
            chrono::DateTime::from_timestamp(0, 0).unwrap(),
        );
        let cmds = app.update(server(ServerMessage::Event {
            event: Event::PrintJob {
                job_id: PrintJobId(5),
                job: job.clone(),
            },
        }));
        assert_eq!(
            cmds,
            vec![Cmd::Print {
                job_id: PrintJobId(5),
                job
            }]
        );
        let cmds = app.update(Msg::Printed {
            job_id: PrintJobId(5),
            error: Some("out of paper".into()),
        });
        assert!(matches!(
            &cmds[0],
            Cmd::Send(ClientMessage {
                request: Request::AckPrintJob {
                    job_id: PrintJobId(5),
                    error: Some(_)
                },
                ..
            })
        ));
        assert!(
            app.toast
                .as_ref()
                .is_some_and(|t| t.severity == Severity::Error)
        );
    }
}
