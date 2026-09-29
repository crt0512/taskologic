//! Moving things between boards or servers as JSON files: a program, a
//! template with what it depends on, a board with everything on it, or the
//! whole server.
//!
//! `scripts/transfer.sh` is the way to run these; this is what it calls.
//! They are one shot modes of the daemon binary, like `--migrate`, so they
//! read and write the database with the daemon's own code. Running them
//! while the daemon is up is fine; the panels show what came in the next
//! time they are opened. Only a Taskologic admin or root may run them: a
//! full export is every task on every board, private ones included.
//!
//! People are the one thing a file cannot carry across on its own: uids
//! mean nothing on another server. Every file therefore lists the people it
//! names, uid and username, and `--remap-users` rewrites a file's uids to
//! this server's by username before it is imported. A program or template
//! import drops anyone the target board lacks and says so; a board import
//! keeps uids as they are, which is right after a remap and wrong before.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use taskologic_core::board::Board;
use taskologic_core::event::EventKind;
use taskologic_core::ids::{BoardId, ColumnId, ProgramId, RunId, TaskId, TemplateId, Uid};
use taskologic_core::offset::Offset;
use taskologic_core::prefs::UserPrefs;
use taskologic_core::print::{PrintRule, Recipients};
use taskologic_core::program::{self, Assign, Program, ProgramDraft, Question, Run};
use taskologic_core::repeat::RepeatSpec;
use taskologic_core::task::{Task, TaskDraft};
use taskologic_core::template::{self, FromAverage, Template, TemplateOptions};
use taskologic_core::user::User;

use crate::auth::Identity;
use crate::db::{Db, dt, repo, ts};
use crate::error::AppError;

/// The file format. Bumped if a file written now could be misread later.
pub const FORMAT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    ExportProgram {
        board: String,
        name: String,
        file: Option<PathBuf>,
    },
    ImportProgram {
        board: String,
        file: PathBuf,
    },
    ExportTemplate {
        board: String,
        name: String,
        file: Option<PathBuf>,
    },
    ImportTemplate {
        board: String,
        file: PathBuf,
    },
    /// Every account and every board, for moving a whole server.
    ExportAll {
        file: Option<PathBuf>,
    },
    ImportAll {
        file: PathBuf,
    },
    /// One board by name, or every board with `all`, with the people it
    /// names but not their accounts.
    ExportBoard {
        name: String,
        file: Option<PathBuf>,
    },
    ImportBoard {
        file: PathBuf,
    },
    /// Who this server knows, uid by uid.
    Users,
    /// Rewrite the uids in a file to this server's, matched by username.
    /// In place unless `out` says where else.
    RemapUsers {
        file: PathBuf,
        out: Option<PathBuf>,
    },
}

/// What every file says about itself first.
#[derive(Serialize, Deserialize)]
struct Header {
    format: u32,
    kind: String,
    taskologic: String,
    exported_at: DateTime<Utc>,
    /// The board it came from, for the reader; nothing matches on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    board: Option<String>,
}

impl Header {
    fn new(kind: &str, board: Option<&Board>) -> Self {
        Self {
            format: FORMAT,
            kind: kind.into(),
            taskologic: env!("CARGO_PKG_VERSION").into(),
            exported_at: Utc::now(),
            board: board.map(|b| b.name.clone()),
        }
    }

    fn check(&self, kinds: &[&str]) -> Result<(), AppError> {
        if self.format > FORMAT {
            return Err(AppError::bad(format!(
                "this file is format {}, written by Taskologic {}; this build reads up to {FORMAT}",
                self.format, self.taskologic
            )));
        }
        if !kinds.contains(&self.kind.as_str()) {
            return Err(AppError::bad(format!(
                "this file holds a {}, not a {}",
                self.kind,
                kinds.join(" or ")
            )));
        }
        Ok(())
    }
}

