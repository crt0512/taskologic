//! Draws a slip as a 1 bit image, for printers that have no fonts of their
//! own.
//!
//! A Star TSP100 is the case this exists for: it holds no font sets at all,
//! so text sent as ESC/POS is simply discarded. Drawing the whole slip here
//! and letting the queue's driver rasterise it works on any printer CUPS can
//! drive, and unlike plain text it keeps the barcodes.

use font8x8::{BASIC_FONTS, LATIN_FONTS, UnicodeFonts};

use crate::layout::{Align, Op, TextLine};
use crate::profile::{BITMAP_SCALE, DeviceProfile, Rotation};
use crate::render::RenderError;
use crate::symbology;

/// Blank dots left above and below a barcode so it is not crowded.
const BARCODE_MARGIN: usize = 8;
const BAR_HEIGHT_DOTS: usize = 64;
const BAR_MODULE_DOTS: usize = 2;
const MATRIX_MODULE_DOTS: usize = 4;

/// A 1 bit canvas, one bit per dot, that grows downwards as lines are added.
/// Bit set means a black dot, which is what both PBM and the ESC/POS raster
/// commands mean by a 1.
pub struct Canvas {
    width: usize,
    bytes_per_row: usize,
    rows: Vec<u8>,
}

impl Canvas {
    pub fn new(width: usize) -> Self {
        Self { width, bytes_per_row: width.div_ceil(8), rows: Vec::new() }
    }

    pub fn height(&self) -> usize {
        if self.bytes_per_row == 0 { 0 } else { self.rows.len() / self.bytes_per_row }
    }

    /// Make sure the canvas has at least this many rows.
    fn reserve_to(&mut self, height: usize) {
        let want = height * self.bytes_per_row;
        if self.rows.len() < want {
            self.rows.resize(want, 0);
        }
    }

    pub fn feed(&mut self, dots: usize) {
        let h = self.height();
        self.reserve_to(h + dots);
    }

    fn get(&self, x: usize, y: usize) -> bool {
        if x >= self.width {
            return false;
        }
        self.rows
            .get(y * self.bytes_per_row + x / 8)
            .is_some_and(|b| b & (0x80 >> (x % 8)) != 0)
    }

    /// The same image turned on the paper. A quarter turn swaps the sides,
    /// which is the caller's problem: a slip longer than the roll is wide
    /// comes out clipped or shrunk by the driver, and the panel says so.
    pub fn rotate(&self, r: Rotation) -> Canvas {
        if r == Rotation::Upright {
            return Canvas { width: self.width, bytes_per_row: self.bytes_per_row, rows: self.rows.clone() };
        }
        let (w, h) = (self.width, self.height());
        let (nw, nh) = if r.swaps_sides() { (h, w) } else { (w, h) };
        let mut out = Canvas::new(nw);
        out.reserve_to(nh);
        for y in 0..h {
            for x in 0..w {
                if !self.get(x, y) {
                    continue;
                }
                let (nx, ny) = match r {
                    Rotation::Upright => (x, y),
                    Rotation::Cw90 => (h - 1 - y, x),
                    Rotation::Cw180 => (w - 1 - x, h - 1 - y),
                    Rotation::Cw270 => (y, w - 1 - x),
                };
                out.set(nx, ny);
            }
        }
        out
    }

    fn set(&mut self, x: usize, y: usize) {
        if x >= self.width {
            return;
        }
        let idx = y * self.bytes_per_row + x / 8;
        if let Some(b) = self.rows.get_mut(idx) {
            *b |= 0x80 >> (x % 8);
        }
    }

    /// Blit packed rows, as `symbology` hands them out, with the top left
    /// corner at `top`. The rows are already the full width of the paper.
    fn blit(&mut self, top: usize, rows: &[Vec<u8>]) {
        self.reserve_to(top + rows.len());
        for (y, row) in rows.iter().enumerate() {
            for x in 0..row.len() * 8 {
                if row[x / 8] & (0x80 >> (x % 8)) != 0 {
                    self.set(x, top + y);
                }
            }
        }
    }

    /// One line of text, at `scale` times the font's 8x8 cell.
    pub fn text(&mut self, line: &TextLine) {
        let scale = BITMAP_SCALE * line.scale.max(1) as usize;
        let cell = 8 * scale;
        let chars: Vec<char> = line.text.chars().collect();
        let top = self.height();
        self.reserve_to(top + cell);
        let used = chars.len() * cell;
        let left = match line.align {
            Align::Left => 0,
            Align::Center => self.width.saturating_sub(used) / 2,
            Align::Right => self.width.saturating_sub(used),
        };
        for (i, c) in chars.iter().enumerate() {
            let Some(glyph) = glyph(*c) else { continue };
            let x0 = left + i * cell;
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..8 {
                    // font8x8 packs the leftmost pixel in the low bit.
                    if bits & (1 << col) == 0 {
                        continue;
                    }
                    for dy in 0..scale {
                        for dx in 0..scale {
                            self.set(x0 + col * scale + dx, top + row * scale + dy);
                        }
                    }
                    // Bold is a second pass one dot to the right.
                    if line.bold {
                        for dy in 0..scale {
                            self.set(x0 + col * scale + scale, top + row * scale + dy);
                        }
                    }
                }
            }
        }
    }

    /// The image as a binary PBM, which CUPS types and filters on its own.
    pub fn into_pbm(self) -> Vec<u8> {
        let mut out = format!("P4\n{} {}\n", self.width, self.height()).into_bytes();
        out.extend_from_slice(&self.rows);
        out
    }
}

