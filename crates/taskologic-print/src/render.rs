//! Turns a `PrintJob` into bytes for a specific printer.
//!
//! The layout is decided once in [`crate::layout`]; this picks the backend
//! that can actually put it on the paper in front of us.

use taskologic_core::print::{Barcode, PrintJob, Symbology};

use crate::escpos::{EscPos, NativeBarcode};
use crate::layout::{Align, Op, TextLine};
use crate::profile::{DeviceProfile, OutputMode};
use crate::symbology::{self, SymbologyError};
use crate::{bitmap, layout, text};

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error(transparent)]
    Symbology(#[from] SymbologyError),
}

const BAR_HEIGHT_DOTS: usize = 64;
const BAR_MODULE_DOTS: usize = 2;
const MATRIX_MODULE_DOTS: usize = 4;

/// Render a job for this device. Pure, no IO.
pub fn render(job: &PrintJob, profile: &DeviceProfile) -> Result<Vec<u8>, RenderError> {
    // A reminder and a receipt are shaped by their own layouts.
    let ops = layout::plan(job, profile.columns(), profile.slips.for_kind(job.kind));
    match profile.output {
        OutputMode::EscPos => escpos_bytes(&ops, profile),
        OutputMode::Bitmap => bitmap::render(&ops, profile),
        OutputMode::Text => Ok(text::render(&ops, profile).into_bytes()),
    }
}

/// The ESC/POS backend: the printer's own fonts, barcodes and cutter.
fn escpos_bytes(ops: &[Op], profile: &DeviceProfile) -> Result<Vec<u8>, RenderError> {
    let mut p = EscPos::new(profile.codepage);
    let mut align = Align::Left;
    for op in ops {
        match op {
            Op::Line(l) => {
                set_align(&mut p, &mut align, l.align);
                emit_line(&mut p, l);
            }
            Op::Blank => {
                p.newline();
            }
            Op::Barcode { code, label } => {
                set_align(&mut p, &mut align, Align::Center);
                p.line(label);
                barcode(&mut p, code, profile)?;
            }
            Op::End => {
                set_align(&mut p, &mut align, Align::Left);
                if profile.auto_cutter {
                    p.cut();
                } else {
                    p.feed(5);
                }
            }
        }
    }
    Ok(p.into_bytes())
}

fn set_align(p: &mut EscPos, current: &mut Align, want: Align) {
    if *current != want {
        p.align(want);
        *current = want;
    }
}

fn emit_line(p: &mut EscPos, l: &TextLine) {
    if l.bold {
        p.bold(true);
    }
    if l.scale > 1 {
        p.size(l.scale, l.scale);
    }
    p.line(&l.text);
    if l.scale > 1 {
        p.size(1, 1);
    }
    if l.bold {
        p.bold(false);
    }
}