/// Somebody a file mentions, so `--remap-users` can find them again on
/// another server by name.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
struct Person {
    uid: Uid,
    username: String,
    /// Present when they have logged in to the server the file came from;
    /// importing a whole server recreates it. Absent for people known only
    /// from the unix group, and in program and template files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<Account>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
struct Account {
    is_admin: bool,
    timezone: String,
    prefs: UserPrefs,
    created_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
struct ProgramFile {
    #[serde(rename = "taskologic")]
    header: Header,
    #[serde(default)]
    users: Vec<Person>,
    program: ProgramDraft,
}

/// One template, its dependencies named rather than numbered so they can
/// be matched up again on another board.
#[derive(Serialize, Deserialize)]
struct TemplateEntry {
    name: String,
    draft: TaskDraft,
    #[serde(default)]
    start_prefill: Option<Offset>,
    #[serde(default)]
    due_prefill: Option<Offset>,
    #[serde(default)]
    due_from_average: Option<FromAverage>,
    #[serde(default)]
    depends_on: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct TemplateFile {
    #[serde(rename = "taskologic")]
    header: Header,
    #[serde(default)]
    users: Vec<Person>,
    /// The one that was asked for; the rest are what it depends on.
    root: String,
    /// Dependencies first, so an import can take them in order.
    templates: Vec<TemplateEntry>,
}

/// A whole server, or one or more boards: kind `everything` or `board`.
#[derive(Serialize, Deserialize)]
struct Everything {
    #[serde(rename = "taskologic")]
    header: Header,
    #[serde(default)]
    users: Vec<Person>,
    #[serde(default)]
    boards: Vec<BoardBundle>,
}

/// A board and everything that hangs off it, ids as they were. An import
/// hands out fresh ones and rewrites every reference.
#[derive(Serialize, Deserialize)]
struct BoardBundle {
    board: Board,
    #[serde(default)]
    templates: Vec<Template>,
    #[serde(default)]
    programs: Vec<Program>,
    /// Live and archived, the archive last.
    #[serde(default)]
    tasks: Vec<Task>,
    /// The repetition rows themselves, stopped ones included, rather than
    /// the active rule each task carries, so the next firing survives.
    #[serde(default)]
    repeats: Vec<RepeatRow>,
    #[serde(default)]
    runs: Vec<RunBundle>,
    #[serde(default)]
    autoprinted: Vec<AutoprintRow>,
    #[serde(default)]
    reminders_sent: Vec<SentRow>,
    #[serde(default)]
    events: Vec<EventRow>,
}

#[derive(Serialize, Deserialize)]
struct RepeatRow {
    task: TaskId,
    rule: RepeatSpec,
    next_fire_at: Option<DateTime<Utc>>,
    active: bool,
}

#[derive(Serialize, Deserialize)]
struct RunBundle {
    run: Run,
    name: String,
    tasks: Vec<RunTaskRow>,
}

#[derive(Serialize, Deserialize)]
struct RunTaskRow {
    task: TaskId,
    step: String,
    iteration: u32,
    param: Option<String>,
    counts: bool,
    time_limit: Option<Offset>,
    paused_at: Option<DateTime<Utc>>,
    skipped: bool,
    question: Option<Question>,
    asked: bool,
    answer: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct AutoprintRow {
    task: TaskId,
    uid: Uid,
    printed_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
struct SentRow {
    task: TaskId,
    uid: Uid,
    kind: String,
    anchor_at: DateTime<Utc>,
    sent_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
struct EventRow {
    task: Option<TaskId>,
    actor_uid: Option<Uid>,
    kind: EventKind,
    at: DateTime<Utc>,
}

/// Run one command as `who`, who becomes the creator of whatever a program
/// or template import makes. `locals` are the people in the daemon's unix
/// group, which is how a server knows the names of people who have not
/// logged in yet. Progress and warnings go to `out`.
pub fn run(
    db: &Db,
    cmd: Command,
    who: Uid,
    locals: &[Identity],
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    db.with(|c| authorize(c, who))?;
    match cmd {
        Command::ExportProgram { board, name, file } => {
            let json = db.with(|c| export_program(c, &board, &name, locals))?;
            emit(&json, file, out)
        }
        Command::ExportTemplate { board, name, file } => {
            let json = db.with(|c| export_template(c, &board, &name, locals))?;
            emit(&json, file, out)
        }
        Command::ImportProgram { board, file } => {
            let text = read(&file)?;
            let notes = db.tx(|c| import_program(c, &board, &text, who))?;
            report(&notes, out)
        }
        Command::ImportTemplate { board, file } => {
            let text = read(&file)?;
            let notes = db.tx(|c| import_template(c, &board, &text, who))?;
            report(&notes, out)
        }
        Command::ExportAll { file } => {
            let json = db.with(|c| export_everything(c, None, locals))?;
            emit(&json, file, out)
        }
        Command::ExportBoard { name, file } => {
            let json = db.with(|c| export_everything(c, Some(&name), locals))?;
            emit(&json, file, out)
        }
        Command::ImportAll { file } => {
            let text = read(&file)?;
            let notes = db.tx(|c| import_everything(c, &text, true))?;
            report(&notes, out)
        }
        Command::ImportBoard { file } => {
            let text = read(&file)?;
            let notes = db.tx(|c| import_everything(c, &text, false))?;
            report(&notes, out)
        }
        Command::Users => {
            let table = db.with(|c| users_table(c, locals))?;
            write!(out, "{table}")?;
            Ok(())
        }
        Command::RemapUsers { file, out: target } => {
            let text = read(&file)?;
            let (json, notes) = db.with(|c| remap_users(c, locals, &text))?;
            report(&notes, out)?;
            // In place, unless it came from stdin, which goes back to stdout.
            let target = target.or_else(|| (file.as_os_str() != "-").then(|| file.clone()));
            emit(&json, target, out)
        }
    }
}

/// Root, or somebody who is an admin inside Taskologic. Nobody else: a full
/// export is every task on every board, and an import writes what it likes.
fn authorize(c: &Connection, who: Uid) -> Result<(), AppError> {
    if who == 0 {
        return Ok(());
    }
    match repo::get_user(c, who)? {
        Some(u) if u.is_admin => Ok(()),
        Some(u) => Err(AppError::bad(format!(
            "only a Taskologic admin or root may do this, and {} (uid {who}) is not an admin",
            u.username
        ))),
        None => Err(AppError::bad(format!(
            "only a Taskologic admin or root may do this, and uid {who} has never logged in to Taskologic"
        ))),
    }
}
fn emit(json: &str, file: Option<PathBuf>, out: &mut dyn Write) -> anyhow::Result<()> {
    match file {
        Some(path) => {
            std::fs::write(&path, format!("{json}\n"))
                .map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))?;
            writeln!(out, "written to {}", path.display())?;
        }
        None => writeln!(out, "{json}")?,
    }
    Ok(())
}

fn read(path: &PathBuf) -> anyhow::Result<String> {
    if path.as_os_str() == "-" {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
        return Ok(text);
    }
    std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))
}

fn report(notes: &[String], out: &mut dyn Write) -> anyhow::Result<()> {
    for n in notes {
        writeln!(out, "{n}")?;
    }
    Ok(())
}

/// The one board called `name`, whatever the case. Two boards with one name
/// is a thing the UI allows, so it is an error here rather than a guess.
fn board_named(c: &Connection, name: &str) -> Result<Board, AppError> {
    let ids = repo::boards_named(c, name)?;
    match ids.as_slice() {
        [] => Err(AppError::bad(format!("no board called {name:?}"))),
        [id] => repo::require_board(c, *id),
        _ => Err(AppError::bad(format!(
            "several boards are called {name:?}; rename one first"
        ))),
    }
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn export_program(
    c: &Connection,
    board: &str,
    name: &str,
    locals: &[Identity],
) -> Result<String, AppError> {
    let board = board_named(c, board)?;
    let matching: Vec<_> = repo::list_programs(c, board.id)?
        .into_iter()
        .filter(|p| same(&p.name, name))
        .collect();
    let program = match matching.as_slice() {
        [] => {
            return Err(AppError::bad(format!(
                "no program called {name:?} on board {:?}",
                board.name
            )));
        }
        [p] => p,
        _ => {
            return Err(AppError::bad(format!(
                "several programs on {:?} are called {name:?}; rename one first",
                board.name
            )));
        }
    };
    let file = ProgramFile {
        header: Header::new("program", Some(&board)),
        users: Vec::new(),
        program: program.draft(),
    };
    with_people(c, locals, &file, People::Names)
}

fn import_program(
    c: &Connection,
    board: &str,
    text: &str,
    owner: Uid,
) -> Result<Vec<String>, AppError> {
    let board = board_named(c, board)?;
    let file: ProgramFile = serde_json::from_str(text)
        .map_err(|e| AppError::bad(format!("this is not a program file: {e}")))?;
    file.header.check(&["program"])?;
    let mut draft = file.program;
    let mut notes = Vec::new();
    for step in &mut draft.steps {
        if let Assign::Users(uids) = &step.assign {
            let kept = known(c, &board, uids)?;
            if kept.len() != uids.len() {
                notes.push(format!(
                    "step {}: dropped {} assignee(s) who are not on this board",
                    step.key,
                    uids.len() - kept.len()
                ));
            }
            step.assign = if kept.is_empty() {
                Assign::Starter
            } else {
                Assign::Users(kept)
            };
        }
        notes.extend(strip_print_users(c, &board, &mut step.print, &format!("step {}", step.key))?);
    }
    program::validate(&draft, &board).map_err(|e| AppError::bad(e.to_string()))?;
    let p = repo::create_program(c, board.id, owner, &draft)?;
    repo::record_event(
        c,
        board.id,
        None,
        Some(owner),
        &EventKind::ProgramCreated { program: p.id },
        Utc::now(),
    )?;
    notes.push(format!(
        "imported program {:?} onto board {:?} with {} step(s)",
        p.name,
        board.name,
        p.steps.len()
    ));
    Ok(notes)
}

/// Which of these uids are people this board has.
fn known(c: &Connection, board: &Board, uids: &[Uid]) -> Result<Vec<Uid>, AppError> {
    let mut kept = Vec::new();
    for uid in uids {
        if board.is_member(*uid) && repo::get_user(c, *uid)?.is_some() {
            kept.push(*uid);
        }
    }
    Ok(kept)
}

/// Print rules naming people: keep the ones this board has, fall back to the
/// assignees when nobody is left.
fn strip_print_users(
    c: &Connection,
    board: &Board,
    rules: &mut [PrintRule],
    what: &str,
) -> Result<Vec<String>, AppError> {
    let mut notes = Vec::new();
    for rule in rules {
        if let Recipients::Users(uids) = &rule.to {
            let kept = known(c, board, uids)?;
            if kept.len() != uids.len() {
                notes.push(format!(
                    "{what}: a print rule named {} person(s) who are not on this board, dropped",
                    uids.len() - kept.len()
                ));
            }
            rule.to = if kept.is_empty() {
                Recipients::Assignees
            } else {
                Recipients::Users(kept)
            };
        }
    }
    Ok(notes)
}

fn export_template(
    c: &Connection,
    board: &str,
    name: &str,
    locals: &[Identity],
) -> Result<String, AppError> {
    let board = board_named(c, board)?;
    let all = repo::list_templates(c, board.id)?;
    let matching: Vec<_> = all.iter().filter(|t| same(&t.name, name)).collect();
    let root = match matching.as_slice() {
        [] => {
            return Err(AppError::bad(format!(
                "no template called {name:?} on board {:?}",
                board.name
            )));
        }
        [t] => *t,
        _ => {
            return Err(AppError::bad(format!(
                "several templates on {:?} are called {name:?}; rename one first",
                board.name
            )));
        }
    };
    let by_id: HashMap<TemplateId, &taskologic_core::template::Template> =
        all.iter().map(|t| (t.id, t)).collect();
    let graph = repo::template_deps_graph(c, board.id)?;
    // The dependencies, deepest first, then the one asked for.
    let mut order = template::expansion_order(&graph, root.id);
    order.push(root.id);
    let templates = order
        .iter()
        .filter_map(|id| by_id.get(id))
        .map(|t| TemplateEntry {
            name: t.name.clone(),
            draft: t.draft.clone(),
            start_prefill: t.options.start_prefill,
            due_prefill: t.options.due_prefill,
            due_from_average: t.options.due_from_average,
            depends_on: t
                .options
                .dep_templates
                .iter()
                .filter_map(|d| by_id.get(d).map(|d| d.name.clone()))
                .collect(),
        })
        .collect();
    let file = TemplateFile {
        header: Header::new("template", Some(&board)),
        users: Vec::new(),
        root: root.name.clone(),
        templates,
    };
    with_people(c, locals, &file, People::Names)
}

fn import_template(
    c: &Connection,
    board: &str,
    text: &str,
    owner: Uid,
) -> Result<Vec<String>, AppError> {
    let board = board_named(c, board)?;
    let file: TemplateFile = serde_json::from_str(text)
        .map_err(|e| AppError::bad(format!("this is not a template file: {e}")))?;
    file.header.check(&["template"])?;
    let mut notes = Vec::new();
    // A template the board already has by that name is used as it is, not
    // doubled: the file may name it only as something to depend on. To get
    // a fresh copy, rename it in the file first.
    let mut ids: HashMap<String, TemplateId> = repo::list_templates(c, board.id)?
        .into_iter()
        .map(|t| (t.name.trim().to_lowercase(), t.id))
        .collect();
    for entry in file.templates {
        let key = entry.name.trim().to_lowercase();
        if let Some(id) = ids.get(&key) {
            notes.push(format!(
                "template {:?} is on this board already (id {id}), left as it is",
                entry.name
            ));
            continue;
        }
        let mut draft = entry.draft;
        // A template carries neither dates nor task dependencies, the same
        // rule the save path applies.
        draft.start_at = None;
        draft.due_at = None;
        draft.depends_on.clear();
        let before = draft.assignees.len();
        draft.assignees = known(c, &board, &draft.assignees)?;
        if draft.assignees.len() != before {
            notes.push(format!(
                "template {:?}: dropped {} assignee(s) who are not on this board",
                entry.name,
                before - draft.assignees.len()
            ));
        }
        notes.extend(strip_print_users(
            c,
            &board,
            &mut draft.print_rules,
            &format!("template {:?}", entry.name),
        )?);
        let mut dep_templates = Vec::new();
        for dep in &entry.depends_on {
            match ids.get(&dep.trim().to_lowercase()) {
                Some(id) => dep_templates.push(*id),
                None => notes.push(format!(
                    "template {:?}: depends on {dep:?}, which is neither in the file nor on the board; skipped",
                    entry.name
                )),
            }
        }
        taskologic_core::task::validate_draft(&draft, &board)?;
        let graph = repo::template_deps_graph(c, board.id)?;
        if let Some(bad) = template::first_cycle(&graph, TemplateId(0), &dep_templates) {
            return Err(AppError::bad(format!(
                "template {:?} would close a dependency cycle through {bad}",
                entry.name
            )));
        }
        let options = TemplateOptions {
            start_prefill: entry.start_prefill,
            due_prefill: entry.due_prefill,
            due_from_average: entry.due_from_average,
            dep_templates,
        };
        let t = repo::create_template(c, board.id, owner, entry.name.trim(), &draft, &options)?;
        repo::record_event(
            c,
            board.id,
            None,
            Some(owner),
            &EventKind::TemplateCreated { template: t.id },
            Utc::now(),
        )?;
        notes.push(format!("imported template {:?} (id {})", t.name, t.id));
        ids.insert(key, t.id);
    }
    if !ids.contains_key(&file.root.trim().to_lowercase()) {
        notes.push(format!(
            "note: the file names {:?} as the template asked for, but it is not among its entries",
            file.root
        ));
    }
    Ok(notes)
}

/// Which people a file lists.
#[derive(Clone, Copy, PartialEq, Eq)]
enum People {
    /// Names only, for a program or template: enough to remap by.
    Names,
    /// Everyone the file mentions, with their account when they have one.
    Mentioned,
    /// Every account there is, mentioned or not: the whole server.
    Everyone,
}

/// Serialize a file and fill in its `users` list from the uids it mentions.
fn with_people(
    c: &Connection,
    locals: &[Identity],
    file: &impl Serialize,
    which: People,
) -> Result<String, AppError> {
    let mut v = serde_json::to_value(file)?;
    let people = people_in(c, locals, &v, which)?;
    v["users"] = serde_json::to_value(people)?;
    Ok(serde_json::to_string_pretty(&v)?)
}

/// The people a file mentions, named. Accounts come from the users table;
/// somebody known only from the unix group gets their name and nothing
/// else; a uid nobody here can name is left out, since it could not be
/// remapped anyway.
fn people_in(
    c: &Connection,
    locals: &[Identity],
    v: &Value,
    which: People,
) -> Result<Vec<Person>, AppError> {
    let mut uids = HashSet::new();
    uids_in(v, &mut uids);
    let users: HashMap<Uid, User> = repo::list_users(c)?
        .into_iter()
        .map(|u| (u.uid, u))
        .collect();
    if which == People::Everyone {
        uids.extend(users.keys().copied());
    }
    let group: HashMap<Uid, &str> = locals
        .iter()
        .map(|i| (i.uid, i.username.as_str()))
        .collect();
    let mut uids: Vec<Uid> = uids.into_iter().collect();
    uids.sort_unstable();
    Ok(uids
        .into_iter()
        .filter_map(|uid| {
            if let Some(u) = users.get(&uid) {
                let account = (which != People::Names).then(|| Account {
                    is_admin: u.is_admin,
                    timezone: u.timezone.name().into(),
                    prefs: u.prefs.clone(),
                    created_at: u.created_at,
                });
                return Some(Person {
                    uid,
                    username: u.username.clone(),
                    account,
                });
            }
            group.get(&uid).map(|name| Person {
                uid,
                username: (*name).into(),
                account: None,
            })
        })
        .collect())
}

/// Keys whose value is one uid, wherever they sit in a file.
const UID_KEYS: &[&str] = &["uid", "owner_uid", "created_by", "started_by", "actor_uid"];
/// Keys whose value is a list of uids: a task's assignees, a board's
/// members, the people a step assigns or a print rule prints for. The file's
/// own `users` list is objects, not numbers, and is walked into instead.
/// A member removal's `unassigned` is task ids and is not in here.
const UID_LIST_KEYS: &[&str] = &["assignees", "members", "users"];

fn uid_list(v: &Value) -> Option<&Vec<Value>> {
    v.as_array()
        .filter(|a| !a.is_empty() && a.iter().all(Value::is_u64))
}

fn as_uid(v: &Value) -> Option<Uid> {
    v.as_u64().and_then(|n| Uid::try_from(n).ok())
}

fn uids_in(v: &Value, out: &mut HashSet<Uid>) {
    match v {
        Value::Object(o) => {
            for (k, child) in o {
                if UID_KEYS.contains(&k.as_str()) {
                    out.extend(as_uid(child));
                } else if let Some(list) = uid_list(child).filter(|_| UID_LIST_KEYS.contains(&k.as_str())) {
                    out.extend(list.iter().filter_map(as_uid));
                } else {
                    uids_in(child, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|e| uids_in(e, out)),
        _ => {}
    }
}

/// Rewrite every uid in `map`, wherever it sits. Returns how many changed.
fn remap_uids(v: &mut Value, map: &HashMap<Uid, Uid>) -> usize {
    let swap = |e: &mut Value| -> usize {
        match as_uid(e).and_then(|u| map.get(&u)) {
            Some(new) => {
                *e = Value::from(*new);
                1
            }
            None => 0,
        }
    };
    match v {
        Value::Object(o) => {
            let mut n = 0;
            for (k, child) in o.iter_mut() {
                if UID_KEYS.contains(&k.as_str()) {
                    n += swap(child);
                } else if uid_list(child).is_some() && UID_LIST_KEYS.contains(&k.as_str()) {
                    for e in child.as_array_mut().into_iter().flatten() {
                        n += swap(e);
                    }
                } else {
                    n += remap_uids(child, map);
                }
            }
            n
        }
        Value::Array(a) => a.iter_mut().map(|e| remap_uids(e, map)).sum(),
        _ => 0,
    }
}

/// The whole server without a name, one board by name, every board with
/// `all`.
fn export_everything(
    c: &Connection,
    board: Option<&str>,
    locals: &[Identity],
) -> Result<String, AppError> {
    let (kind, boards, which) = match board {
        None => ("everything", all_boards(c)?, People::Everyone),
        Some(name) if name.eq_ignore_ascii_case("all") => ("board", all_boards(c)?, People::Mentioned),
        Some(name) => ("board", vec![board_named(c, name)?], People::Mentioned),
    };
    let named = (kind == "board" && boards.len() == 1).then(|| boards[0].clone());
    let bundles = boards
        .into_iter()
        .map(|b| bundle(c, b))
        .collect::<Result<Vec<_>, _>>()?;
    let file = Everything {
        header: Header::new(kind, named.as_ref()),
        users: Vec::new(),
        boards: bundles,
    };
    with_people(c, locals, &file, which)
}

fn all_boards(c: &Connection) -> Result<Vec<Board>, AppError> {
    let mut st = c.prepare("SELECT id FROM boards ORDER BY id")?;
    let ids: Vec<BoardId> = st
        .query_map([], |r| Ok(BoardId(r.get(0)?)))?
        .collect::<Result<_, _>>()?;
    ids.into_iter().map(|id| repo::require_board(c, id)).collect()
}

/// Everything a board has, read straight from its tables.
fn bundle(c: &Connection, board: Board) -> Result<BoardBundle, AppError> {
    let id = board.id;
    let templates = repo::list_templates(c, id)?;
    let programs = repo::list_programs(c, id)?;
    let mut tasks = repo::list_tasks(c, id, false)?;
    tasks.extend(repo::list_tasks(c, id, true)?);
    tasks.sort_by_key(|t| t.id);

    let mut st = c.prepare(
        "SELECT r.task_id, r.rule_json, r.next_fire_at, r.active FROM repeats r \
         JOIN tasks t ON t.id = r.task_id WHERE t.board_id = ?1 ORDER BY r.task_id",
    )?;
    let repeats = st
        .query_map(params![id.0], |r| {
            Ok((
                TaskId(r.get(0)?),
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        // A rule this build cannot read is one the daemon skips as well.
        .filter_map(|(task, rule, next, active)| {
            Some(RepeatRow {
                task,
                rule: serde_json::from_str(&rule).ok()?,
                next_fire_at: next.map(dt),
                active: active != 0,
            })
        })
        .collect();

    let mut runs = Vec::new();
    for run in repo::list_runs(c, id)? {
        let name = repo::run_name(c, run.id)?;
        let mut st = c.prepare(
            "SELECT task_id, step_key, iteration, param, counts, time_limit_json, paused_at, \
             skipped, question_json, asked, answer FROM program_tasks WHERE run_id = ?1 ORDER BY task_id",
        )?;
        let tasks = st
            .query_map(params![run.id.0], |r| {
                let limit: Option<String> = r.get(5)?;
                let question: Option<String> = r.get(8)?;
                Ok(RunTaskRow {
                    task: TaskId(r.get(0)?),
                    step: r.get(1)?,
                    iteration: r.get::<_, i64>(2)?.max(1) as u32,
                    param: r.get(3)?,
                    counts: r.get::<_, i64>(4)? != 0,
                    time_limit: limit.and_then(|j| serde_json::from_str(&j).ok()),
                    paused_at: r.get::<_, Option<i64>>(6)?.map(dt),
                    skipped: r.get::<_, i64>(7)? != 0,
                    question: question.and_then(|j| serde_json::from_str(&j).ok()),
                    asked: r.get::<_, i64>(9)? != 0,
                    answer: r.get(10)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        runs.push(RunBundle { run, name, tasks });
    }
    runs.sort_by_key(|r| r.run.id);

    let mut st = c.prepare(
        "SELECT a.task_id, a.uid, a.printed_at FROM task_autoprint a \
         JOIN tasks t ON t.id = a.task_id WHERE t.board_id = ?1 ORDER BY a.task_id, a.uid",
    )?;
    let autoprinted = st
        .query_map(params![id.0], |r| {
            Ok(AutoprintRow {
                task: TaskId(r.get(0)?),
                uid: r.get::<_, i64>(1)? as Uid,
                printed_at: dt(r.get(2)?),
            })
        })?
        .collect::<Result<_, _>>()?;

    let mut st = c.prepare(
        "SELECT s.task_id, s.uid, s.kind, s.anchor_at, s.sent_at FROM reminders_sent s \
         JOIN tasks t ON t.id = s.task_id WHERE t.board_id = ?1 \
         ORDER BY s.task_id, s.uid, s.kind, s.anchor_at",
    )?;
    let reminders_sent = st
        .query_map(params![id.0], |r| {
            Ok(SentRow {
                task: TaskId(r.get(0)?),
                uid: r.get::<_, i64>(1)? as Uid,
                kind: r.get(2)?,
                anchor_at: dt(r.get(3)?),
                sent_at: dt(r.get(4)?),
            })
        })?
        .collect::<Result<_, _>>()?;

    let mut st = c.prepare(
        "SELECT task_id, actor_uid, detail_json, at FROM events WHERE board_id = ?1 ORDER BY id",
    )?;
    let events = st
        .query_map(params![id.0], |r| {
            Ok((
                r.get::<_, Option<i64>>(0)?.map(TaskId),
                r.get::<_, Option<i64>>(1)?.map(|u| u as Uid),
                r.get::<_, String>(2)?,
                dt(r.get(3)?),
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|(task, actor_uid, detail, at)| {
            Some(EventRow {
                task,
                actor_uid,
                kind: serde_json::from_str(&detail).ok()?,
                at,
            })
        })
        .collect();

    Ok(BoardBundle {
        board,
        templates,
        programs,
        tasks,
        repeats,
        runs,
        autoprinted,
        reminders_sent,
        events,
    })
}

/// Old id to new, for everything an import hands a fresh id.
#[derive(Default)]
struct Maps {
    boards: HashMap<BoardId, BoardId>,
    columns: HashMap<ColumnId, ColumnId>,
    tasks: HashMap<TaskId, TaskId>,
    templates: HashMap<TemplateId, TemplateId>,
    programs: HashMap<ProgramId, ProgramId>,
    runs: HashMap<RunId, RunId>,
}

/// Bring in a board file, or a whole server. Every board arrives as a new
/// one with fresh ids; nothing already here is touched, and a board whose
/// name is taken stops the import before anything is written, so running
/// it twice cannot double anything. `whole_server` is `--import-all`: the
/// file has to be a full export, and its accounts arrive as they were,
/// admins included. A board import makes plain accounts for the people it
/// names, so their names show, and only once somebody has logged in here.
fn import_everything(
    c: &Connection,
    text: &str,
    whole_server: bool,
) -> Result<Vec<String>, AppError> {
    let file: Everything = serde_json::from_str(text)
        .map_err(|e| AppError::bad(format!("this is not a board or server file: {e}")))?;
    let kinds: &[&str] = if whole_server {
        &["everything"]
    } else {
        &["board", "everything"]
    };
    file.header.check(kinds)?;
    let mut notes = import_people(c, &file.users, whole_server)?;
    let mut m = Maps::default();
    for b in &file.boards {
        import_board_rows(c, b, &mut m, &mut notes)?;
    }
    import_links(c, &file.boards, &mut m, &mut notes)?;
    Ok(notes)
}

fn import_people(
    c: &Connection,
    people: &[Person],
    whole_server: bool,
) -> Result<Vec<String>, AppError> {
    let accounts: Vec<(&Person, &Account)> = people
        .iter()
        .filter_map(|p| p.account.as_ref().map(|a| (p, a)))
        .collect();
    if accounts.is_empty() {
        return Ok(Vec::new());
    }
    let existing: i64 = c.query_row("SELECT count(*) FROM users", [], |r| r.get(0))?;
    if !whole_server && existing == 0 {
        // The first person to log in becomes the admin; rows made here
        // would take that from them.
        return Ok(vec![format!(
            "no accounts made for the {} people in the file: nobody has logged in here yet, \
             and the first login has to stay the admin",
            accounts.len()
        )]);
    }
    let mut made = 0;
    for (p, a) in &accounts {
        made += c.execute(
            "INSERT OR IGNORE INTO users (uid, username, is_admin, timezone, prefs_json, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                i64::from(p.uid),
                p.username,
                (whole_server && a.is_admin) as i64,
                a.timezone,
                serde_json::to_string(&a.prefs)?,
                ts(a.created_at)
            ],
        )?;
    }
    Ok(vec![format!(
        "{made} account(s) made{}, {} already here; PINs do not travel, people set them again",
        if whole_server { "" } else { ", none of them admins" },
        accounts.len() - made
    )])
}

/// The board itself and the rows that hang off it alone: columns, members,
/// templates, programs, tasks with their assignees. What ties tasks to each
/// other and to runs comes in [`import_links`], once every task has an id.
fn import_board_rows(
    c: &Connection,
    b: &BoardBundle,
    m: &mut Maps,
    notes: &mut Vec<String>,
) -> Result<(), AppError> {
    let board = &b.board;
    if let Some(id) = repo::boards_named(c, &board.name)?.first() {
        return Err(AppError::bad(format!(
            "a board called {:?} is here already (id {id}); rename or delete it first, or change \
             the name in the file, so nothing arrives twice",
            board.name
        )));
    }
    c.execute(
        "INSERT INTO boards (name, owner_uid, is_locked, is_private, archive_after_secs, created_at, \
         purge_deleted_after_secs, card_fields_json, description) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            board.name,
            i64::from(board.owner_uid),
            board.is_locked as i64,
            board.is_private as i64,
            board.archive_after_secs,
            ts(board.created_at),
            board.purge_deleted_after_secs,
            serde_json::to_string(&board.card_fields)?,
            board.description
        ],
    )?;
    let new_board = BoardId(c.last_insert_rowid());
    m.boards.insert(board.id, new_board);
    for col in &board.columns {
        c.execute(
            "INSERT INTO columns (board_id, name, position, sort_by_due) VALUES (?1, ?2, ?3, ?4)",
            params![new_board.0, col.name, col.position, col.sort_by_due as i64],
        )?;
        m.columns.insert(col.id, ColumnId(c.last_insert_rowid()));
    }
    let role = |col: ColumnId, what: &str| {
        m.columns.get(&col).copied().ok_or_else(|| {
            AppError::bad(format!(
                "board {:?}: its {what} column is not among its columns; the file is damaged",
                board.name
            ))
        })
    };
    let roles = (
        role(board.started_col, "started")?,
        role(board.paused_col, "paused")?,
        role(board.finished_col, "finished")?,
    );
    c.execute(
        "UPDATE boards SET started_col = ?2, paused_col = ?3, finished_col = ?4 WHERE id = ?1",
        params![new_board.0, roles.0.0, roles.1.0, roles.2.0],
    )?;
    let mut members = board.members.clone();
    if !members.contains(&board.owner_uid) {
        members.insert(0, board.owner_uid);
    }
    for uid in members {
        c.execute(
            "INSERT OR IGNORE INTO board_members (board_id, uid, added_at) VALUES (?1, ?2, ?3)",
            params![new_board.0, i64::from(uid), ts(board.created_at)],
        )?;
    }

    // Templates twice: the rows, then the dependencies among them.
    for t in &b.templates {
        let bare = TemplateOptions {
            dep_templates: Vec::new(),
            ..t.options.clone()
        };
        c.execute(
            "INSERT INTO templates (board_id, owner_uid, name, payload_json, options_json) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                new_board.0,
                i64::from(t.owner_uid),
                t.name,
                serde_json::to_string(&t.draft)?,
                serde_json::to_string(&bare)?
            ],
        )?;
        m.templates.insert(t.id, TemplateId(c.last_insert_rowid()));
    }
    let mut lost_deps = 0;
    for t in &b.templates {
        if t.options.dep_templates.is_empty() {
            continue;
        }
        let deps: Vec<TemplateId> = t
            .options
            .dep_templates
            .iter()
            .filter_map(|d| m.templates.get(d).copied())
            .collect();
        lost_deps += t.options.dep_templates.len() - deps.len();
        let options = TemplateOptions {
            dep_templates: deps,
            ..t.options.clone()
        };
        c.execute(
            "UPDATE templates SET options_json = ?2 WHERE id = ?1",
            params![m.templates[&t.id].0, serde_json::to_string(&options)?],
        )?;
    }

    for p in &b.programs {
        c.execute(
            "INSERT INTO programs (board_id, owner_uid, name, description, steps_json) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                new_board.0,
                i64::from(p.owner_uid),
                p.name,
                p.description,
                serde_json::to_string(&p.steps)?
            ],
        )?;
        m.programs.insert(p.id, ProgramId(c.last_insert_rowid()));
    }

    let mut renamed = 0;
    for t in &b.tasks {
        let column = m.columns.get(&t.column_id).copied().ok_or_else(|| {
            AppError::bad(format!(
                "task {}: its column is not among the board's columns; the file is damaged",
                t.short_id
            ))
        })?;
        let taken: i64 = c.query_row(
            "SELECT count(*) FROM tasks WHERE short_id = ?1",
            params![t.short_id.as_str()],
            |r| r.get(0),
        )?;
        // A short id is what is printed on paper, so it is kept when it can
        // be; a clash gets a fresh one and the old slips stop scanning.
        let short = if taken == 0 {
            t.short_id
        } else {
            renamed += 1;
            repo::fresh_short_id(c)?
        };
        c.execute(
            "INSERT INTO tasks (short_id, board_id, column_id, position, title, description, start_at, \
             due_at, created_by, created_at, finished_at, version, archived_at, archived_from_col, \
             deleted_at, checklist, reminder_start_minutes, reminder_due_minutes, template_id, \
             exclude_from_stats, print_rules, auto_start) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
             ?19, ?20, ?21, ?22)",
            params![
                short.as_str(),
                new_board.0,
                column.0,
                t.position,
                t.title,
                t.description,
                t.start_at.map(ts),
                t.due_at.map(ts),
                i64::from(t.created_by),
                ts(t.created_at),
                t.finished_at.map(ts),
                t.version as i64,
                t.archived_at.map(ts),
                t.archived_from_col.and_then(|c| m.columns.get(&c)).map(|c| c.0),
                t.deleted_at.map(ts),
                serde_json::to_string(&t.checklist)?,
                t.reminder_start_minutes.map(i64::from),
                t.reminder_due_minutes.map(i64::from),
                t.template_id.and_then(|id| m.templates.get(&id)).map(|id| id.0),
                t.exclude_from_stats as i64,
                serde_json::to_string(&t.print_rules)?,
                t.auto_start as i64
            ],
        )?;
        let new_task = TaskId(c.last_insert_rowid());
        m.tasks.insert(t.id, new_task);
        for uid in &t.assignees {
            c.execute(
                "INSERT OR IGNORE INTO assignees (task_id, uid) VALUES (?1, ?2)",
                params![new_task.0, i64::from(*uid)],
            )?;
        }
    }
    notes.push(format!(
        "board {:?} (id {new_board}): {} columns, {} templates, {} programs, {} tasks{}",
        board.name,
        board.columns.len(),
        b.templates.len(),
        b.programs.len(),
        b.tasks.len(),
        if renamed > 0 {
            format!(", {renamed} of them given new short ids because theirs were taken")
        } else {
            String::new()
        }
    ));
    if lost_deps > 0 {
        notes.push(format!(
            "board {:?}: {lost_deps} template dependency(ies) pointed outside the file and were dropped",
            board.name
        ));
    }
    Ok(())
}

/// What refers to tasks by id: dependencies, repetitions, runs and their
/// task links, print bookkeeping, and the history last, since it names
/// runs too. A reference to something the file does not hold is dropped and
/// counted, never guessed.
fn import_links(
    c: &Connection,
    boards: &[BoardBundle],
    m: &mut Maps,
    notes: &mut Vec<String>,
) -> Result<(), AppError> {
    let (mut deps, mut lost_deps, mut repeats, mut runs, mut lost_runs, mut links) = (0, 0, 0, 0, 0, 0);
    for b in boards {
        let new_board = m.boards[&b.board.id];
        for t in &b.tasks {
            let Some(task) = m.tasks.get(&t.id) else { continue };
            for d in &t.depends_on {
                match m.tasks.get(d) {
                    Some(dep) => {
                        c.execute(
                            "INSERT OR IGNORE INTO deps (task_id, depends_on_task_id) VALUES (?1, ?2)",
                            params![task.0, dep.0],
                        )?;
                        deps += 1;
                    }
                    None => lost_deps += 1,
                }
            }
        }
        for r in &b.repeats {
            let Some(task) = m.tasks.get(&r.task) else { continue };
            c.execute(
                "INSERT OR REPLACE INTO repeats (task_id, rule_json, next_fire_at, active) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    task.0,
                    serde_json::to_string(&r.rule)?,
                    r.next_fire_at.map(ts),
                    r.active as i64
                ],
            )?;
            repeats += 1;
        }
        // A run remembers the column its root was made in, which may have
        // been removed since; the started column stands in for it then.
        let started: i64 = c.query_row(
            "SELECT started_col FROM boards WHERE id = ?1",
            params![new_board.0],
            |r| r.get(0),
        )?;
        for rb in &b.runs {
            let run = &rb.run;
            let Some(root) = m.tasks.get(&run.root_task) else {
                lost_runs += 1;
                continue;
            };
            let column = m.columns.get(&run.column_id).map(|c| c.0).unwrap_or(started);
            c.execute(
                "INSERT INTO program_runs (program_id, board_id, root_task_id, started_by, column_id, \
                 name, steps_json, created_at, started_at, finished_at, cancelled_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    run.program_id.and_then(|p| m.programs.get(&p)).map(|p| p.0),
                    new_board.0,
                    root.0,
                    i64::from(run.started_by),
                    column,
                    rb.name,
                    serde_json::to_string(&run.steps)?,
                    ts(run.created_at),
                    run.started_at.map(ts),
                    run.finished_at.map(ts),
                    run.cancelled_at.map(ts)
                ],
            )?;
            let new_run = RunId(c.last_insert_rowid());
            m.runs.insert(run.id, new_run);
            runs += 1;
            for l in &rb.tasks {
                let Some(task) = m.tasks.get(&l.task) else { continue };
                c.execute(
                    "INSERT OR IGNORE INTO program_tasks (task_id, run_id, step_key, iteration, param, \
                     counts, time_limit_json, paused_at, skipped, question_json, asked, answer) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        task.0,
                        new_run.0,
                        l.step,
                        i64::from(l.iteration),
                        l.param,
                        l.counts as i64,
                        l.time_limit.map(|o| serde_json::to_string(&o)).transpose()?,
                        l.paused_at.map(ts),
                        l.skipped as i64,
                        l.question.as_ref().map(serde_json::to_string).transpose()?,
                        l.asked as i64,
                        l.answer
                    ],
                )?;
                links += 1;
            }
        }
        for a in &b.autoprinted {
            if let Some(task) = m.tasks.get(&a.task) {
                c.execute(
                    "INSERT OR IGNORE INTO task_autoprint (task_id, uid, printed_at) VALUES (?1, ?2, ?3)",
                    params![task.0, i64::from(a.uid), ts(a.printed_at)],
                )?;
            }
        }
        for s in &b.reminders_sent {
            if let Some(task) = m.tasks.get(&s.task) {
                c.execute(
                    "INSERT OR IGNORE INTO reminders_sent (task_id, uid, kind, anchor_at, sent_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![task.0, i64::from(s.uid), s.kind, ts(s.anchor_at), ts(s.sent_at)],
                )?;
            }
        }
    }
    let mut events = 0;
    for b in boards {
        let new_board = m.boards[&b.board.id];
        for e in &b.events {
            let kind = remap_event(e.kind.clone(), m);
            let (from, to) = kind.columns();
            c.execute(
                "INSERT INTO events (task_id, board_id, actor_uid, kind, from_col, to_col, detail_json, at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    e.task.and_then(|t| m.tasks.get(&t)).map(|t| t.0),
                    new_board.0,
                    e.actor_uid.map(i64::from),
                    kind.name(),
                    from.map(|c| c.0),
                    to.map(|c| c.0),
                    serde_json::to_string(&kind)?,
                    ts(e.at)
                ],
            )?;
            events += 1;
        }
    }
    notes.push(format!(
        "{deps} dependencies, {repeats} repetitions, {runs} runs linking {links} tasks, {events} history entries"
    ));
    if lost_deps > 0 || lost_runs > 0 {
        notes.push(format!(
            "dropped: {lost_deps} dependencies on tasks not in the file, {lost_runs} runs whose root task is not in the file"
        ));
    }
    Ok(())
}

/// The ids a history entry names, moved to their new values. One the file
/// does not hold is left as it was: the entry still reads, it just points
/// at nothing, which is what a purged task's history does anyway.
fn remap_event(kind: EventKind, m: &Maps) -> EventKind {
    use EventKind::*;
    let col = |c: ColumnId| m.columns.get(&c).copied().unwrap_or(c);
    let task = |t: TaskId| m.tasks.get(&t).copied().unwrap_or(t);
    let tpl = |t: TemplateId| m.templates.get(&t).copied().unwrap_or(t);
    let prog = |p: ProgramId| m.programs.get(&p).copied().unwrap_or(p);
    let run_id = |r: RunId| m.runs.get(&r).copied().unwrap_or(r);
    match kind {
        TaskCreated { column } => TaskCreated {
            column: column.map(col),
        },
        TaskMoved { from, to } => TaskMoved {
            from: col(from),
            to: col(to),
        },
        TaskArchived { from } => TaskArchived { from: col(from) },
        TaskRestored { to } => TaskRestored { to: col(to) },
        DependencyOverridden { open } => DependencyOverridden {
            open: open.into_iter().map(task).collect(),
        },
        RepeatSpawned { from_task } => RepeatSpawned {
            from_task: task(from_task),
        },
        ColumnAdded { column } => ColumnAdded {
            column: col(column),
        },
        ColumnRemoved {
            column,
            tasks_moved_to,
        } => ColumnRemoved {
            column: col(column),
            tasks_moved_to: tasks_moved_to.map(col),
        },
        ColumnRenamed { column } => ColumnRenamed {
            column: col(column),
        },
        MemberRemoved { uid, unassigned } => MemberRemoved {
            uid,
            unassigned: unassigned.into_iter().map(task).collect(),
        },
        TemplateCreated { template } => TemplateCreated {
            template: tpl(template),
        },
        TemplateChanged { template } => TemplateChanged {
            template: tpl(template),
        },
        TemplateDeleted { template } => TemplateDeleted {
            template: tpl(template),
        },
        ProgramCreated { program } => ProgramCreated {
            program: prog(program),
        },
        ProgramChanged { program } => ProgramChanged {
            program: prog(program),
        },
        ProgramDeleted { program } => ProgramDeleted {
            program: prog(program),
        },
        RunStarted { run } => RunStarted { run: run_id(run) },
        RunCancelled { run } => RunCancelled { run: run_id(run) },
        other => other,
    }
}

/// Who this server knows: everyone who has logged in, and everyone in the
/// daemon's group who has not.
fn users_table(c: &Connection, locals: &[Identity]) -> Result<String, AppError> {
    // uid -> (username, admin if they have an account, in the group)
    let mut rows: BTreeMap<Uid, (String, Option<bool>, bool)> = BTreeMap::new();
    for u in repo::list_users(c)? {
        rows.insert(u.uid, (u.username, Some(u.is_admin), false));
    }
    for i in locals {
        let row = rows
            .entry(i.uid)
            .or_insert_with(|| (i.username.clone(), None, false));
        row.2 = true;
    }
    if rows.is_empty() {
        return Ok("nobody yet: no logins, and the daemon's group is empty\n".into());
    }
    let width = rows.values().map(|r| r.0.len()).max().unwrap_or(0).max(8);
    let mut s = format!("{:>7}  {:<width$}  {:<5}  {}\n", "uid", "username", "admin", "status");
    for (uid, (name, admin, in_group)) in &rows {
        let status = match (admin, in_group) {
            (Some(_), true) => "logged in",
            (Some(_), false) => "logged in, no longer in the group",
            (None, _) => "in the group, never logged in",
        };
        let admin = if *admin == Some(true) { "yes" } else { "no" };
        s += &format!("{uid:>7}  {name:<width$}  {admin:<5}  {status}\n");
    }
    Ok(s)
}

/// Rewrite the uids in a file to this server's by username: alice was 1234
/// there and is 1000 here, so every 1234 becomes 1000. The file's own
/// `users` list is the only place the names come from.
fn remap_users(
    c: &Connection,
    locals: &[Identity],
    text: &str,
) -> Result<(String, Vec<String>), AppError> {
    let mut v: Value = serde_json::from_str(text)
        .map_err(|e| AppError::bad(format!("this is not a JSON file: {e}")))?;
    let people: Vec<Person> = match v.get("users") {
        Some(list) => serde_json::from_value(list.clone())
            .map_err(|e| AppError::bad(format!("the file's users list is not readable: {e}")))?,
        None => Vec::new(),
    };
    if people.is_empty() {
        return Err(AppError::bad(
            "this file names nobody, so there are no usernames to match; only files written by \
             Taskologic 0.1.12 or later carry them",
        ));
    }
    let mut local: HashMap<String, Uid> = HashMap::new();
    for u in repo::list_users(c)? {
        local.insert(u.username, u.uid);
    }
    // The system's word on who is which uid beats a stale login row.
    for i in locals {
        local.insert(i.username.clone(), i.uid);
    }
    let mut map = HashMap::new();
    let mut notes = Vec::new();
    for p in &people {
        let found = local.get(&p.username).copied().or_else(|| {
            local
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(&p.username))
                .map(|(_, u)| *u)
        });
        match found {
            Some(new) if new == p.uid => {
                notes.push(format!("{}: uid {} here as well", p.username, p.uid));
            }
            Some(new) => {
                map.insert(p.uid, new);
                notes.push(format!("{}: {} -> {new}", p.username, p.uid));
            }
            None => notes.push(format!(
                "{}: nobody called that here, uid {} left as it is",
                p.username, p.uid
            )),
        }
    }
    let changed = remap_uids(&mut v, &map);
    notes.push(format!("{changed} uid(s) rewritten"));
    Ok((serde_json::to_string_pretty(&v)?, notes))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use taskologic_core::board::{DEFAULT_ARCHIVE_AFTER_SECS, DEFAULT_PURGE_DELETED_AFTER_SECS};
    use taskologic_core::offset::OffsetUnit;
    use taskologic_core::print::{PrintWhen, SlipKind};
    use chrono::NaiveTime;
    use taskologic_core::program::{ROOT_KEY, Step, Trigger};
    use taskologic_core::repeat::Repeat;
    use taskologic_proto::CreateBoard;

    fn db_with_boards() -> (Db, Board, Board) {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            repo::upsert_user_on_login(c, 1, "alice", chrono_tz::UTC, true, Utc::now())?;
            repo::upsert_user_on_login(c, 2, "bob", chrono_tz::UTC, false, Utc::now())?;
            Ok(())
        })
        .unwrap();
        let make = |name: &str, members: &[Uid]| {
            db.tx(|c| {
                repo::create_board(
                    c,
                    1,
                    &CreateBoard {
                        name: name.into(),
                        description: String::new(),
                        columns: vec!["Todo".into(), "Doing".into(), "Wait".into(), "Done".into()],
                        started_col: 1,
                        paused_col: 2,
                        finished_col: 3,
                        archive_after_secs: DEFAULT_ARCHIVE_AFTER_SECS,
                        purge_deleted_after_secs: DEFAULT_PURGE_DELETED_AFTER_SECS,
                        card_fields: Default::default(),
                        is_private: true,
                        is_locked: false,
                        members: members.to_vec(),
                    },
                    Utc::now(),
                )
            })
            .unwrap()
        };
        let from = make("From", &[1, 2]);
        let to = make("To", &[1]);
        (db, from, to)
    }

    #[test]
    fn a_program_round_trips_and_loses_only_the_people_the_other_board_lacks() {
        let (db, from, to) = db_with_boards();
        let draft = ProgramDraft {
            name: "Clean up".into(),
            description: "the flat".into(),
            steps: vec![
                Step {
                    key: ROOT_KEY.into(),
                    title: "Clean".into(),
                    ..Default::default()
                },
                Step {
                    key: "1".into(),
                    title: "Wash".into(),
                    created: vec![Trigger::WithRoot],
                    assign: Assign::Users(vec![2]),
                    print: vec![PrintRule {
                        when: PrintWhen::OnStart,
                        slip: SlipKind::Task,
                        to: Recipients::Users(vec![2]),
                    }],
                    ..Default::default()
                },
            ],
        };
        db.tx(|c| repo::create_program(c, from.id, 1, &draft)).unwrap();
        let json = db.with(|c| export_program(c, "from", "clean up", &[])).unwrap();
        assert!(json.contains("\"kind\": \"program\""), "{json}");

        let notes = db.tx(|c| import_program(c, "To", &json, 1)).unwrap();
        assert!(notes.iter().any(|n| n.contains("dropped 1 assignee")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("imported program")), "{notes:?}");
        let imported = db.with(|c| repo::list_programs(c, to.id)).unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].steps[1].assign, Assign::Starter, "bob is not on To");
        assert_eq!(imported[0].steps[1].print[0].to, Recipients::Assignees);
        assert_eq!(imported[0].owner_uid, 1);

        // The wrong kind of file, and a board that does not exist, say so.
        let err = db.tx(|c| import_template(c, "To", &json, 1)).unwrap_err();
        assert!(err.to_string().contains("not a template"), "{err}");
        let err = db.with(|c| export_program(c, "Nowhere", "x", &[])).unwrap_err();
        assert!(err.to_string().contains("no board called"), "{err}");
    }

    #[test]
    fn a_template_travels_with_what_it_depends_on_and_reuses_what_is_there() {
        let (db, from, to) = db_with_boards();
        let plain = |title: &str| TaskDraft {
            title: title.into(),
            ..Default::default()
        };
        let (soap, sink, wash) = db
            .tx(|c| {
                let soap = repo::create_template(
                    c,
                    from.id,
                    1,
                    "Buy soap",
                    &plain("Buy soap"),
                    &TemplateOptions {
                        due_prefill: Some(Offset {
                            amount: 2,
                            unit: OffsetUnit::Hours,
                        }),
                        ..Default::default()
                    },
                )?;
                let sink = repo::create_template(
                    c,
                    from.id,
                    1,
                    "Fill sink",
                    &plain("Fill sink"),
                    &TemplateOptions::default(),
                )?;
                let wash = repo::create_template(
                    c,
                    from.id,
                    1,
                    "Wash up",
                    &plain("Wash up"),
                    &TemplateOptions {
                        dep_templates: vec![soap.id, sink.id],
                        ..Default::default()
                    },
                )?;
                Ok((soap, sink, wash))
            })
            .unwrap();
        // The target board already has its own "Fill sink".
        db.tx(|c| {
            repo::create_template(c, to.id, 1, "Fill sink", &plain("Fill the sink"), &Default::default())
        })
        .unwrap();

        let json = db.with(|c| export_template(c, "From", "Wash up", &[])).unwrap();
        let file: TemplateFile = serde_json::from_str(&json).unwrap();
        assert_eq!(file.root, "Wash up");
        assert_eq!(
            file.templates.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["Buy soap", "Fill sink", "Wash up"],
            "dependencies first"
        );
        assert_eq!(file.templates[2].depends_on, vec!["Buy soap", "Fill sink"]);

        let notes = db.tx(|c| import_template(c, "To", &json, 2)).unwrap();
        assert!(notes.iter().any(|n| n.contains("\"Fill sink\" is on this board already")), "{notes:?}");
        let on_to = db.with(|c| repo::list_templates(c, to.id)).unwrap();
        assert_eq!(on_to.len(), 3, "soap and wash came, the sink was already there");
        let wash_to = on_to.iter().find(|t| t.name == "Wash up").unwrap();
        let soap_to = on_to.iter().find(|t| t.name == "Buy soap").unwrap();
        let sink_to = on_to.iter().find(|t| t.name == "Fill sink").unwrap();
        assert_eq!(sink_to.draft.title, "Fill the sink", "the board's own, untouched");
        let mut deps = wash_to.options.dep_templates.clone();
        deps.sort();
        let mut want = vec![soap_to.id, sink_to.id];
        want.sort();
        assert_eq!(deps, want, "remapped to this board's ids");
        assert_eq!(soap_to.options.due_prefill.map(|o| o.amount), Some(2));
        assert_eq!(wash_to.owner_uid, 2);
        // Nothing on the source board moved.
        let on_from = db.with(|c| repo::list_templates(c, from.id)).unwrap();
        assert_eq!(on_from.len(), 3);
        assert!(on_from.iter().any(|t| t.id == wash.id && t.options.dep_templates == vec![soap.id, sink.id]));
    }

    fn identity(uid: Uid, username: &str) -> Identity {
        Identity {
            uid,
            username: username.into(),
        }
    }

    #[test]
    fn only_admins_and_root_get_in() {
        let (db, ..) = db_with_boards();
        let mut out = Vec::new();
        let err = run(&db, Command::Users, 2, &[], &mut out).unwrap_err();
        assert!(err.to_string().contains("bob (uid 2) is not an admin"), "{err}");
        let err = run(&db, Command::Users, 999, &[], &mut out).unwrap_err();
        assert!(err.to_string().contains("never logged in"), "{err}");
        run(&db, Command::Users, 1, &[], &mut out).unwrap();
        run(&db, Command::Users, 0, &[identity(3, "carol")], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("alice") && text.contains("yes"), "{text}");
        assert!(text.contains("carol") && text.contains("never logged in"), "{text}");
    }

    /// A board with one of everything on it, for the round trips.
    fn seed(db: &Db, from: &Board) -> (Task, Task, Program) {
        db.tx(|c| {
            let now = Utc::now();
            let soap = repo::create_template(
                c,
                from.id,
                1,
                "Buy soap",
                &TaskDraft {
                    title: "Buy soap".into(),
                    ..Default::default()
                },
                &TemplateOptions::default(),
            )?;
            repo::create_template(
                c,
                from.id,
                1,
                "Wash up",
                &TaskDraft {
                    title: "Wash up".into(),
                    ..Default::default()
                },
                &TemplateOptions {
                    dep_templates: vec![soap.id],
                    ..Default::default()
                },
            )?;
            let program = repo::create_program(
                c,
                from.id,
                1,
                &ProgramDraft {
                    name: "Clean up".into(),
                    description: String::new(),
                    steps: vec![Step {
                        key: ROOT_KEY.into(),
                        title: "Clean".into(),
                        ..Default::default()
                    }],
                },
            )?;
            let first = repo::create_task(
                c,
                from,
                from.columns[0].id,
                &TaskDraft {
                    title: "First".into(),
                    assignees: vec![1, 2],
                    repeat: Some(RepeatSpec {
                        rule: Repeat::DayOfMonth { day: 5 },
                        at: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                        tz: chrono_tz::UTC,
                        start_rule: None,
                        due_rule: None,
                    }),
                    ..Default::default()
                },
                1,
                Some(soap.id),
                now,
            )?;
            let second = repo::create_task(
                c,
                from,
                from.columns[1].id,
                &TaskDraft {
                    title: "Second".into(),
                    depends_on: vec![first.id],
                    ..Default::default()
                },
                2,
                None,
                now,
            )?;
            let run = repo::create_run(c, &program, first.id, 1, from.columns[0].id, now)?;
            repo::link_task(c, run.id, first.id, ROOT_KEY, 1, None, true, None, None)?;
            repo::record_event(
                c,
                from.id,
                Some(first.id),
                Some(1),
                &EventKind::RunStarted { run: run.id },
                now,
            )?;
            repo::mark_autoprinted(c, first.id, 1, now)?;
            repo::mark_slip_sent(c, first.id, 2, "rule_start_task", now, now)?;
            Ok((first, second, program))
        })
        .unwrap()
    }

    #[test]
    fn a_whole_server_round_trips_with_fresh_ids() {
        let (db, from, to) = db_with_boards();
        let (first, second, program) = seed(&db, &from);
        let json = db.with(|c| export_everything(c, None, &[])).unwrap();
        let file: Everything = serde_json::from_str(&json).unwrap();
        assert_eq!(file.header.kind, "everything");
        assert_eq!(file.users.len(), 2, "every account, with its details");
        assert!(file.users.iter().all(|p| p.account.is_some()));
        assert_eq!(file.boards.len(), 2);
        let bundle = &file.boards[0];
        assert_eq!(bundle.tasks.len(), 2);
        assert_eq!(bundle.repeats.len(), 1);
        assert_eq!(bundle.runs.len(), 1);
        assert_eq!(bundle.autoprinted.len(), 1);
        assert_eq!(bundle.reminders_sent.len(), 1);
        assert!(bundle.events.len() >= 4, "{}", bundle.events.len());

        // Importing onto a server that has these boards is refused whole.
        let err = db.tx(|c| import_everything(c, &json, true)).unwrap_err();
        assert!(err.to_string().contains("\"From\" is here already"), "{err}");

        // Out of the way, the same file comes back as new boards.
        db.tx(|c| {
            c.execute("UPDATE boards SET name = name || ' (old)'", [])?;
            Ok(())
        })
        .unwrap();
        let notes = db.tx(|c| import_everything(c, &json, true)).unwrap();
        assert!(notes.iter().any(|n| n.contains("0 account(s) made, 2 already here")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("2 tasks, 2 of them given new short ids")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("1 dependencies, 1 repetitions, 1 runs linking 1 tasks")), "{notes:?}");

        let new_from = db.with(|c| board_named(c, "From")).unwrap();
        let new_to = db.with(|c| board_named(c, "To")).unwrap();
        assert!(new_from.id != from.id && new_to.id != to.id);
        assert_eq!(new_from.members, vec![1, 2]);
        assert_eq!(new_from.started_col, new_from.columns[1].id, "roles point at the new columns");
        let tasks = db.with(|c| repo::list_tasks(c, new_from.id, false)).unwrap();
        assert_eq!(tasks.len(), 2);
        let new_first = tasks.iter().find(|t| t.title == "First").unwrap();
        let new_second = tasks.iter().find(|t| t.title == "Second").unwrap();
        assert!(new_first.short_id != first.short_id, "the old one still has it");
        assert_eq!(new_first.assignees, vec![1, 2]);
        assert_eq!(new_first.column_id, new_from.columns[0].id);
        assert_eq!(new_second.depends_on, vec![new_first.id], "remapped");
        assert!(new_first.repeat.is_some());
        assert_eq!(new_first.created_at, first.created_at, "history keeps its dates");
        let templates = db.with(|c| repo::list_templates(c, new_from.id)).unwrap();
        let soap = templates.iter().find(|t| t.name == "Buy soap").unwrap();
        let wash = templates.iter().find(|t| t.name == "Wash up").unwrap();
        assert_eq!(wash.options.dep_templates, vec![soap.id]);
        assert_eq!(new_first.template_id, Some(soap.id));
        let programs = db.with(|c| repo::list_programs(c, new_from.id)).unwrap();
        assert_eq!(programs.len(), 1);
        assert!(programs[0].id != program.id);
        let runs = db.with(|c| repo::list_runs(c, new_from.id)).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].root_task, new_first.id);
        assert_eq!(runs[0].program_id, Some(programs[0].id));
        let link = db.with(|c| repo::task_link(c, new_first.id)).unwrap().unwrap();
        assert_eq!((link.run, link.step.as_str()), (runs[0].id, ROOT_KEY));
        assert_eq!(db.with(|c| repo::autoprinted_uids(c, new_first.id)).unwrap(), vec![1]);
        assert!(db.with(|c| repo::slip_sent(c, new_first.id, 2, "rule_start_task", first.created_at)).unwrap());
        let history = db.with(|c| repo::task_history(c, new_first.id)).unwrap();
        assert_eq!(history[0].1.as_deref(), Some("alice"));
        assert_eq!(
            history[0].0.kind,
            EventKind::TaskCreated {
                column: Some(new_from.columns[0].id)
            },
            "the column inside the entry moved too"
        );
        assert!(history.iter().any(|(e, _)| e.kind == EventKind::RunStarted { run: runs[0].id }));
        // The old boards kept everything.
        assert_eq!(db.with(|c| repo::list_tasks(c, from.id, false)).unwrap().len(), 2);
        let _ = second;
    }

