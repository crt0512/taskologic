//! One trait over three barcode crates. Nothing outside this module knows
//! which crate draws what.

use taskologic_core::print::Symbology;

#[derive(Debug, thiserror::Error)]
pub enum SymbologyError {
    #[error("{0} cannot encode {1:?}")]
    Unencodable(&'static str, String),
}

/// A black and white module grid. 1D codes have a height of one and get
/// stretched by the renderer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitMatrix {
    pub width: usize,
    pub height: usize,
    bits: Vec<bool>,
}

impl BitMatrix {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            bits: vec![false; width * height],
        }
    }

    pub fn get(&self, x: usize, y: usize) -> bool {
        x < self.width && y < self.height && self.bits[y * self.width + x]
    }

    pub fn set(&mut self, x: usize, y: usize, on: bool) {
        if x < self.width && y < self.height {
            self.bits[y * self.width + x] = on;
        }
    }

    pub fn is_1d(&self) -> bool {
        self.height == 1
    }

    /// Scale every module to `scale` x `scale` dots, stretch 1D codes to
    /// `bar_height` dots, and surround with `quiet` modules of white.
    pub fn scaled(&self, scale: usize, bar_height: usize, quiet: usize) -> BitMatrix {
        let scale = scale.max(1);
        let out_w = (self.width + 2 * quiet) * scale;
        let rows = if self.is_1d() {
            bar_height.max(1)
        } else {
            self.height * scale
        };
        let out_h = rows + 2 * quiet * scale;
        let mut out = BitMatrix::new(out_w, out_h);
        for y in 0..rows {
            let src_y = if self.is_1d() { 0 } else { y / scale };
            for x in 0..self.width * scale {
                if self.get(x / scale, src_y) {
                    out.set(x + quiet * scale, y + quiet * scale, true);
                }
            }
        }
        out
    }

    /// Pack rows MSB first for `GS v 0`, left aligned inside `total_width`
    /// dots (or the matrix width, whichever is larger).
    pub fn packed_rows(&self, total_width: usize) -> (u16, Vec<Vec<u8>>) {
        let width = total_width.max(self.width);
        let bytes_per_row = width.div_ceil(8);
        let rows = (0..self.height)
            .map(|y| {
                let mut row = vec![0u8; bytes_per_row];
                for x in 0..self.width {
                    if self.get(x, y) {
                        row[x / 8] |= 0x80 >> (x % 8);
                    }
                }
                row
            })
            .collect();
        (bytes_per_row as u16, rows)
    }

    /// Shift the image right so it sits centred in `total_width` dots.
    pub fn centered_in(&self, total_width: usize) -> BitMatrix {
        if total_width <= self.width {
            return self.clone();
        }
        let pad = (total_width - self.width) / 2;
        let mut out = BitMatrix::new(total_width, self.height);
        for y in 0..self.height {
            for x in 0..self.width {
                if self.get(x, y) {
                    out.set(x + pad, y, true);
                }
            }
        }
        out
    }
}

/// Encode `payload` as `symbology`. Payloads are the uppercase alphanumeric
/// strings from `taskologic_core::barcode`, every symbology here can carry them.
pub fn encode(symbology: Symbology, payload: &str) -> Result<BitMatrix, SymbologyError> {
    match symbology {
        Symbology::Code39 => {
            let code = barcoders::sym::code39::Code39::new(payload)
                .map_err(|_| SymbologyError::Unencodable("CODE39", payload.into()))?;
            Ok(from_1d(&code.encode()))
        }
        Symbology::Code128 => {
            // The crate wants the initial code set as a marker character.
            // Set B covers all printable ASCII, which is all we ever emit.
            let code = barcoders::sym::code128::Code128::new(format!("\u{0181}{payload}"))
                .map_err(|_| SymbologyError::Unencodable("CODE128", payload.into()))?;
            Ok(from_1d(&code.encode()))
        }
        Symbology::Qr => {
            let qr = qrcode::QrCode::new(payload.as_bytes())
                .map_err(|_| SymbologyError::Unencodable("QR", payload.into()))?;
            let w = qr.width();
            let mut m = BitMatrix::new(w, w);
            for (i, c) in qr.to_colors().into_iter().enumerate() {
                m.set(i % w, i / w, c == qrcode::Color::Dark);
            }
            Ok(m)
        }
        Symbology::DataMatrix => {
            let dm = datamatrix::DataMatrix::encode_str(payload, datamatrix::SymbolList::default())
                .map_err(|_| SymbologyError::Unencodable("DataMatrix", payload.into()))?;
            let bm = dm.bitmap();
            let mut m = BitMatrix::new(bm.width(), bm.height());
            for (x, y) in bm.pixels() {
                m.set(x, y, true);
            }
            Ok(m)
        }
    }
}

fn from_1d(modules: &[u8]) -> BitMatrix {
    let mut m = BitMatrix::new(modules.len(), 1);
    for (x, v) in modules.iter().enumerate() {
        m.set(x, 0, *v != 0);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYLOAD: &str = "..1SK4M9Q2J";

    #[test]
    fn every_symbology_encodes_a_payload() {
        for s in Symbology::ALL {
            let m = encode(s, PAYLOAD).unwrap_or_else(|e| panic!("{s:?}: {e}"));
            assert!(m.width > 0 && m.height > 0);
            let black = (0..m.height)
                .flat_map(|y| (0..m.width).map(move |x| (x, y)))
                .filter(|(x, y)| m.get(*x, *y))
                .count();
            assert!(black > 0, "{s:?} produced a blank code");
            match s {
                Symbology::Code39 | Symbology::Code128 => assert!(m.is_1d()),
                Symbology::Qr => {
                    assert!(!m.is_1d());
                    assert_eq!(m.width, m.height);
                }
                // DataMatrix picks a rectangular symbol for short payloads.
                Symbology::DataMatrix => assert!(m.height > 1),
            }
        }
        // Both magics survive CODE39, which is the whole reason for choosing them.
        assert!(encode(Symbology::Code39, "--1F0000015").is_ok());
    }

    #[test]
    fn scaling_and_packing() {
        let mut m = BitMatrix::new(2, 1);
        m.set(0, 0, true);
        let s = m.scaled(2, 3, 1);
        assert_eq!((s.width, s.height), (8, 7));
        // Row 2 (after the quiet zone) has the first module as two black dots at x=2,3.
        assert!(s.get(2, 2) && s.get(3, 2) && !s.get(4, 2) && !s.get(1, 2));
        let (bpr, rows) = s.packed_rows(16);
        assert_eq!(bpr, 2);
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[2], vec![0b0011_0000, 0]);
        let c = s.centered_in(16);
        assert_eq!(c.width, 16);
        assert!(c.get(6, 2) && c.get(7, 2));
    }
}
