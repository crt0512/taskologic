//! Prints the barcode payload for a task, so a scan can be tested by typing
//! it. Usage: cargo run -p taskologic-core --example payload -- K4M9Q2 S

use taskologic_core::barcode::{Magic, ScanAction, ScanPayload};
use taskologic_core::ids::ShortId;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = |a: &String| a.chars().next().and_then(ScanAction::from_code);
    let (id, action) = match args.as_slice() {
        [id] => (id.as_str(), ScanAction::StartPause),
        [id, a] if a.chars().count() == 1 && code(a).is_some() => {
            (id.as_str(), code(a).expect("checked"))
        }
        _ => {
            eprintln!(
                "usage: payload <SHORTID> [S|F|Y|N|1-8]   (S = start/pause, F = finish, \
                 Y/N = answer yes/no and finish, 1-8 = pick that answer and finish)"
            );
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
