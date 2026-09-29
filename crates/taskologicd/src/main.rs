//! taskologicd: owns the database, the scheduler and the event bus. The only
//! process that ever opens the SQLite file.

mod auth;
mod bus;
mod config;
mod db;
mod error;
mod handlers;
mod programs;
mod scheduler;
mod server;
mod slips;
mod state;
mod transfer;

use std::path::{Path, PathBuf};

use anyhow::Context;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::db::Db;
use crate::state::AppState;

/// What the process was asked to do. Everything but [`Mode::Serve`] prints
/// something and exits, which is what lets `scripts/update.sh` ask about the
/// database without starting a daemon that would then hold it open.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Serve,
    Version,
    DbStatus,
    Migrate,
    /// Move programs, templates, boards or the whole server in or out as
    /// JSON files, see `transfer` and `scripts/transfer.sh`.
    Transfer(transfer::Command),
}

fn usage() -> ! {
    eprintln!(
        "usage: taskologicd [--config PATH]\n\
         \n\
         one shot commands, none of which start the daemon:\n\
         \x20 --version     print the version and exit\n\
         \x20 --db-status   print the database schema version and what is pending\n\
         \x20 --migrate     back the database up, then apply pending migrations\n\
         \x20 --export-program  BOARD NAME [FILE]   write a program as JSON, to stdout without FILE\n\
         \x20 --import-program  BOARD FILE          create the program in FILE on BOARD (- reads stdin)\n\
         \x20                   [--on-clash refuse|new|replace]  when the server has the file's short id\n\
         \x20 --export-template BOARD NAME [FILE]   the same for a template and what it depends on\n\
         \x20 --import-template BOARD FILE\n\
         \x20 --export-board    NAME|all [FILE]     a board with its tasks, history, programs, repeats\n\
         \x20 --import-board    FILE                add the board(s) in FILE as new boards\n\
         \x20 --export-all      [FILE]              every account and every board, for moving servers\n\
         \x20 --import-all      FILE\n\
         \x20 --users                               who this server knows, uid by uid\n\
         \x20 --remap-users     FILE [OUT]          rewrite FILE's uids to this server's by username\n\
         all of these need root or a Taskologic admin\n\
         \n\
         environment: TASKOLOGICD_CONFIG, TASKOLOGICD_LOG (tracing filter, default info)"
    );
    std::process::exit(2)
}

fn parse_args() -> (Mode, PathBuf) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut path: Option<PathBuf> = None;
    let mut mode = Mode::Serve;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        match a {
            "--config" | "-c" => {
                path = Some(args.get(i).map(PathBuf::from).unwrap_or_else(|| usage()));
                i += 1;
            }
            "--version" | "-V" => mode = Mode::Version,
            "--db-status" => mode = Mode::DbStatus,
            "--migrate" => mode = Mode::Migrate,
            "--export-program" | "--import-program" | "--export-template" | "--import-template"
            | "--export-board" | "--import-board" | "--export-all" | "--import-all" | "--users"
            | "--remap-users" => {
                use transfer::Command;
                // So many words are needed, and one more may follow.
                let mut need = |n: usize| -> Vec<String> {
                    let words: Vec<String> = args[i..].iter().take(n).cloned().collect();
                    if words.len() < n {
                        usage();
                    }
                    i += n;
                    words
                };
                let extra = |i: &mut usize| -> Option<PathBuf> {
                    let word = args.get(*i).filter(|s| !s.starts_with("--")).cloned();
                    if word.is_some() {
                        *i += 1;
                    }
                    word.map(PathBuf::from)
                };
                mode = Mode::Transfer(match a {
                    "--export-program" | "--export-template" => {
                        let mut w = need(2);
                        let (board, name) = (w.remove(0), w.remove(0));
                        let file = extra(&mut i);
                        if a == "--export-program" {
                            Command::ExportProgram { board, name, file }
                        } else {
                            Command::ExportTemplate { board, name, file }
                        }
                    }
                    "--import-program" | "--import-template" => {
                        let mut w = need(2);
                        let (board, file) = (w.remove(0), PathBuf::from(w.remove(0)));
                        let clash = on_clash(&args, &mut i);
                        if a == "--import-program" {
                            Command::ImportProgram { board, file, clash }
                        } else {
                            Command::ImportTemplate { board, file, clash }
                        }
                    }
                    "--export-board" => {
                        let name = need(1).remove(0);
                        Command::ExportBoard {
                            name,
                            file: extra(&mut i),
                        }
                    }
                    "--import-board" => {
                        let file = PathBuf::from(need(1).remove(0));
                        Command::ImportBoard { file, clash: on_clash(&args, &mut i) }
                    }
                    "--export-all" => Command::ExportAll {
                        file: extra(&mut i),
                    },
                    "--import-all" => {
                        let file = PathBuf::from(need(1).remove(0));
                        Command::ImportAll { file, clash: on_clash(&args, &mut i) }
                    }
                    "--users" => Command::Users,
                    _ => {
                        let file = PathBuf::from(need(1).remove(0));
                        Command::RemapUsers {
                            file,
                            out: extra(&mut i),
                        }
                    }
                });
            }
            "--help" | "-h" => usage(),
            _ => usage(),
        }
    }
    let path = path
        .or_else(|| std::env::var_os("TASKOLOGICD_CONFIG").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(config::DEFAULT_PATH));
    (mode, path)
}

