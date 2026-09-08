//! taskologicd: owns the database, the scheduler and the event bus. The only
//! process that ever opens the SQLite file.

mod auth;
mod bus;
mod config;
mod db;
mod error;
mod handlers;
mod scheduler;
mod server;
mod state;

use std::path::PathBuf;

use anyhow::Context;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::db::Db;
use crate::state::AppState;

fn usage() -> ! {
    eprintln!(
        "usage: taskologicd [--config PATH]\n\nenvironment: TASKOLOGICD_CONFIG, TASKOLOGICD_LOG (tracing filter, default info)"
    );
    std::process::exit(2)
}

fn config_path() -> PathBuf {
    let mut args = std::env::args().skip(1);
    let mut path: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" | "-c" => {
                path = Some(args.next().map(PathBuf::from).unwrap_or_else(|| usage()))
            }
            "--help" | "-h" => usage(),
            _ => usage(),
        }
    }
    path.or_else(|| std::env::var_os("TASKOLOGICD_CONFIG").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(config::DEFAULT_PATH))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("TASKOLOGICD_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let path = config_path();
    let cfg = Config::load(&path).with_context(|| format!("loading config {}", path.display()))?;
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