fn barcode(p: &mut EscPos, bc: &Barcode, profile: &DeviceProfile) -> Result<(), RenderError> {
    // A printer's own command draws two dots per module at the least, so a
    // code asked for thin is rasterised whatever the printer can do.
    let native = match bc.symbology {
        _ if bc.narrow => None,
        Symbology::Code39 if profile.supports_natively(Symbology::Code39) => {
            Some(NativeBarcode::Code39)
        }
        Symbology::Code128 if profile.supports_natively(Symbology::Code128) => {
            Some(NativeBarcode::Code128)
        }
        _ => None,
    };
    // Encoded either way: the module count is what sizes a wide barcode.
    let m = symbology::encode(bc.symbology, &bc.payload)?;
    match native {
        Some(kind) => {
            // Printers take 2 to 6 dots per module for their own codes, and
            // draw their own quiet zones, about ten modules each side.
            let width = profile.bar_module_dots(m.width, 10, BAR_MODULE_DOTS, 6);
            p.native_barcode(kind, &bc.payload, BAR_HEIGHT_DOTS as u8, width as u8);
        }
        None => {
            let scale = if bc.narrow && m.is_1d() {
                1
            } else if m.is_1d() {
                profile.bar_module_dots(m.width, 2, BAR_MODULE_DOTS, usize::MAX)
            } else {
                MATRIX_MODULE_DOTS
            };
            let img = m.scaled(scale, BAR_HEIGHT_DOTS, 2).centered_in(profile.paper.dots());
            let (bpr, rows) = img.packed_rows(profile.paper.dots());
            p.raster(bpr, &rows);
            p.line(&bc.payload);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codepage::Codepage;
    use crate::profile::{OutputMode, PaperWidth};
    use chrono::DateTime;
    use taskologic_core::board::test_support::board_with_members;
    use taskologic_core::prefs::UserPrefs;
    use taskologic_core::print::build_task_job;
    use taskologic_core::task::test_support::task_on;
    use taskologic_core::user::User;

    fn job(scanner: bool, symbology: Symbology) -> PrintJob {
        let board = board_with_members(1, &[1, 2]);
        let mut task = task_on(&board, 1);
        task.title = "Wasser für die Pflanzen".into();
        task.description = "Balkon und Küche.\n\nNicht die Kakteen.".into();
        task.assignees = vec![2];
        task.due_at = Some(DateTime::from_timestamp(1_800_000_000, 0).unwrap());
        let mut prefs = UserPrefs::default();
        prefs.ui.scanner_enabled = scanner;
        prefs.scanner.format = symbology;
        let user = User {
            uid: 1,
            username: "alice".into(),
            is_admin: false,
            timezone: chrono_tz::Europe::Berlin,
            prefs,
            has_pin: false,
            created_at: DateTime::from_timestamp(0, 0).unwrap(),
        };
        build_task_job(
            &task,
            &board,
            &[],
            &|u| format!("user{u}"),
            &user,
            DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
        )
    }

    /// A profile for the ESC/POS backend. The default mode is bitmap, which
    /// is the right default for a printer nobody has told us about, but these
    /// tests are about the command bytes.
    fn escpos() -> DeviceProfile {
        DeviceProfile { output: OutputMode::EscPos, ..Default::default() }
    }

    /// Every section on, so a test about a backend is not also a test about
    /// which sections the default layout happens to enable.
    fn show_everything(mut p: DeviceProfile) -> DeviceProfile {
        for l in [&mut p.slips.task, &mut p.slips.reminder] {
            for row in &mut l.rows {
                row.enabled = true;
            }
        }
        p
    }

    fn count(hay: &[u8], needle: &[u8]) -> usize {
        hay.windows(needle.len()).filter(|w| *w == needle).count()
    }

    #[test]
    fn plain_slip_has_reset_text_and_cut() {
        let bytes = render(&job(false, Symbology::Code39), &escpos()).unwrap();
        assert_eq!(&bytes[..2], &[0x1B, b'@']);
        assert_eq!(
            count(&bytes, &[0x1B, b't']),
            0,
            "the default set is UTF-8, which selects no table"
        );
        assert!(
            count(&bytes, "Wasser für die Pflanzen".as_bytes()) == 1,
            "title missing or wrongly encoded"
        );
        assert!(count(&bytes, "Küche".as_bytes()) == 1);
        assert!(
            count(&bytes, b"Due: 2027-01-15 09:00") == 1,
            "due date in Berlin time"
        );
        // Assignees are off in the default layout, and the start barcode is
        // for reminders, so a task slip carries only the finish one.
        assert_eq!(count(&bytes, b"For: user2"), 0, "assignees are off by default");
        assert_eq!(count(&bytes, &[0x1D, b'k']), 1, "the finish barcode only");
        assert!(bytes.ends_with(&[0x1D, b'V', 1]), "partial cut at the end");
    }

    #[test]
    fn a_single_byte_codepage_encodes_the_text_for_the_printer() {
        let profile = DeviceProfile { codepage: Codepage::Cp437, ..escpos() };
        let bytes = render(&job(false, Symbology::Code39), &profile).unwrap();
        assert_eq!(
            &bytes[..5],
            &[0x1B, b'@', 0x1B, b't', 0],
            "CP437 is selected up front"
        );
        // Codepage 437 umlaut u.
        assert!(
            count(&bytes, b"Wasser f\x81r die Pflanzen") == 1,
            "title missing or wrongly encoded"
        );
        assert!(count(&bytes, b"K\x81che") == 1);
    }

    #[test]
    fn wide_barcodes_fill_the_paper_native_and_rasterised() {
        let plain = show_everything(escpos());
        let wide = DeviceProfile { wide_barcodes: true, ..plain.clone() };
        // The printer's own command takes the module width right after GS w.
        let module_width = |bytes: &[u8]| {
            bytes
                .windows(3)
                .find(|w| w[0] == 0x1D && w[1] == b'w')
                .map(|w| w[2])
                .expect("a GS w")
        };
        let narrow = render(&job(true, Symbology::Code39), &plain).unwrap();
        assert_eq!(module_width(&narrow), 2);
        let n = module_width(&render(&job(true, Symbology::Code39), &wide).unwrap());
        assert!((3..=6).contains(&n), "wider, within what the command takes: {n}");

        // Rasterised, the first image (GS v 0 m xL xH yL yH, then the rows)
        // carries more black, the text around it being the same.
        let raster_ink = |bytes: &[u8]| {
            let at = bytes
                .windows(3)
                .position(|w| w == [0x1D, b'v', b'0'])
                .expect("a raster");
            let h = &bytes[at + 4..at + 8];
            let (bpr, rows) = (h[0] as usize + 256 * h[1] as usize, h[2] as usize + 256 * h[3] as usize);
            bytes[at + 8..at + 8 + bpr * rows].iter().map(|b| b.count_ones()).sum::<u32>()
        };
        let rasterised = |p: &DeviceProfile, s| {
            let p = DeviceProfile { native_symbologies: vec![], ..p.clone() };
            render(&job(true, s), &p).unwrap()
        };
        let (narrow, wider) = (
            raster_ink(&rasterised(&plain, Symbology::Code39)),
            raster_ink(&rasterised(&wide, Symbology::Code39)),
        );
        assert!(wider > narrow, "the bars got wider: {narrow} -> {wider}");
        // A square code is left alone: it would take the whole slip.
        assert_eq!(
            raster_ink(&rasterised(&wide, Symbology::Qr)),
            raster_ink(&rasterised(&plain, Symbology::Qr))
        );
    }

    #[test]
    fn a_codes_card_prints_its_heading_and_every_code_in_every_mode() {
        use taskologic_core::print::{CodeLine, build_codes_job};
        let user = User {
            uid: 1,
            username: "alice".into(),
            is_admin: false,
            timezone: chrono_tz::UTC,
            prefs: UserPrefs::default(),
            has_pin: false,
            created_at: DateTime::from_timestamp(0, 0).unwrap(),
        };
        let lines = vec![
            CodeLine { label: "move left".into(), payload: "--1ML--".into(), symbology: Symbology::Code39, narrow: false },
            CodeLine { label: "press t".into(), payload: "--1Kt--".into(), symbology: Symbology::Code128, narrow: false },
        ];
        let job = build_codes_job("Navigation and keys", lines, &user, DateTime::from_timestamp(0, 0).unwrap());
        assert_eq!(job.kind, taskologic_core::print::PrintJobKind::Codes);
        // The plan ignores the printer's layout: heading, then the codes.
        let ops = crate::layout::plan(&job, 32, &crate::slip::SlipLayout::reminder());
        let labels: Vec<&str> = ops
            .iter()
            .filter_map(|o| match o {
                crate::layout::Op::Barcode { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(labels, vec!["move left", "press t"]);
        assert!(matches!(&ops[0], crate::layout::Op::Line(l) if l.text == "Navigation and keys" && l.bold));
        // Every output mode takes it: the bitmap is a picture, ESC/POS and
        // text carry the payloads as characters.
        for output in [OutputMode::EscPos, OutputMode::Bitmap, OutputMode::Text] {
            let profile = DeviceProfile { output, ..Default::default() };
            let bytes = render(&job, &profile).unwrap();
            assert!(!bytes.is_empty(), "{output:?}");
            if output != OutputMode::Bitmap {
                let text = String::from_utf8_lossy(&bytes);
                assert!(text.contains("--1ML--"), "{output:?}");
            }
        }
        let escpos = render(&job, &DeviceProfile { output: OutputMode::EscPos, ..Default::default() }).unwrap();
        assert_eq!(count(&escpos, &[0x1D, b'k']), 2, "two native codes");
    }

    #[test]
    fn a_narrow_code_is_rasterised_thin_even_on_a_printer_that_draws_its_own() {
        use taskologic_core::print::{CodeLine, build_codes_job};
        let user = User {
            uid: 1,
            username: "alice".into(),
            is_admin: false,
            timezone: chrono_tz::UTC,
            prefs: UserPrefs::default(),
            has_pin: false,
            created_at: DateTime::from_timestamp(0, 0).unwrap(),
        };
        let line = |narrow| CodeLine {
            label: "scanner check".into(),
            payload: "--1PING--".into(),
            symbology: Symbology::Code39,
            narrow,
        };
        let profile = show_everything(escpos());
        let thick = render(&build_codes_job("strip", vec![line(false)], &user, DateTime::from_timestamp(0, 0).unwrap()), &profile).unwrap();
        let thin = render(&build_codes_job("strip", vec![line(true)], &user, DateTime::from_timestamp(0, 0).unwrap()), &profile).unwrap();
        assert_eq!(count(&thick, &[0x1D, b'k']), 1, "the printer draws the normal one");
        assert_eq!(count(&thin, &[0x1D, b'k']), 0, "the thin one is a picture");
        assert_eq!(count(&thin, &[0x1D, b'v', b'0']), 1);
    }

    #[test]
    fn native_code39_when_supported_else_raster() {

        let both = show_everything(escpos());
        let bytes = render(&job(true, Symbology::Code39), &both).unwrap();
        assert_eq!(
            count(&bytes, &[0x1D, b'k', 69]),
            2,
            "start/pause and finish as native CODE39"
        );
        assert_eq!(count(&bytes, &[0x1D, b'v', b'0']), 0);

        let profile = DeviceProfile { native_symbologies: vec![], ..both };
        let bytes = render(&job(true, Symbology::Code39), &profile).unwrap();
        assert_eq!(count(&bytes, &[0x1D, b'k']), 0);
        assert_eq!(count(&bytes, &[0x1D, b'v', b'0']), 2, "rasterised instead");
    }

    #[test]
    fn datamatrix_is_always_raster() {
        // Raster width covers the whole line, whatever the paper is:
        // 58 mm prints 384 dots = 48 bytes, 80 mm prints 576 = 72.
        for (paper, bytes_per_row) in [
            (PaperWidth::Mm58, 48),
            (PaperWidth::Mm80, 72),
            (PaperWidth::Custom(60), 60),
        ] {
            let profile = DeviceProfile {
                paper,
                native_symbologies: Symbology::ALL.to_vec(),
                ..show_everything(escpos())
            };
            let bytes = render(&job(true, Symbology::DataMatrix), &profile).unwrap();
            assert_eq!(count(&bytes, &[0x1D, b'k']), 0, "{paper:?}");
            assert_eq!(count(&bytes, &[0x1D, b'v', b'0']), 2, "{paper:?}");
            let idx = bytes
                .windows(3)
                .position(|w| w == [0x1D, b'v', b'0'])
                .unwrap();
            assert_eq!(&bytes[idx + 4..idx + 6], &[bytes_per_row, 0], "{paper:?}");
        }
    }

    #[test]
    fn no_cutter_means_feed_only() {
        let profile = DeviceProfile { auto_cutter: false, ..escpos() };
        let bytes = render(&job(false, Symbology::Code39), &profile).unwrap();
        assert_eq!(count(&bytes, &[0x1D, b'V']), 0);
        assert!(bytes.ends_with(&[0x1B, b'd', 5]));
    }


    fn render_as(output: OutputMode) -> Vec<u8> {
        let profile = DeviceProfile { output, ..Default::default() };
        render(&job(true, Symbology::Code39), &profile).unwrap()
    }

    #[test]
    fn a_bitmap_slip_is_a_pbm_that_fits_inside_the_paper() {
        let bytes = render_as(OutputMode::Bitmap);
        let header = String::from_utf8_lossy(&bytes[..16]).to_string();
        // 576 dots of 80 mm roll, less the millimetre of slack that keeps the
        // image inside a page whose media is really 71.97 mm.
        assert!(header.starts_with("P4\n568 "), "{header:?}");
        // Header, then one bit per dot: 72 bytes a row for an 80 mm roll.
        let nl = bytes.iter().enumerate().filter(|(_, b)| **b == b'\n').nth(1).unwrap().0;
        let dims: Vec<usize> = String::from_utf8_lossy(&bytes[3..nl])
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect();
        assert_eq!(dims[0], 568);
        assert!(dims[1] > 200, "a slip with two barcodes is taller than this: {dims:?}");
        assert_eq!(bytes.len() - nl - 1, dims[0].div_ceil(8) * dims[1], "packed rows");
    }

    #[test]
    fn a_bitmap_slip_actually_has_ink_on_it() {
        let bytes = render_as(OutputMode::Bitmap);
        let nl = bytes.iter().enumerate().filter(|(_, b)| **b == b'\n').nth(1).unwrap().0;
        let set: u32 = bytes[nl + 1..].iter().map(|b| b.count_ones()).sum();
        // Text, two barcodes and their labels: not a blank page, and not a
        // solid black one either.
        let total = ((bytes.len() - nl - 1) * 8) as u32;
        assert!(set > 2_000, "too few dots to be a drawn slip: {set}");
        assert!(set < total / 2, "mostly black, something is inverted: {set}/{total}");
    }

    #[test]
    fn a_text_slip_is_readable_and_says_what_the_barcodes_were() {
        let bytes = render_as(OutputMode::Text);
        let out = String::from_utf8(bytes).expect("text mode is UTF-8");
        assert!(out.contains("Wasser für die Pflanzen"), "{out}");
        assert!(out.contains("Küche"));
        assert!(out.contains("Due: 2027-01-15 09:00"));
        // No bars can be drawn, so the payload goes on as text instead.
        assert!(out.contains("scan to finish: "), "{out}");
        assert!(!out.contains('\x1b'), "no escape codes in text mode");
    }

    #[test]
    fn the_mode_decides_how_wide_a_line_is() {
        let p = |output| DeviceProfile { output, ..Default::default() };
        // The bitmap font is wider than the printer's built in one.
        assert_eq!(p(OutputMode::EscPos).columns(), 48);
        assert_eq!(p(OutputMode::Text).columns(), 48);
        // One fewer than the 36 the width alone would give: the drawn slip
        // leaves a millimetre of slack so it fits inside the page.
        assert_eq!(p(OutputMode::Bitmap).columns(), 35);
    }

    #[test]
    fn every_mode_renders_every_symbology_without_failing() {
        for output in [OutputMode::EscPos, OutputMode::Bitmap, OutputMode::Text] {
            for sym in Symbology::ALL {
                let profile = DeviceProfile { output, ..Default::default() };
                let out = render(&job(true, sym), &profile);
                assert!(out.is_ok(), "{output:?} {sym:?}: {out:?}");
                assert!(!out.unwrap().is_empty(), "{output:?} {sym:?} produced nothing");
            }
        }
    }

    #[test]
    fn a_sample_barcode_is_drawn_wherever_one_can_be() {
        use taskologic_core::barcode::ScanAction;
        use taskologic_core::print::{SAMPLE_BARCODE_PAYLOAD, sample_barcode};

        for output in [OutputMode::EscPos, OutputMode::Bitmap, OutputMode::Text] {
            let mut j = job(false, Symbology::Code39);
            j.barcodes = vec![sample_barcode(Symbology::Code39, ScanAction::Finish)];
            let profile = DeviceProfile { output, ..Default::default() };
            let bytes = render(&j, &profile).unwrap();

            match output {
                // The printer draws it, from its own barcode command.
                OutputMode::EscPos => {
                    assert_eq!(count(&bytes, &[0x1D, b'k']), 1, "one native barcode");
                    assert!(count(&bytes, b"sample barcode") == 1, "labelled honestly");
                }
                // We draw it, so the slip simply gets taller and inkier.
                OutputMode::Bitmap => {
                    // The same slip with nothing to scan is shorter.
                    let mut bare = job(false, Symbology::Code39);
                    bare.barcodes.clear();
                    let plain = render(&bare, &profile).unwrap();
                    assert!(bytes.len() > plain.len(), "a barcode takes room");
                }
                // Nothing to draw with, so the payload goes on as text.
                OutputMode::Text => {
                    let out = String::from_utf8(bytes).unwrap();
                    assert!(out.contains(SAMPLE_BARCODE_PAYLOAD), "{out}");
                    assert!(out.contains("sample barcode"), "{out}");
                }
            }
        }
    }

    #[test]
    fn the_sample_payload_fits_every_symbology_a_user_could_have_set() {
        use taskologic_core::barcode::ScanAction;
        use taskologic_core::print::{SAMPLE_BARCODE_PAYLOAD, sample_barcode};

        // Whichever scanner format the user has, the test slip uses it, so
        // the sample payload has to encode in all of them.
        for sym in Symbology::ALL {
            assert!(
                symbology::encode(sym, SAMPLE_BARCODE_PAYLOAD).is_ok(),
                "{sym:?} cannot carry {SAMPLE_BARCODE_PAYLOAD}"
            );
            let mut j = job(false, sym);
            j.barcodes = vec![sample_barcode(sym, ScanAction::Finish)];
            for output in [OutputMode::EscPos, OutputMode::Bitmap, OutputMode::Text] {
                let profile = DeviceProfile { output, ..Default::default() };
                assert!(render(&j, &profile).is_ok(), "{output:?} {sym:?}");
            }
        }
    }

    #[test]
    fn a_turned_slip_is_the_same_picture_the_other_way_round() {
        use crate::profile::Rotation;

        let upright = DeviceProfile { output: OutputMode::Bitmap, ..Default::default() };
        let half = DeviceProfile { rotation: Rotation::Cw180, ..upright.clone() };
        let quarter = DeviceProfile { rotation: Rotation::Cw90, ..upright.clone() };
        let j = job(true, Symbology::Code39);

        let dims = |p: &DeviceProfile| {
            let b = render(&j, p).unwrap();
            let nl = b.iter().enumerate().filter(|(_, c)| **c == b'\n').nth(1).unwrap().0;
            let d: Vec<usize> = String::from_utf8_lossy(&b[3..nl])
                .split_whitespace()
                .map(|n| n.parse().unwrap())
                .collect();
            (d[0], d[1])
        };
        let (w, h) = dims(&upright);
        assert_eq!(dims(&half), (w, h), "a half turn keeps the shape");
        assert_eq!(dims(&quarter), (h, w), "a quarter turn swaps it");
        // Rotation is a bitmap notion; the other modes ignore it entirely.
        let text = DeviceProfile { output: OutputMode::Text, rotation: Rotation::Cw90, ..Default::default() };
        let plain = DeviceProfile { output: OutputMode::Text, ..Default::default() };
        assert_eq!(render(&j, &text).unwrap(), render(&j, &plain).unwrap());
    }
}
