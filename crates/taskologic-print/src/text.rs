//! Draws a slip as plain text, for a driver queue where the simplest thing
//! that can work is the right thing.
//!
//! CUPS renders this with its own font, so there is no bold, no double size
//! and, most of it, no barcodes: bars cannot be made of characters at a
//! density any scanner would read. The payload is printed as text instead,
//! so the slip is still readable by a person, and
//! [`OutputMode::draws_barcodes`](crate::OutputMode::draws_barcodes) says
//! plainly that this mode cannot do it.

use crate::layout::{Align, Op};
use crate::profile::DeviceProfile;

/// Render a planned slip as text laid out for a paper this many columns wide.
pub fn render(ops: &[Op], profile: &DeviceProfile) -> String {
    let cols = profile.columns().max(1);
    let mut out = String::new();
    for op in ops {
        match op {
            Op::Line(l) => {
                let width = l.text.chars().count();
                let pad = match l.align {
                    Align::Left => 0,
                    Align::Center => cols.saturating_sub(width) / 2,
                    Align::Right => cols.saturating_sub(width),
                };
                for _ in 0..pad {
                    out.push(' ');
                }
                out.push_str(&l.text);
                out.push('\n');
            }
            Op::Blank => out.push('\n'),
            Op::Barcode { code, label } => {
                out.push_str(&format!("{label}: {}\n", code.payload));
            }
            // Paper clear of the head, then the driver's own cut.
            Op::End => out.push_str("\n\n\n"),
        }
    }
    out
}
