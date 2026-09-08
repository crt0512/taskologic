//! The device profile lives in the client's local config file, not in the
//! database, because the printer belongs to the machine and the preferences
//! belong to the person.

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use taskologic_core::print::Symbology;

use crate::codepage::Codepage;
use crate::slip::SlipSet;

/// Dots per millimetre on a 203 dpi head, the common receipt density.
const DOTS_PER_MM: usize = 8;
/// The same density as CUPS wants to hear it, for `-o ppi=`.
pub const DPI: usize = 203;
/// Dots one character of the printer's default font takes.
const DOTS_PER_CHAR: usize = 12;
/// Narrower than this and a line holds nothing worth reading; wider than
/// this and no receipt printer will follow. Only a hand edited config can
/// land outside, the form refuses it first.
pub const MIN_CUSTOM_MM: u16 = 20;
pub const MAX_CUSTOM_MM: u16 = 120;

/// How far the 8x8 font is blown up for printing. At 203 dpi a scale of 2 is
/// about 2 mm tall, which is the smallest that still reads on a receipt.
pub const BITMAP_SCALE: usize = 2;
/// Dots one bitmap character takes: the 8x8 font at [`BITMAP_SCALE`].
pub const BITMAP_CHAR_DOTS: usize = 8 * BITMAP_SCALE;
/// A millimetre of slack left at the edge of a drawn slip.
///
/// The paper widths here are round numbers, but a CUPS queue's media is
/// whatever someone measured: a roll we call 72 mm printable is set up as
/// 71.97 mm, and 576 dots at 203 dpi is 72.07 mm, which is *wider than the
/// page*. An image that does not fit makes the CUPS image filter turn it
/// sideways to try, so a slip that is a tenth of a millimetre too wide comes
/// out rotated. One millimetre of slack costs a character a line and takes
/// that whole class of surprise away.
pub const BITMAP_INSET_DOTS: usize = DOTS_PER_MM;

/// Which way up a drawn slip goes on the paper.
///
/// Only [`OutputMode::Bitmap`] can do this: the other two hand their work to
/// something else to place. A half turn is the one receipt printers actually
/// want, for a roll that feeds the other way round. A quarter turn swaps the
/// slip's width and height, so it only fits if the slip is shorter than the
/// roll is wide.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    #[default]
    Upright,
    Cw90,
    Cw180,
    Cw270,
}

impl Rotation {
    pub fn label(self) -> &'static str {
        match self {
            Rotation::Upright => "Upright",
            Rotation::Cw90 => "90 right",
            Rotation::Cw180 => "Upside down",
            Rotation::Cw270 => "90 left",
        }
    }

    /// Whether this turn swaps the image's width and height.
    pub fn swaps_sides(self) -> bool {
        matches!(self, Rotation::Cw90 | Rotation::Cw270)
    }
}

/// How a slip is turned into something the printer will accept.
///
/// This is the setting that decides whether anything comes out at all: a
/// printer that speaks ESC/POS wants the first, a printer whose CUPS queue
/// has a driver wants one of the other two.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    /// ESC/POS command bytes. What most receipt printers with no driver
    /// take, and the only mode that uses the printer's own fonts, barcode
    /// commands and cutter.
    EscPos,
    /// The whole slip drawn as a 1 bit image and handed to CUPS, which runs
    /// it through the queue's driver. Larger jobs, but it works on any
    /// printer CUPS can drive, including the raster only ones with no fonts
    /// on board at all, and it keeps the barcodes. The default, because it
    /// is the mode most likely to put ink on paper on a printer nobody has
    /// told us anything about yet.
    #[default]
    Bitmap,
    /// Plain text through the queue's driver. The simplest thing that can
    /// work on a driver queue, at the cost of the barcodes, which cannot be
    /// drawn with characters.
    Text,
}

