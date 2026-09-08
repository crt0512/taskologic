//! Where the bytes go.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::profile::{DPI, DeviceProfile, OutputMode};

#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    #[error("no printer queue configured")]
    NoQueue,
    #[error("could not run lp: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("lp failed: {0}")]
    Lp(String),
}

pub trait PrinterSink {
    fn print(&mut self, bytes: &[u8]) -> Result<(), SinkError>;
}

/// Captures everything for tests.
#[derive(Debug, Default)]
pub struct FakeSink {
    pub jobs: Vec<Vec<u8>>,
    pub fail_next: bool,
}

impl PrinterSink for FakeSink {
    fn print(&mut self, bytes: &[u8]) -> Result<(), SinkError> {
        if self.fail_next {
            self.fail_next = false;
            return Err(SinkError::Lp("simulated failure".into()));
        }
        self.jobs.push(bytes.to_vec());
        Ok(())
    }
}

/// Spools through CUPS with `lp`.
///
/// This is the only subprocess the login shell is allowed to start. The
/// queue name comes from the local config file, the payload goes over stdin,
/// and nothing is ever assembled into a shell string.
///
/// The options depend on how the slip was rendered. ESC/POS bytes may want
/// `-o raw` so the filters leave them alone; a bitmap or a text slip wants
/// the opposite, plus enough of a hint that CUPS lays it out at the size it
/// was drawn for rather than fitting it to a sheet of A4.
///
/// Either way `lp` only reports whether the job was *queued*: a job that
/// dies in the filter chain afterwards still leaves `lp` reporting success.
#[derive(Debug, Clone)]
pub struct LpSink {
    queue: String,
    options: Vec<String>,
}

impl LpSink {
    pub fn new(queue: impl Into<String>, options: Vec<String>) -> Self {
        Self { queue: queue.into(), options }
    }

    /// The sink this profile asks for.
    pub fn for_profile(p: &DeviceProfile) -> Self {
        Self::new(p.queue.clone(), lp_options(p))
    }

    /// The argv, as a fixed list. Only the queue name varies, and it is
    /// always its own argument.
    fn args(&self) -> Vec<&str> {
        let mut args = vec!["-d", self.queue.as_str(), "-s"];
        for o in &self.options {
            args.push("-o");
            args.push(o);
        }
        args
    }
}

/// The `-o` options a profile needs.
pub fn lp_options(p: &DeviceProfile) -> Vec<String> {
    match p.output {
        // The printer's own language. Filters can only get in the way, but
        // only skip them if this profile actually asked to.
        OutputMode::EscPos => {
            if p.use_raw() {
                vec!["raw".to_string()]
            } else {
                Vec::new()
            }
        }
        // Drawn at the printer's own density, so say so, and let CUPS place
        // it 1:1 in the corner instead of scaling it to fill a page.
        OutputMode::Bitmap => {
            let mut o = vec![
                format!("ppi={DPI}"),
                "position=top-left".to_string(),
                // Portrait, and say so. Left to itself the CUPS image filter
                // turns a picture sideways whenever that would fit the page
                // better, which on a receipt roll means any slip a hair too
                // wide comes out along the paper instead of down it. The
                // rotation setting is ours to make, not the filter's.
                "orientation-requested=3".to_string(),
            ];
            o.extend(no_margins());
            o
        }
        // CUPS sets the type here, so it has to be told how dense to set it.
        OutputMode::Text => {
            let mut o = vec![format!("cpi={}", p.output.cpi())];
            o.extend(no_margins());
            o
        }
    }
}

/// A receipt has no margins, the roll is the margin. Left in, CUPS' default
/// inch on every side is wider than the paper itself.
fn no_margins() -> impl Iterator<Item = String> {
    ["page-left=0", "page-right=0", "page-top=0", "page-bottom=0"].into_iter().map(String::from)
}

impl PrinterSink for LpSink {
    fn print(&mut self, bytes: &[u8]) -> Result<(), SinkError> {
        if self.queue.trim().is_empty() {
            return Err(SinkError::NoQueue);
        }
        let mut child = Command::new("lp")
            .args(self.args())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(SinkError::Spawn)?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(bytes).map_err(SinkError::Spawn)?;
        }
        let out = child.wait_with_output().map_err(SinkError::Spawn)?;
        if out.status.success() {
            Ok(())
        } else {
            Err(SinkError::Lp(String::from_utf8_lossy(&out.stderr).trim().to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(output: OutputMode, raw: bool) -> DeviceProfile {
        DeviceProfile { queue: "receipt".into(), output, raw, ..Default::default() }
    }

    #[test]
    fn escpos_passes_raw_only_when_it_was_asked_to() {
        assert_eq!(lp_options(&profile(OutputMode::EscPos, false)), Vec::<String>::new());
        assert_eq!(lp_options(&profile(OutputMode::EscPos, true)), ["raw"]);
        let sink = LpSink::for_profile(&profile(OutputMode::EscPos, true));
        assert_eq!(sink.args(), ["-d", "receipt", "-s", "-o", "raw"]);
    }

    #[test]
    fn the_filtered_modes_never_go_raw_however_the_flag_is_set() {
        for output in [OutputMode::Bitmap, OutputMode::Text] {
            let o = lp_options(&profile(output, true));
            assert!(!o.iter().any(|x| x == "raw"), "{output:?} must be filtered: {o:?}");
            // A receipt roll has no room for CUPS' default inch of margin.
            assert!(o.iter().any(|x| x == "page-left=0"), "{output:?}: {o:?}");
        }
    }

    #[test]
    fn a_bitmap_is_placed_at_the_density_it_was_drawn_for() {
        let o = lp_options(&profile(OutputMode::Bitmap, false));
        assert!(o.contains(&"ppi=203".to_string()), "{o:?}");
        assert!(o.contains(&"position=top-left".to_string()), "{o:?}");
    }

    #[test]
    fn a_bitmap_is_pinned_to_portrait_so_the_filter_cannot_turn_it() {
        // 3 is IPP's portrait. Without it the image filter rotates a slip
        // that does not quite fit, and it comes out along the roll.
        let o = lp_options(&profile(OutputMode::Bitmap, false));
        assert!(o.contains(&"orientation-requested=3".to_string()), "{o:?}");
    }

    #[test]
    fn text_tells_cups_how_dense_to_set_it() {
        // 12 dots a character at 203 dpi is about 17 characters an inch.
        assert!(lp_options(&profile(OutputMode::Text, false)).contains(&"cpi=17".to_string()));
    }

    #[test]
    fn an_empty_queue_never_reaches_lp() {
        assert!(matches!(LpSink::new("  ", Vec::new()).print(b"x"), Err(SinkError::NoQueue)));
    }
}
