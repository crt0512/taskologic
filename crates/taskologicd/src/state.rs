//! Everything a handler needs, behind one Arc.

use std::sync::Arc;

use chrono::{TimeDelta, Utc};
use taskologic_core::board::Board;
use taskologic_core::ids::Uid;
use taskologic_core::print::PrintJob;
use taskologic_proto::{Event, ServerMessage, Severity};

use crate::bus::{Audience, Bus, Registry};
use crate::config::Config;
use crate::db::{Db, repo};
use crate::error::AppError;

pub struct AppState {
    pub cfg: Config,
    pub db: Db,
    pub bus: Bus,
    pub registry: Registry,
}

impl AppState {
    pub fn new(cfg: Config, db: Db) -> Arc<AppState> {
        Arc::new(AppState {
            cfg,
            db,
            bus: Bus::new(),
            registry: Registry::default(),
        })
    }

    pub fn publish_board(&self, board: &Board, event: Event) {
        self.bus
            .publish(Audience::for_board(board), ServerMessage::Event { event });
    }

    /// For board level facts (created, deleted, settings). Admins hear these
    /// for every board, since they may delete any of them.
    pub fn publish_board_meta(&self, board: &Board, event: Event) {
        self.bus.publish(
            Audience::for_board_or_admins(board),
            ServerMessage::Event { event },
        );
    }

    pub fn publish_uid(&self, uid: Uid, event: Event) {
        self.bus
            .publish(Audience::Uid(uid), ServerMessage::Event { event });
    }

    pub fn notify(&self, uid: Uid, severity: Severity, text: impl Into<String>) {
        self.publish_uid(
            uid,
            Event::Notice {
                text: text.into(),
                severity,
            },
        );
    }

    pub fn print_job_max_age(&self) -> TimeDelta {
        TimeDelta::seconds(self.cfg.print_job_max_age_secs as i64)
    }

    /// Queue a job for a user and try to hand it out right away.
    pub fn enqueue_print(&self, uid: Uid, job: &PrintJob) -> Result<(), AppError> {
        let now = Utc::now();
        let max_age = self.print_job_max_age();
        self.db
            .with(|c| repo::enqueue_print(c, uid, job, max_age, now))?;
        self.dispatch_print_jobs(uid)
    }

    /// Hand every pending job for `uid` to their best connected printer
    /// client, if there is one. Otherwise the jobs wait in the queue.
    pub fn dispatch_print_jobs(&self, uid: Uid) -> Result<(), AppError> {
        let Some(client) = self.registry.printer_for(uid) else {
            return Ok(());
        };
        let now = Utc::now();
        let jobs = self.db.with(|c| repo::pending_print_jobs(c, uid, now))?;
        for (job_id, job) in jobs {
            self.db
                .with(|c| repo::mark_print_in_flight(c, job_id, now))?;
            if client
                .tx
                .send(ServerMessage::Event {
                    event: Event::PrintJob { job_id, job },
                })
                .is_err()
            {
                // Client went away between lookup and send. The scheduler
                // releases stale in-flight jobs, nothing to do here.
                break;
            }
        }
        Ok(())
    }
}