impl OutputMode {
    pub fn label(self) -> &'static str {
        match self {
            OutputMode::EscPos => "ESC/POS",
            OutputMode::Bitmap => "Bitmap",
            OutputMode::Text => "Text only",
        }
    }

    /// Dots across the paper this mode draws on. Only the bitmap has an
    /// opinion: it is the only mode that hands CUPS something with a size of
    /// its own, so it is the only one that has to fit inside the page.
    pub fn dots(self, paper: PaperWidth) -> usize {
        match self {
            OutputMode::EscPos | OutputMode::Text => paper.dots(),
            OutputMode::Bitmap => paper.dots().saturating_sub(BITMAP_INSET_DOTS),
        }
    }

    /// Characters per line in this mode. The bitmap font is wider than the
    /// printer's built in one, so the same roll holds fewer of them.
    pub fn columns(self, paper: PaperWidth) -> usize {
        match self {
            OutputMode::EscPos | OutputMode::Text => paper.dots() / DOTS_PER_CHAR,
            OutputMode::Bitmap => self.dots(paper) / BITMAP_CHAR_DOTS,
        }
    }

    /// Whether barcodes can be put on the paper at all in this mode.
    pub fn draws_barcodes(self) -> bool {
        !matches!(self, OutputMode::Text)
    }

    /// What one line of this mode's text costs in characters per inch, which
    /// is what CUPS wants to be told when it is the one setting the type.
    pub fn cpi(self) -> usize {
        (DOTS_PER_MM * 254 / DOTS_PER_CHAR).div_ceil(10)
    }
}

/// Whether a string is safe to hand to `lp` or `lpoptions` as a queue name.
///
/// The queue name is the one piece of user text that reaches a subprocess.
/// It is always passed as its own argument and never through a shell, but
/// the client also runs as a login shell, so the rule lives here and every
/// caller checks it *before* spawning, not only when a profile is saved.
/// An empty name is not valid here; "no printer" is the caller's business.
pub fn queue_name_ok(queue: &str) -> bool {
    !queue.is_empty()
        && queue.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The paper setting to use for a queue whose media is this many millimetres
/// wide, or None if that is no receipt roll.
///
/// Receipt queues are set up both ways round: some name the media after the
/// roll (58 or 80 mm), others after the printable area (48 or 72 mm). Both
/// spellings of the same roll land on the same preset.
pub fn paper_for_media_mm(mm: u16) -> Option<PaperWidth> {
    match mm {
        46..=50 | 56..=60 => Some(PaperWidth::Mm58),
        70..=74 | 78..=82 => Some(PaperWidth::Mm80),
        m if (MIN_CUSTOM_MM..=MAX_CUSTOM_MM).contains(&m) => Some(PaperWidth::Custom(m)),
        _ => None,
    }
}

/// How wide the printer can actually print.
///
/// The two presets are named after the paper roll, which is what is written
/// on the box the rolls come in. A custom width is given as the *printable*
/// width instead, because a roll's margins vary by model: a 58 mm roll
/// prints 48 mm wide and an 80 mm roll prints 72 mm.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum PaperWidth {
    Mm58,
    #[default]
    Mm80,
    /// Printable width in millimetres.
    Custom(u16),
}

impl PaperWidth {
    /// Printable width in millimetres, clamped to what a printer could do.
    pub fn printable_mm(self) -> u16 {
        match self {
            PaperWidth::Mm58 => 48,
            PaperWidth::Mm80 => 72,
            PaperWidth::Custom(mm) => mm.clamp(MIN_CUSTOM_MM, MAX_CUSTOM_MM),
        }
    }

    /// Printable dots per line. Always a multiple of eight, which is what
    /// the raster commands pack into bytes.
    pub fn dots(self) -> usize {
        self.printable_mm() as usize * DOTS_PER_MM
    }

    /// Characters per line in the printer's default font.
    pub fn columns(self) -> usize {
        self.dots() / DOTS_PER_CHAR
    }

    /// How this is written in the config file.
    pub fn as_config_str(self) -> String {
        match self {
            PaperWidth::Mm58 => "mm58".to_string(),
            PaperWidth::Mm80 => "mm80".to_string(),
            PaperWidth::Custom(mm) => format!("custom:{mm}"),
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "mm58" => Some(PaperWidth::Mm58),
            "mm80" => Some(PaperWidth::Mm80),
            other => other
                .strip_prefix("custom:")?
                .trim()
                .parse()
                .ok()
                .map(PaperWidth::Custom),
        }
    }
}