    #[test]
    fn a_board_file_names_its_people_and_remap_rewrites_them() {
        let (db, from, _to) = db_with_boards();
        seed(&db, &from);
        let carol = [identity(3, "carol")];
        // The other board never mentions bob, so its file does not either.
        let only_alice: Everything =
            serde_json::from_str(&db.with(|c| export_everything(c, Some("To"), &carol)).unwrap()).unwrap();
        assert_eq!(only_alice.header.kind, "board");
        assert_eq!(only_alice.header.board.as_deref(), Some("To"));
        assert_eq!(only_alice.users.iter().map(|p| p.uid).collect::<Vec<_>>(), vec![1]);

        let json = db.with(|c| export_everything(c, Some("from"), &carol)).unwrap();
        let original: Value = serde_json::from_str(&json).unwrap();
        let mut mentioned = HashSet::new();
        uids_in(&original, &mut mentioned);
        assert_eq!(mentioned, HashSet::from([1, 2]));

        // The same file as another server would have written it: alice was
        // 501 there, bob 502, and dave is somebody this server never had.
        let mut foreign = original.clone();
        assert_eq!(remap_uids(&mut foreign, &HashMap::from([(1, 501), (2, 502)])), {
            let mut n = HashSet::new();
            uids_in(&foreign, &mut n);
            assert_eq!(n, HashSet::from([501, 502]));
            // Every place a uid sat got rewritten, the users list included.
            foreign["users"][0]["uid"].as_u64().unwrap();
            serde_json::to_string(&original).unwrap().matches("1").count() * 0 + count_uids(&original)
        });
        foreign["users"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"uid": 503, "username": "dave"}));
        let (back, notes) = db
            .with(|c| remap_users(c, &carol, &serde_json::to_string(&foreign).unwrap()))
            .unwrap();
        assert!(notes.iter().any(|n| n == "alice: 501 -> 1"), "{notes:?}");
        assert!(notes.iter().any(|n| n == "bob: 502 -> 2"), "{notes:?}");
        assert!(notes.iter().any(|n| n.starts_with("dave: nobody called that here")), "{notes:?}");
        let mut back: Value = serde_json::from_str(&back).unwrap();
        back["users"].as_array_mut().unwrap().pop();
        assert_eq!(back, original, "the remap undid the foreign uids exactly");