/// An optional `--on-clash refuse|new|replace` after an import's arguments:
/// what to do when the server already has a short id the file carries.
fn on_clash(args: &[String], i: &mut usize) -> transfer::Clash {
    if args.get(*i).map(String::as_str) == Some("--on-clash") {
        let value = args.get(*i + 1).cloned().unwrap_or_else(|| usage());
        *i += 2;
        return transfer::Clash::parse(&value).unwrap_or_else(|| usage());
    }
    transfer::Clash::default()
}

/// Who an import belongs to: whoever ran it, seen through sudo when the
/// database needed root to write.
fn invoking_uid() -> u32 {
    std::env::var("SUDO_UID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| nix::unistd::getuid().as_raw())
}

/// The schema version the file is actually at, and how many migrations this
/// binary would still apply. A negative count means the database was written
/// by a newer Taskologic than this one, which is a downgrade and refuses to
/// migrate rather than guessing.
fn db_state(path: &Path) -> anyhow::Result<(usize, i32)> {
    let conn =
        rusqlite::Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    let ms = db::migrations();
    let at = usize::from(ms.current_version(&conn)?);
    let pending = ms.pending_migrations(&conn)?;
    Ok((at, pending))
}

/// Give `path` the owner `owner` names. An upgrade is run by root but the
/// daemon is not, so without this a migration leaves root owned files in a
/// directory the daemon user has to write, and the next start fails on the
/// write ahead log. Failing to chown is reported rather than fatal: an
/// operator running this as the owning user already has it right.
fn match_owner(path: &Path, owner: (u32, u32)) {
    let (uid, gid) = owner;
    if let Err(e) = nix::unistd::chown(
        path,
        Some(nix::unistd::Uid::from_raw(uid)),
        Some(nix::unistd::Gid::from_raw(gid)),
    ) {
        eprintln!(
            "warning: could not give {} to {uid}:{gid}: {e}",
            path.display()
        );
    }
}

/// The uid and gid that own a file.
fn owner_of(path: &Path) -> anyhow::Result<(u32, u32)> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
    Ok((m.uid(), m.gid()))
}

/// The database's write ahead log and shared memory files, which only exist
/// while it has been written since the last checkpoint.
fn siblings(path: &Path) -> [PathBuf; 2] {
    [
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ]
}

/// Copy the database beside itself before touching its schema. Migrations run
/// one way only, so the copy is the entire undo story for an upgrade that
/// turns out to have been a bad idea.
fn back_up(path: &Path, owner: (u32, u32)) -> anyhow::Result<PathBuf> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "taskologic.db".into());
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let target = path.with_file_name(format!("{name}.bak-{stamp}"));
    std::fs::copy(path, &target)
        .with_context(|| format!("copying {} to {}", path.display(), target.display()))?;
    match_owner(&target, owner);
    // Everything since the last checkpoint lives in the write ahead log, so a
    // copy of the main file on its own can be missing the newest boards.
    for from in siblings(path) {
        if from.exists() {
            let to = PathBuf::from(format!(
                "{}{}",
                target.display(),
                if from.to_string_lossy().ends_with("-wal") {
                    "-wal"
                } else {
                    "-shm"
                }
            ));
            std::fs::copy(&from, &to).with_context(|| format!("copying {}", from.display()))?;
            match_owner(&to, owner);
        }
    }
    Ok(target)
}