impl Serialize for PaperWidth {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.as_config_str())
    }
}

impl<'de> Deserialize<'de> for PaperWidth {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = PaperWidth;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("\"mm58\", \"mm80\" or \"custom:<printable mm>\"")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<PaperWidth, E> {
                PaperWidth::parse(v)
                    .ok_or_else(|| de::Error::invalid_value(de::Unexpected::Str(v), &self))
            }
        }
        d.deserialize_str(V)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceProfile {
    /// CUPS queue name. This is the only string that ever reaches a
    /// subprocess, and it comes from the config file, never from the UI.
    pub queue: String,
    pub paper: PaperWidth,
    pub codepage: Codepage,
    pub auto_cutter: bool,
    /// How the slip is encoded for this printer.
    pub output: OutputMode,
    /// Which way up the drawn slip goes. Bitmap output only.
    pub rotation: Rotation,
    /// Hand the bytes to CUPS with `-o raw`, so they reach the printer
    /// untouched by the filter chain. Off by default: a queue set up with a
    /// driver expects to filter its jobs, and raw printing is deprecated in
    /// current CUPS. Turn it on for a queue that takes ESC/POS directly,
    /// which is the usual shape of a socket, serial or parallel queue.
    pub raw: bool,
    /// What a slip shows, in what order, and how each part is set: one
    /// layout for task receipts and one for reminders. The one answer to
    /// "why did it print like that", since nothing outside this decides what
    /// reaches the paper.
    pub slips: SlipSet,
    /// Symbologies the printer can draw with its own barcode commands.
    /// Anything else is rasterised. DataMatrix is never native, standard
    /// ESC/POS has no command for it.
    pub native_symbologies: Vec<Symbology>,
}

impl Default for DeviceProfile {
    fn default() -> Self {
        Self {
            queue: String::new(),
            paper: PaperWidth::Mm80,
            codepage: Codepage::Utf8,
            auto_cutter: true,
            output: OutputMode::Bitmap,
            rotation: Rotation::Upright,
            raw: false,
            slips: SlipSet::default(),
            native_symbologies: vec![Symbology::Code39, Symbology::Code128],
        }
    }
}

impl DeviceProfile {
    /// Dots across, which depends on the mode as well as the paper.
    pub fn dots(&self) -> usize {
        self.output.dots(self.paper)
    }

    /// Characters per line, which depends on the mode as well as the paper.
    pub fn columns(&self) -> usize {
        self.output.columns(self.paper)
    }

    /// Whether `lp` should be told to skip the filters for this profile.
    /// Only ever true for ESC/POS: a bitmap or text job exists precisely to
    /// be filtered, and sending one raw would print nothing.
    pub fn use_raw(&self) -> bool {
        self.raw && self.output == OutputMode::EscPos
    }

