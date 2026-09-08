//! Prints the barcode payload for a task, so a scan can be tested by typing
//! it. Usage: cargo run -p taskologic-core --example payload -- K4M9Q2 S

use taskologic_core::barcode::{Magic, ScanAction, ScanPayload};
use taskologic_core::ids::ShortId;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (id, action) = match args.as_slice() {
        [id] => (id.as_str(), ScanAction::StartPause),
        [id, a] if a.eq_ignore_ascii_case("s") => (id.as_str(), ScanAction::StartPause),
        [id, a] if a.eq_ignore_ascii_case("f") => (id.as_str(), ScanAction::Finish),
        _ => {
            eprintln!("usage: payload <SHORTID> [S|F]   (S = start/pause, F = finish)");
            std::process::exit(2);
        }
    };
    match ShortId::parse(id) {
        Ok(short_id) => println!("{}", ScanPayload { action, short_id }.encode(Magic::Dots)),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