fn db_status(cfg: &Config) -> anyhow::Result<()> {
    let path = &cfg.db_path;
    if !path.exists() {
        println!(
            "no database at {} yet, the daemon creates it on first start",
            path.display()
        );
        return Ok(());
    }
    let (at, pending) = db_state(path)?;
    let latest = at.saturating_add(pending.max(0) as usize);
    println!("database:       {}", path.display());
    println!("schema version: {at}");
    match pending {
        0 => println!("pending:        none, already at {latest}"),
        n if n > 0 => println!("pending:        {n} migration(s), up to {latest}"),
        n => println!(
            "pending:        none, and {} migration(s) ahead of this binary",
            -n
        ),
    }
    Ok(())
}

fn migrate(cfg: &Config) -> anyhow::Result<()> {
    let path = &cfg.db_path;
    if !path.exists() {
        println!(
            "no database at {} yet, nothing to migrate; the daemon creates and \
             migrates it on first start",
            path.display()
        );
        return Ok(());
    }
    // The daemon keeps the socket while it runs, so its presence is the one
    // cheap hint that something else has this file open.
    if cfg.socket_path.exists() {
        eprintln!(
            "warning: {} exists, so the daemon may still be running. Stop it first, \
             or the backup can catch a half written write ahead log.",
            cfg.socket_path.display()
        );
    }
    let (at, pending) = db_state(path)?;
    if pending < 0 {
        anyhow::bail!(
            "{} is at schema version {at}, which is {} ahead of this taskologicd. \
             That is a downgrade; install the newer build again or restore a backup.",
            path.display(),
            -pending
        );
    }
    if pending == 0 {
        println!(
            "{} is already at schema version {at}, nothing to do",
            path.display()
        );
        return Ok(());
    }
    let owner = owner_of(path)?;
    let backup = back_up(path, owner)?;
    println!("backed up to {}", backup.display());
    // Opening is what migrates; it is the same path the daemon takes on start,
    // so an upgrade can never apply a different set of migrations than a
    // plain restart would.
    Db::open(path).context("migrating database")?;
    // Whoever ran this is probably root, and SQLite recreates the write ahead
    // log under that account. The daemon is not root and has to write it next.
    for p in siblings(path) {
        if p.exists() {
            match_owner(&p, owner);
        }
    }
    match_owner(path, owner);
    let (now, _) = db_state(path)?;
    println!(
        "migrated {} from schema version {at} to {now}",
        path.display()
    );
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (mode, path) = parse_args();
    if mode == Mode::Version {
        println!("taskologicd {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // The one shot commands print for a human or a shell script to read, so
    // they stay out of the log format the daemon uses.
    if mode == Mode::Serve {
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_env("TASKOLOGICD_LOG")
                    .unwrap_or_else(|_| EnvFilter::new("info")),
            )
            .with_target(false)
            .init();
    }

    let cfg = Config::load(&path).with_context(|| format!("loading config {}", path.display()))?;
    match mode {
        Mode::Version => unreachable!("handled above"),
        Mode::DbStatus => return db_status(&cfg),
        Mode::Migrate => return migrate(&cfg),
        Mode::Transfer(cmd) => {
            let db = Db::open(&cfg.db_path).context("opening database")?;
            // The group is how a server names people who have not logged
            // in yet; a checkout without one still works, it just knows fewer.
            let locals = auth::group_members(&cfg.group).unwrap_or_else(|e| {
                eprintln!("warning: could not read group {}: {e}", cfg.group);
                Vec::new()
            });
            return transfer::run(&db, cmd, invoking_uid(), &locals, &mut std::io::stdout());
        }
        Mode::Serve => {}
    }
    tracing::info!(config = %path.display(), db = %cfg.db_path.display(), socket = %cfg.socket_path.display(), "starting");

    let db = Db::open(&cfg.db_path).context("opening database")?;
    let listener = server::bind(&cfg.socket_path, &cfg.group).context("binding socket")?;
    let socket_path = cfg.socket_path.clone();
    let state = AppState::new(cfg, db);

    tokio::spawn(scheduler::run(state.clone()));
    let server = tokio::spawn(server::run(state, listener));

    shutdown_signal().await;
    tracing::info!("shutting down");
    server.abort();
    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}