        // A member removal lists task ids under `unassigned`; those are not uids.
        let mut event = serde_json::json!({"kind": "member_removed", "uid": 2, "unassigned": [2, 7]});
        remap_uids(&mut event, &HashMap::from([(2, 9)]));
        assert_eq!(event, serde_json::json!({"kind": "member_removed", "uid": 9, "unassigned": [2, 7]}));

        // Somebody known only from the group is matched by name as well.
        let mut file = serde_json::json!({"users": [{"uid": 77, "username": "carol"}], "owner_uid": 77});
        let (out, _) = db
            .with(|c| remap_users(c, &carol, &file.to_string()))
            .unwrap();
        file = serde_json::from_str(&out).unwrap();
        assert_eq!(file["owner_uid"], 3);

        // Without a users list there is nothing to match on.
        let err = db.with(|c| remap_users(c, &carol, "{}")).unwrap_err();
        assert!(err.to_string().contains("names nobody"), "{err}");

        // A board file on a server nobody has logged in to makes no accounts.
        let fresh = Db::open_in_memory().unwrap();
        let notes = fresh.tx(|c| import_everything(c, &json, false)).unwrap();
        assert!(notes.iter().any(|n| n.starts_with("no accounts made for the 2 people")), "{notes:?}");
        assert!(fresh.with(|c| repo::list_users(c)).unwrap().is_empty());
        assert_eq!(fresh.with(|c| repo::list_tasks(c, BoardId(1), false)).unwrap().len(), 2);
        // And a whole server file is what --import-all wants.
        let err = fresh.tx(|c| import_everything(c, &json, true)).unwrap_err();
        assert!(err.to_string().contains("holds a board, not a everything"), "{err}");
    }

    /// How many uid slots a file has, counting each list entry.
    fn count_uids(v: &Value) -> usize {
        let mut all = HashMap::new();
        let mut seen = HashSet::new();
        uids_in(v, &mut seen);
        for u in seen {
            all.insert(u, u + 100_000);
        }
        remap_uids(&mut v.clone(), &all)
    }
}
