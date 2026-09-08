//! A small ESC/POS byte builder. Hand rolled on purpose, the protocol is
//! simple and we want control over cut, codepage and raster output.

use crate::codepage::Codepage;
use crate::layout::Align;

const ESC: u8 = 0x1B;
const GS: u8 = 0x1D;

/// Native 1D barcode types by their `GS k` function B number.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NativeBarcode {
    Code39 = 69,
    Code128 = 73,
}

#[derive(Clone, Debug)]
pub struct EscPos {
    buf: Vec<u8>,
    codepage: Codepage,
}

impl EscPos {
    /// Starts with `ESC @` (reset), then the codepage select if the
    /// character set needs one. UTF-8 does not: there is no table to pick,
    /// so the reset is the whole preamble.
    pub fn new(codepage: Codepage) -> Self {
        let mut e = Self {
            buf: Vec::with_capacity(1024),
            codepage,
        };
        e.buf.extend_from_slice(&[ESC, b'@']);
        if let Some(n) = codepage.escpos_number() {
            e.buf.extend_from_slice(&[ESC, b't', n]);
        }
        e
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn raw(&mut self, bytes: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(bytes);
        self
    }

    /// Text in the selected codepage, no newline.
    pub fn text(&mut self, s: &str) -> &mut Self {
        self.buf.extend(self.codepage.encode(s));
        self
    }

    pub fn line(&mut self, s: &str) -> &mut Self {
        self.text(s);
        self.buf.push(b'\n');
        self
    }

    pub fn newline(&mut self) -> &mut Self {
        self.buf.push(b'\n');
        self
    }

    pub fn feed(&mut self, lines: u8) -> &mut Self {
        self.buf.extend_from_slice(&[ESC, b'd', lines]);
        self
    }

    pub fn bold(&mut self, on: bool) -> &mut Self {
        self.buf.extend_from_slice(&[ESC, b'E', u8::from(on)]);
        self
    }

    pub fn underline(&mut self, on: bool) -> &mut Self {
        self.buf.extend_from_slice(&[ESC, b'-', u8::from(on)]);
        self
    }

    /// Character size multiplier, 1 to 8 each way.
    pub fn size(&mut self, width: u8, height: u8) -> &mut Self {
        let w = width.clamp(1, 8) - 1;
        let h = height.clamp(1, 8) - 1;
        self.buf.extend_from_slice(&[GS, b'!', (w << 4) | h]);
        self
    }

    pub fn align(&mut self, a: Align) -> &mut Self {
        let n = match a {
            Align::Left => 0,
            Align::Center => 1,
            Align::Right => 2,
        };
        self.buf.extend_from_slice(&[ESC, b'a', n]);
        self
    }

    /// Partial cut after feeding the paper clear of the blade.
    pub fn cut(&mut self) -> &mut Self {
        self.feed(4);
        self.buf.extend_from_slice(&[GS, b'V', 1]);
        self
    }

    /// One of the printer's own barcode commands, with the human readable
    /// text printed underneath. `data` must already be valid for the type.
    pub fn native_barcode(
        &mut self,
        kind: NativeBarcode,
        data: &str,
        height: u8,
        module_width: u8,
    ) -> &mut Self {
        self.buf.extend_from_slice(&[GS, b'h', height]);
        self.buf
            .extend_from_slice(&[GS, b'w', module_width.clamp(2, 6)]);
        self.buf.extend_from_slice(&[GS, b'H', 2]);
        let mut payload: Vec<u8> = Vec::new();
        if kind == NativeBarcode::Code128 {
            // Select code set B up front, printers do not guess.
            payload.extend_from_slice(b"{B");
        }
        payload.extend(data.bytes());
        self.buf
            .extend_from_slice(&[GS, b'k', kind as u8, payload.len() as u8]);
        self.buf.extend(payload);
        self.newline()
    }

    /// `GS v 0` raster bit image. `rows` are packed MSB first, 1 = black,
    /// every row exactly `bytes_per_row` long.
    pub fn raster(&mut self, bytes_per_row: u16, rows: &[Vec<u8>]) -> &mut Self {
        debug_assert!(rows.iter().all(|r| r.len() == bytes_per_row as usize));
        let height = rows.len() as u16;
        self.buf.extend_from_slice(&[GS, b'v', b'0', 0]);
        self.buf.extend_from_slice(&bytes_per_row.to_le_bytes());
        self.buf.extend_from_slice(&height.to_le_bytes());
        for r in rows {
            self.buf.extend_from_slice(r);
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_selects_no_table() {
        let e = EscPos::new(Codepage::Utf8);
        assert_eq!(e.into_bytes(), vec![ESC, b'@']);
    }

    #[test]
    fn starts_with_reset_and_codepage() {
        let e = EscPos::new(Codepage::Cp858);
        assert_eq!(e.into_bytes(), vec![ESC, b'@', ESC, b't', 19]);
    }

    #[test]
    fn native_barcode_uses_function_b_with_length() {
        let mut e = EscPos::new(Codepage::Cp437);
        e.native_barcode(NativeBarcode::Code39, "..1SABCDEFG", 80, 2);
        let b = e.into_bytes();
        let idx = b.windows(2).position(|w| w == [GS, b'k']).unwrap();
        assert_eq!(&b[idx..idx + 4], &[GS, b'k', 69, 11]);
        assert_eq!(&b[idx + 4..idx + 15], b"..1SABCDEFG");

        let mut e = EscPos::new(Codepage::Cp437);
        e.native_barcode(NativeBarcode::Code128, "AB", 80, 2);
        let b = e.into_bytes();
        let idx = b.windows(2).position(|w| w == [GS, b'k']).unwrap();
        assert_eq!(&b[idx..idx + 8], &[GS, b'k', 73, 4, b'{', b'B', b'A', b'B']);
    }

    #[test]
    fn raster_header_is_little_endian() {
        let mut e = EscPos::new(Codepage::Cp437);
        e.raster(2, &[vec![0xFF, 0x00], vec![0x0F, 0xF0], vec![0, 0]]);
        let b = e.into_bytes();
        let idx = b.windows(3).position(|w| w == [GS, b'v', b'0']).unwrap();
        assert_eq!(&b[idx..idx + 8], &[GS, b'v', b'0', 0, 2, 0, 3, 0]);
        assert_eq!(&b[idx + 8..], &[0xFF, 0, 0x0F, 0xF0, 0, 0]);
    }
}