    pub fn supports_natively(&self, s: Symbology) -> bool {
        s != Symbology::DataMatrix && self.native_symbologies.contains(&s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_keep_their_established_geometry() {
        assert_eq!(PaperWidth::Mm58.dots(), 384);
        assert_eq!(PaperWidth::Mm58.columns(), 32);
        assert_eq!(PaperWidth::Mm80.dots(), 576);
        assert_eq!(PaperWidth::Mm80.columns(), 48);
    }

    #[test]
    fn a_custom_width_measures_the_printable_area() {
        // The presets are the same thing said another way.
        assert_eq!(PaperWidth::Custom(48).columns(), PaperWidth::Mm58.columns());
        assert_eq!(PaperWidth::Custom(72).dots(), PaperWidth::Mm80.dots());
        assert_eq!(PaperWidth::Custom(60).dots(), 480);
        assert_eq!(PaperWidth::Custom(60).columns(), 40);
    }

    #[test]
    fn a_nonsense_custom_width_still_leaves_a_usable_line() {
        // Only a hand edited config gets here, and it must not divide by zero.
        assert!(PaperWidth::Custom(0).columns() > 0);
        assert_eq!(PaperWidth::Custom(9999).printable_mm(), MAX_CUSTOM_MM);
        assert_eq!(
            PaperWidth::Custom(0).dots() % 8,
            0,
            "raster rows pack into whole bytes"
        );
    }

    #[test]
    fn queue_names_are_checked_before_they_reach_a_subprocess() {
        assert!(queue_name_ok("receipt"));
        assert!(queue_name_ok("TSP143-STR_T-001"));
        assert!(queue_name_ok("front.desk_2"));
        assert!(!queue_name_ok(""), "empty is not a name");
        assert!(!queue_name_ok("bad name"));
        assert!(!queue_name_ok("rm -rf /"));
        assert!(!queue_name_ok("a;b"));
        assert!(!queue_name_ok("$(id)"));
        assert!(!queue_name_ok("../etc/passwd"));
    }

    #[test]
    fn media_widths_map_onto_the_roll_they_came_from() {
        // Named after the printable area.
        assert_eq!(paper_for_media_mm(48), Some(PaperWidth::Mm58));
        assert_eq!(paper_for_media_mm(72), Some(PaperWidth::Mm80));
        // Named after the roll.
        assert_eq!(paper_for_media_mm(58), Some(PaperWidth::Mm58));
        assert_eq!(paper_for_media_mm(80), Some(PaperWidth::Mm80));
        // A Star TSP143 reports 71.97 mm, which rounds onto the 80 mm preset.
        assert_eq!(paper_for_media_mm(72), Some(PaperWidth::Mm80));
        // Anything else usable becomes a custom width.
        assert_eq!(paper_for_media_mm(64), Some(PaperWidth::Custom(64)));
        // A4 or Letter is not a receipt roll.
        assert_eq!(paper_for_media_mm(210), None);
        assert_eq!(paper_for_media_mm(216), None);
        assert_eq!(paper_for_media_mm(0), None);
    }

    #[test]
    fn config_strings_round_trip() {
        for p in [PaperWidth::Mm58, PaperWidth::Mm80, PaperWidth::Custom(64)] {
            let toml = toml::to_string(&DeviceProfile {
                paper: p,
                ..Default::default()
            })
            .unwrap();
            let back: DeviceProfile = toml::from_str(&toml).unwrap();
            assert_eq!(back.paper, p, "{toml}");
        }
    }

    #[test]
    fn the_old_spelling_of_the_presets_still_loads() {
        let p: DeviceProfile = toml::from_str("queue = \"receipt\"\npaper = \"mm58\"\n").unwrap();
        assert_eq!(p.paper, PaperWidth::Mm58);
    }

    #[test]
    fn a_profile_with_nothing_set_is_the_common_printer() {
        let p: DeviceProfile = toml::from_str("queue = \"receipt\"\n").unwrap();
        assert_eq!(p.paper, PaperWidth::Mm80);
        assert_eq!(p.codepage, Codepage::Utf8);
        assert_eq!(p.output, OutputMode::Bitmap, "the mode that works on the most printers");
        assert_eq!(p.rotation, Rotation::Upright);
        assert!(!p.raw, "raw printing is opt in");
        assert!(p.auto_cutter);
    }

    #[test]
    fn a_drawn_slip_always_fits_inside_the_page_it_is_named_after() {
        // A queue set up for an 80 mm roll reports 71.97 mm of media, and
        // 203 dpi is really 203.2, so the round numbers do not quite line up.
        // Whatever the paper, the drawn width has to come out under it.
        for paper in [PaperWidth::Mm58, PaperWidth::Mm80, PaperWidth::Custom(64)] {
            let page_mm = f64::from(paper.printable_mm());
            let drawn_mm = OutputMode::Bitmap.dots(paper) as f64 / 203.0 * 25.4;
            assert!(drawn_mm < page_mm, "{paper:?}: drew {drawn_mm:.2} mm on {page_mm} mm");
            // ESC/POS is placed by the printer, so it keeps the full width.
            assert_eq!(OutputMode::EscPos.dots(paper), paper.dots());
        }
    }

    #[test]
    fn only_a_quarter_turn_swaps_the_sides() {
        assert!(!Rotation::Upright.swaps_sides());
        assert!(!Rotation::Cw180.swaps_sides());
        assert!(Rotation::Cw90.swaps_sides());
        assert!(Rotation::Cw270.swaps_sides());
    }
}
