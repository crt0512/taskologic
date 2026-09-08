//! Domain logic for Taskologic.
//!
//! This crate is deliberately pure: no IO, no rendering, no async. It depends
//! on nothing that talks to a terminal, a socket or a database. That is what
//! lets the daemon, the TUI client and any future GUI or browser client share
//! one copy of every rule about who may do what, how a barcode is laid out and
//! when a repeating task fires.
//!
//! If a rule about permissions or transitions is not in here, it does not
//! exist as far as the clients are concerned.

pub mod barcode;
pub mod board;
pub mod deps;
pub mod event;
pub mod ids;
pub mod permission;
pub mod prefs;
pub mod print;
pub mod repeat;
pub mod task;
pub mod template;
pub mod transition;
pub mod user;

pub use ids::{BoardId, ColumnId, ShortId, TaskId, Uid};
