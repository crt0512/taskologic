//! What character set the printer expects on the wire.
//!
//! Modern printers take UTF-8 straight, which is the default. The single
//! byte codepages are for older models, typically the ones hanging off a
//! serial or parallel port: UTF-8 goes in, one byte per character comes
//! out, and anything the codepage cannot show becomes a question mark
//! rather than garbage.

use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Codepage {
    /// Pass the text through as UTF-8, selecting no ESC/POS table at all.
    #[default]
    Utf8,
    Cp437,
    Cp850,
    Cp858,
    Wpc1252,
}

impl Codepage {
    /// The `n` in `ESC t n`, per the Epson table most clones follow.
    /// `None` for UTF-8, which has no table to select: leaving the command
    /// out means whatever the printer already uses stays put.
    pub fn escpos_number(self) -> Option<u8> {
        match self {
            Codepage::Utf8 => None,
            Codepage::Cp437 => Some(0),
            Codepage::Cp850 => Some(2),
            Codepage::Wpc1252 => Some(16),
            Codepage::Cp858 => Some(19),
        }
    }

    fn table(self) -> Option<&'static [char; 128]> {
        match self {
            Codepage::Utf8 => None,
            Codepage::Cp437 => Some(&CP437),
            Codepage::Cp850 => Some(&CP850),
            Codepage::Cp858 => Some(&CP858),
            Codepage::Wpc1252 => Some(&WPC1252),
        }
    }

    /// One byte for one character. UTF-8 has no such mapping, so anything
    /// outside ASCII comes back as `?`; use [`Codepage::encode`] instead,
    /// which encodes the whole string properly.
    pub fn encode_char(self, c: char) -> u8 {
        if c.is_ascii() {
            return c as u8;
        }
        // The replacement character marks holes in the tables below.
        if c == '\u{fffd}' {
            return b'?';
        }
        let Some(table) = self.table() else {
            return b'?';
        };
        table
            .iter()
            .position(|t| *t == c)
            .map(|i| 0x80 + i as u8)
            .unwrap_or(b'?')
    }

    pub fn encode(self, s: &str) -> Vec<u8> {
        match self {
            Codepage::Utf8 => s.as_bytes().to_vec(),
            _ => s.chars().map(|c| self.encode_char(c)).collect(),
        }
    }
}

const CP437: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', 'É', 'æ', 'Æ',
    'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', 'á', 'í', 'ó', 'ú', 'ñ', 'Ñ',
    'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕',
    '╣', '║', '╗', '╝', '╜', '╛', '┐', '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦',
    '╠', '═', '╬', '╧', '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐',
    '▀', 'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', '≡', '±',
    '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}',
];

const CP850: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', 'É', 'æ', 'Æ',
    'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', 'ø', '£', 'Ø', '×', 'ƒ', 'á', 'í', 'ó', 'ú', 'ñ', 'Ñ',
    'ª', 'º', '¿', '®', '¬', '½', '¼', '¡', '«', '»', '░', '▒', '▓', '│', '┤', 'Á', 'Â', 'À', '©',
    '╣', '║', '╗', '╝', '¢', '¥', '┐', '└', '┴', '┬', '├', '─', '┼', 'ã', 'Ã', '╚', '╔', '╩', '╦',
    '╠', '═', '╬', '¤', 'ð', 'Ð', 'Ê', 'Ë', 'È', 'ı', 'Í', 'Î', 'Ï', '┘', '┌', '█', '▄', '¦', 'Ì',
    '▀', 'Ó', 'ß', 'Ô', 'Ò', 'õ', 'Õ', 'µ', 'þ', 'Þ', 'Ú', 'Û', 'Ù', 'ý', 'Ý', '¯', '´', '\u{ad}',
    '±', '‗', '¾', '¶', '§', '÷', '¸', '°', '¨', '·', '¹', '³', '²', '■', '\u{a0}',
];

// CP858 is CP850 with the euro sign at 0xD5 in place of dotless i.
const CP858: [char; 128] = {
    let mut t = CP850;
    t[0xD5 - 0x80] = '€';
    t
};

const WPC1252: [char; 128] = [
    '€', '\u{fffd}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{fffd}', 'Ž',
    '\u{fffd}', '\u{fffd}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{fffd}',
    'ž', 'Ÿ', '\u{a0}', '¡', '¢', '£', '¤', '¥', '¦', '§', '¨', '©', 'ª', '«', '¬', '\u{ad}', '®',
    '¯', '°', '±', '²', '³', '´', 'µ', '¶', '·', '¸', '¹', 'º', '»', '¼', '½', '¾', '¿', 'À', 'Á',
    'Â', 'Ã', 'Ä', 'Å', 'Æ', 'Ç', 'È', 'É', 'Ê', 'Ë', 'Ì', 'Í', 'Î', 'Ï', 'Ð', 'Ñ', 'Ò', 'Ó', 'Ô',
    'Õ', 'Ö', '×', 'Ø', 'Ù', 'Ú', 'Û', 'Ü', 'Ý', 'Þ', 'ß', 'à', 'á', 'â', 'ã', 'ä', 'å', 'æ', 'ç',
    'è', 'é', 'ê', 'ë', 'ì', 'í', 'î', 'ï', 'ð', 'ñ', 'ò', 'ó', 'ô', 'õ', 'ö', '÷', 'ø', 'ù', 'ú',
    'û', 'ü', 'ý', 'þ', 'ÿ',
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_passes_the_string_through_untouched() {
        assert_eq!(Codepage::Utf8.encode("Käse"), "Käse".as_bytes());
        assert_eq!(Codepage::Utf8.encode("日本"), "日本".as_bytes());
        assert_eq!(Codepage::Utf8.escpos_number(), None, "no ESC t for UTF-8");
    }

    #[test]
    fn ascii_passes_through_and_umlauts_map() {
        assert_eq!(Codepage::Cp437.encode("Milch"), b"Milch");
        assert_eq!(Codepage::Cp437.encode("Käse"), vec![b'K', 0x84, b's', b'e']);
        assert_eq!(Codepage::Cp850.encode("Käse"), vec![b'K', 0x84, b's', b'e']);
        assert_eq!(
            Codepage::Wpc1252.encode("Käse"),
            vec![b'K', 0xE4, b's', b'e']
        );
        assert_eq!(Codepage::Cp858.encode("5€"), vec![b'5', 0xD5]);
        assert_eq!(Codepage::Cp850.encode("5€"), vec![b'5', b'?']);
        assert_eq!(Codepage::Wpc1252.encode("5€"), vec![b'5', 0x80]);
    }

    #[test]
    fn unmappable_becomes_question_mark() {
        assert_eq!(Codepage::Cp437.encode("日本"), b"??");
        // The replacement char must never match an input.
        assert_eq!(Codepage::Wpc1252.encode("\u{fffd}"), b"?");
    }

    #[test]
    fn tables_have_no_duplicates_apart_from_holes() {
        for cp in [
            Codepage::Cp437,
            Codepage::Cp850,
            Codepage::Cp858,
            Codepage::Wpc1252,
        ] {
            let t = cp.table().expect("single byte codepages have a table");
            for (i, c) in t.iter().enumerate() {
                if *c == '\u{fffd}' {
                    continue;
                }
                assert_eq!(
                    t.iter().filter(|x| *x == c).count(),
                    1,
                    "{cp:?} duplicates {c:?} at {i}"
                );
            }
        }
    }
}