/// The 8x8 cell for a character, from the basic set first and then Latin-1
/// so that ä ö ü ß come out as themselves. Anything with no glyph is skipped
/// rather than drawn as a box.
fn glyph(c: char) -> Option<[u8; 8]> {
    BASIC_FONTS.get(c).or_else(|| LATIN_FONTS.get(c))
}

/// Draw a planned slip and return it as a PBM image.
pub fn render(ops: &[Op], profile: &DeviceProfile) -> Result<Vec<u8>, RenderError> {
    let width = profile.dots();
    let mut c = Canvas::new(width);
    for op in ops {
        match op {
            Op::Line(l) => c.text(l),
            Op::Blank => c.feed(8 * BITMAP_SCALE),
            Op::Barcode { code, label } => {
                c.text(&TextLine {
                    text: (*label).to_string(),
                    align: Align::Center,
                    bold: false,
                    scale: 1,
                });
                let m = symbology::encode(code.symbology, &code.payload)?;
                let scale = if m.is_1d() { BAR_MODULE_DOTS } else { MATRIX_MODULE_DOTS };
                let img = m.scaled(scale, BAR_HEIGHT_DOTS, 2).centered_in(width);
                let (_, rows) = img.packed_rows(width);
                c.feed(BARCODE_MARGIN);
                let top = c.height();
                c.blit(top, &rows);
                c.feed(BARCODE_MARGIN);
                c.text(&TextLine {
                    text: code.payload.clone(),
                    align: Align::Center,
                    bold: false,
                    scale: 1,
                });
            }
            // The driver cuts, and it needs paper clear of the head first.
            Op::End => c.feed(8 * BITMAP_SCALE * 3),
        }
    }
    Ok(c.rotate(profile.rotation).into_pbm())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::PaperWidth;

    /// A canvas with one dot set, so a turn is easy to follow.
    fn corner() -> Canvas {
        let mut c = Canvas::new(16);
        c.feed(8);
        c.set(1, 0);
        c
    }

    #[test]
    fn upright_changes_nothing() {
        let c = corner().rotate(Rotation::Upright);
        assert!(c.get(1, 0));
        assert_eq!((c.width, c.height()), (16, 8));
    }

    #[test]
    fn a_half_turn_puts_the_dot_in_the_opposite_corner() {
        let c = corner().rotate(Rotation::Cw180);
        assert!(c.get(16 - 1 - 1, 8 - 1), "top left goes to bottom right");
        assert!(!c.get(1, 0));
        assert_eq!((c.width, c.height()), (16, 8), "a half turn keeps the shape");
    }

    #[test]
    fn a_quarter_turn_swaps_the_sides() {
        let c = corner().rotate(Rotation::Cw90);
        assert_eq!((c.width, c.height()), (8, 16), "width and height trade places");
        // (1,0) clockwise into an 8 wide canvas lands at (h-1-y, x) = (7, 1).
        assert!(c.get(7, 1));
    }

    #[test]
    fn turning_four_times_comes_back_to_where_it_started() {
        let start = corner();
        let mut c = start.rotate(Rotation::Cw90);
        for _ in 0..3 {
            c = c.rotate(Rotation::Cw90);
        }
        assert_eq!((c.width, c.height()), (start.width, start.height()));
        for y in 0..start.height() {
            for x in 0..start.width {
                assert_eq!(c.get(x, y), start.get(x, y), "dot at {x},{y}");
            }
        }
    }

    #[test]
    fn a_turned_slip_keeps_every_dot_it_had() {
        let profile = DeviceProfile { paper: PaperWidth::Mm58, ..Default::default() };
        let mut c = Canvas::new(profile.dots());
        c.text(&crate::layout::TextLine {
            text: "Küche 123".to_string(),
            align: Align::Left,
            bold: true,
            scale: 1,
        });
        let ink = |c: &Canvas| c.rows.iter().map(|b| b.count_ones()).sum::<u32>();
        let before = ink(&c);
        assert!(before > 0);
        for r in [Rotation::Cw90, Rotation::Cw180, Rotation::Cw270] {
            assert_eq!(ink(&c.rotate(r)), before, "{r:?} lost or gained dots");
        }
    }
}
