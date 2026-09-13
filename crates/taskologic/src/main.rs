//! taskologic: the TUI client and login shell.

mod app;
mod config;
mod forms;
mod net;
mod printing;
mod scan;
mod term;
mod ui;

use std::time::{Duration, Instant};

use anyhow::Context;
use taskologic_proto::Hello;
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use crate::app::{App, Cmd, Msg};

/// Diagnostics go to a file, never the terminal, this is a full screen app.
/// Off unless TASKOLOGIC_LOG names a path; RUST_LOG filters, default debug.
fn init_logging() {
    let Some(path) = std::env::var_os("TASKOLOGIC_LOG") else {
        return;
    };
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("debug"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .init();
}

/// This binary is somebody's login shell, so it gets handed all sorts of
/// arguments it has never cared about and must keep ignoring. The one
/// exception is a bare `--version`, which `scripts/update.sh` asks for to
/// find out what is installed before it overwrites it.
fn version_asked() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    matches!(args.as_slice(), [one] if one == "--version" || one == "-V")
}

#[tokio::main]
async fn main() {
    if version_asked() {
        println!("taskologic {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    term::install_panic_hook();
    let code = match run().await {
        Ok(()) => 0,
        Err(e) => {
            term::restore();
            eprintln!("taskologic: {e:#}");
            1
        }
    };
    term::restore();
    std::process::exit(code);
}

async fn run() -> anyhow::Result<()> {
    let (mut cfg, cfg_path) = config::load().context("loading client config")?;
    init_logging();
    let (to_app, mut inbox) = mpsc::unbounded_channel::<Msg>();
    let start = Instant::now();
    let now_ms = move || start.elapsed().as_millis() as u64;

    let hello = Hello {
        protocol_version: taskologic_proto::PROTOCOL_VERSION,
        client_version: env!("CARGO_PKG_VERSION").into(),
        has_printer: cfg.has_printer(),
        print_priority: cfg.print_priority,
        capabilities: vec!["mouse".into()],
    };
    let mut app = App::new(cfg.ascii(), cfg.color_mode(), cfg.printer.clone());

    let net = match UnixStream::connect(&cfg.socket_path).await {
        Ok(s) => Some(net::spawn(s, to_app.clone())),
        Err(e) => {
            app.update(Msg::Disconnected(format!(
                "cannot reach the Taskologic daemon at {}: {e}",
                cfg.socket_path.display()
            )));
            None
        }
    };

    let mut terminal = term::setup()?;
    {
        // crossterm's blocking reader on its own thread, feeding the same inbox.
        let tx = to_app.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = crossterm::event::read() {
                if tx.send(Msg::Term(ev)).is_err() {
                    break;
                }
            }
        });
    }
    let size = terminal.size()?;
    app.update(Msg::Term(crossterm::event::Event::Resize(
        size.width,
        size.height,
    )));

    let mut cmds = if net.is_some() {
        app.start(hello)
    } else {
        Vec::new()
    };
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    loop {
        for cmd in cmds.drain(..) {
            match cmd {
                Cmd::Send(msg) => {
                    if let Some(n) = &net {
                        let _ = n.tx.send(msg);
                    }
                }
                Cmd::Print { job_id, job } => match cfg.printer.clone() {
                    Some(profile) => printing::spawn(profile, job_id, job, to_app.clone()),
                    None => {
                        let _ = to_app.send(Msg::Printed {
                            job_id,
                            error: Some("this client has no printer".into()),
                        });
                    }
                },
                Cmd::TestPrint { job, profile } => {
                    printing::spawn_test(profile, job, to_app.clone())
                }
                Cmd::DetectPrinters => printing::detect_queues(to_app.clone()),
                Cmd::DetectMedia(queue) => printing::detect_media(queue, to_app.clone()),
                Cmd::SavePrinter(profile) => {
                    cfg.printer = profile;
                    let msg = match config::save_printer(cfg_path.as_deref(), cfg.printer.as_ref())
                    {
                        Ok(path) => Msg::PrinterSaved {
                            path: path.display().to_string(),
                            error: None,
                        },
                        Err(e) => Msg::PrinterSaved {
                            path: "client.toml".into(),
                            error: Some(format!("{e:#}")),
                        },
                    };
                    let _ = to_app.send(msg);
                }
                Cmd::Bell => term::bell(),
                Cmd::Quit => return Ok(()),
            }
        }
        terminal.draw(|f| ui::view(&mut app, f))?;

        let msg = tokio::select! {
            Some(m) = inbox.recv() => m,
            _ = tick.tick() => Msg::Tick(now_ms()),
        };
        cmds = app.update(msg);
        // Coalesce whatever else is already waiting before the next draw.
        while let Ok(m) = inbox.try_recv() {
            cmds.extend(app.update(m));
        }
    }
}
