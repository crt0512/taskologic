//! Runs a print job against this client's printer, off the UI thread.

use taskologic_core::ids::PrintJobId;
use taskologic_core::print::PrintJob;
use taskologic_print::{DeviceProfile, LpSink, PrinterSink, queue_name_ok};
use tokio::sync::mpsc;

use crate::app::Msg;

pub fn spawn(
    profile: DeviceProfile,
    job_id: PrintJobId,
    job: PrintJob,
    to_app: mpsc::UnboundedSender<Msg>,
) {
    tokio::task::spawn_blocking(move || {
        let result = taskologic_print::render(&job, &profile)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                LpSink::for_profile(&profile)
                    .print(&bytes)
                    .map_err(|e| e.to_string())
            });
        let _ = to_app.send(Msg::Printed {
            job_id,
            error: result.err(),
        });
    });
}

/// Same as `spawn`, but the result goes nowhere near the daemon.
pub fn spawn_test(profile: DeviceProfile, job: PrintJob, to_app: mpsc::UnboundedSender<Msg>) {
    tokio::task::spawn_blocking(move || {
        let result = taskologic_print::render(&job, &profile)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                LpSink::for_profile(&profile)
                    .print(&bytes)
                    .map_err(|e| e.to_string())
            });
        let _ = to_app.send(Msg::TestPrinted {
            error: result.err(),
        });
    });
}

/// Asks CUPS which queues exist, for the printer setup panel. `lpstat -e`
/// prints one destination name per line. The argv is fixed, no user text
/// gets anywhere near it, which keeps the login shell rule intact.
pub fn detect_queues(to_app: mpsc::UnboundedSender<Msg>) {
    tokio::task::spawn_blocking(move || {
        let msg = match std::process::Command::new("lpstat").arg("-e").output() {
            Ok(out) if out.status.success() => {
                let queues = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect();
                Msg::PrintersDetected {
                    queues,
                    error: None,
                }
            }
            Ok(out) => {
                let e = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let error = if e.is_empty() {
                    "lpstat found no destinations".to_string()
                } else {
                    e
                };
                Msg::PrintersDetected {
                    queues: Vec::new(),
                    error: Some(error),
                }
            }
            Err(e) => Msg::PrintersDetected {
                queues: Vec::new(),
                error: Some(format!("could not run lpstat: {e}")),
            },
        };
        let _ = to_app.send(msg);
    });
}

/// The width in millimetres of a CUPS media name, if it carries one.
///
/// Covers the spellings a receipt queue actually turns up with:
///
/// ```text
/// custom_71.97x199.67mm_71.97x199.67mm   what CUPS writes today (PWG)
/// Custom.72x200mm                        older CUPS
/// 72x200mm                               bare
/// na_letter_8.5x11in                     inches, converted
/// X72MMY200MM                            some Star and Epson PPDs
/// ```
///
/// A named size with no dimensions in it, "A4" or "Letter", gives None.
pub fn media_width_mm(media: &str) -> Option<u16> {
    let m = media.trim();
    if m.is_empty() {
        return None;
    }
    // X72MMY200MM: width sits between the leading X and the first MM.
    if let Some(rest) = m.strip_prefix(['X', 'x'])
        && let Some(end) = rest.to_ascii_uppercase().find("MM")
        && let Ok(w) = rest[..end].parse::<f64>()
    {
        return round_mm(w);
    }
    // PWG names put the dimensions last: class_name_WxH<unit>.
    let field = m.rsplit('_').next().unwrap_or(m);
    let field = field.strip_prefix("Custom.").or_else(|| field.strip_prefix("custom.")).unwrap_or(field);
    let (w, rest) = field.split_once(['x', 'X'])?;
    let w: f64 = w.parse().ok()?;
    round_mm(if rest.to_ascii_lowercase().ends_with("in") { w * 25.4 } else { w })
}

fn round_mm(mm: f64) -> Option<u16> {
    (mm.is_finite() && mm > 0.0 && mm < 10_000.0).then(|| mm.round() as u16)
}

/// Asks CUPS what media a queue is set up for, so the printer setup panel can
/// offer the right paper width instead of making someone measure the roll.
/// `lpoptions -p QUEUE` prints one line of `key=value` pairs.
pub fn detect_media(queue: String, to_app: mpsc::UnboundedSender<Msg>) {
    tokio::task::spawn_blocking(move || {
        // Checked here rather than trusted: this string becomes an argv entry.
        if !queue_name_ok(&queue) {
            return;
        }
        let mm = std::process::Command::new("lpoptions")
            .arg("-p")
            .arg(&queue)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .split_whitespace()
                    .find_map(|f| f.strip_prefix("media=").and_then(media_width_mm))
            });
        let _ = to_app.send(Msg::MediaDetected { queue, mm });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_width_out_of_the_media_names_cups_uses() {
        // Exactly what a Star TSP143 receipt queue reports.
        assert_eq!(media_width_mm("custom_71.97x199.67mm_71.97x199.67mm"), Some(72));
        assert_eq!(media_width_mm("Custom.72x200mm"), Some(72));
        assert_eq!(media_width_mm("72x200mm"), Some(72));
        assert_eq!(media_width_mm("X72MMY200MM"), Some(72));
        assert_eq!(media_width_mm("om_58mm_58x210mm"), Some(58));
        // Inches get converted.
        assert_eq!(media_width_mm("na_letter_8.5x11in"), Some(216));
    }

    #[test]
    fn a_name_without_dimensions_has_no_width() {
        assert_eq!(media_width_mm("A4"), None);
        assert_eq!(media_width_mm("Letter"), None);
        assert_eq!(media_width_mm(""), None);
        assert_eq!(media_width_mm("   "), None);
        assert_eq!(media_width_mm("custom_nonsense"), None);
        assert_eq!(media_width_mm("xerox_thing"), None);
    }
}
