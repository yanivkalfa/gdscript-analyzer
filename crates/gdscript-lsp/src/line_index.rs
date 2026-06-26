//! THE load-bearing correctness component (Playbook §3): convert between our `u32` UTF-8 **byte
//! offsets** and LSP `(line, character)` positions, honoring the negotiated position encoding.
//!
//! LSP `Position.character` is **UTF-16 code units by default** (LSP 3.17 lets the client/server
//! negotiate UTF-8/UTF-16/UTF-32). Get this wrong and every range is subtly off on any line with a
//! non-ASCII character. This module is the *only* place that knows about encoding; every range we
//! send or receive funnels through it. Built once per document text version (a small `Vec<u32>` of
//! line-start offsets); conversions take the document text, kept paired with the index in the VFS.

use lsp_types::{Position, PositionEncodingKind};

/// The position encoding negotiated in `initialize`. UTF-16 is the mandatory baseline; UTF-8 is
/// preferred when the client offers it (then `character` is our native byte column — a no-op).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PositionEncoding {
    /// `character` = UTF-8 byte offset within the line (our native form).
    Utf8,
    /// `character` = UTF-16 code units (the LSP default + mandatory fallback).
    Utf16,
    /// `character` = Unicode scalar values (code points).
    Utf32,
}

impl PositionEncoding {
    /// The protocol value to advertise in `ServerCapabilities.positionEncoding`.
    #[must_use]
    pub fn to_lsp(self) -> PositionEncodingKind {
        match self {
            Self::Utf8 => PositionEncodingKind::UTF8,
            Self::Utf16 => PositionEncodingKind::UTF16,
            Self::Utf32 => PositionEncodingKind::UTF32,
        }
    }

    /// The width of `c` in this encoding's units.
    fn width(self, c: char) -> u32 {
        match self {
            Self::Utf8 => u32::try_from(c.len_utf8()).unwrap_or(1),
            Self::Utf16 => u32::try_from(c.len_utf16()).unwrap_or(1),
            Self::Utf32 => 1,
        }
    }
}

/// Line-start byte offsets for one document text. Cheap to rebuild per edit.
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Byte offset of the start of each line. `line_starts[0] == 0`; always non-empty.
    line_starts: Vec<u32>,
    /// Total document length in bytes (for clamping).
    len: u32,
}

impl LineIndex {
    /// Build the index by scanning for `\n`. `\r` (in `\r\n`) is ordinary line content.
    #[must_use]
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(u32::try_from(i + 1).unwrap_or(u32::MAX));
            }
        }
        Self {
            line_starts,
            len: u32::try_from(text.len()).unwrap_or(u32::MAX),
        }
    }

    /// A byte `offset` → LSP `Position` in `enc`. Offsets past EOF clamp to the document end.
    #[must_use]
    pub fn position(&self, text: &str, offset: u32, enc: PositionEncoding) -> Position {
        let offset = offset.min(self.len);
        // The line is the last line-start `<= offset`.
        let line = self
            .line_starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1);
        let line_start = self.line_starts[line];
        let line_text = &text[line_start as usize..];
        // Floor to a UTF-8 char boundary so a (defensively) mis-aligned offset never panics the
        // slice. Our internal offsets are always boundaries; this is no-panic insurance.
        let col_bytes = floor_char_boundary(line_text, (offset - line_start) as usize);
        let segment = &line_text[..col_bytes];
        let character: u32 = segment.chars().map(|c| enc.width(c)).sum();
        Position {
            line: u32::try_from(line).unwrap_or(u32::MAX),
            character,
        }
    }

    /// An LSP `Position` → byte offset. Out-of-range line/character clamp to the line/document end
    /// (LSP clients legitimately send end-of-line / end-of-file positions).
    #[must_use]
    pub fn offset(&self, text: &str, pos: Position, enc: PositionEncoding) -> u32 {
        let line = pos.line as usize;
        let Some(&line_start) = self.line_starts.get(line) else {
            return self.len; // line past EOF
        };
        let line_end = self.line_starts.get(line + 1).copied().unwrap_or(self.len);
        let line_text = &text[line_start as usize..line_end as usize];
        let mut units = 0u32;
        for (byte_off, c) in line_text.char_indices() {
            // Stop at the target column, or at the line terminator (a column never points past it).
            if units >= pos.character || c == '\n' {
                return line_start + u32::try_from(byte_off).unwrap_or(0);
            }
            units += enc.width(c);
        }
        line_end // last line with no trailing '\n'
    }
}

/// The largest char boundary `<= i` in `s` (a stable stand-in for the unstable
/// `str::floor_char_boundary`). `i` is first clamped to `s.len()`.
fn floor_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    /// Round-trip `offset → position → offset` must be identity at every char boundary.
    fn assert_roundtrip(text: &str, enc: PositionEncoding) {
        let li = LineIndex::new(text);
        for (off, _) in text
            .char_indices()
            .chain(std::iter::once((text.len(), ' ')))
        {
            let off = u32::try_from(off).unwrap();
            let p = li.position(text, off, enc);
            let back = li.offset(text, p, enc);
            assert_eq!(
                back, off,
                "roundtrip failed at byte {off} in {text:?} ({enc:?})"
            );
        }
    }

    #[test]
    fn ascii_positions() {
        let text = "extends Node\nfunc _ready():\n";
        let li = LineIndex::new(text);
        // `Node` starts at byte 8 → line 0, char 8.
        assert_eq!(li.position(text, 8, PositionEncoding::Utf16), pos(0, 8));
        // start of line 1 (`func`) is byte 13.
        assert_eq!(li.position(text, 13, PositionEncoding::Utf16), pos(1, 0));
        assert_eq!(li.offset(text, pos(1, 0), PositionEncoding::Utf16), 13);
    }

    #[test]
    fn multibyte_utf16_vs_utf8_korean() {
        // The cited rust-analyzer bug (issue #202 / helix #5894): `된장` is 2 chars, 3 UTF-8 bytes
        // each (6 bytes), 1 UTF-16 unit each (2 units). A byte offset right after `된장`:
        let text = "var x = \"된장\"\n"; // `"`@8, `된`@9(3B), `장`@12(3B), closing `"`@15
        let li = LineIndex::new(text);
        // byte 15 = the closing quote (right after `된장`).
        // UTF-16: `var x = "` = 9 units, + 된(1) + 장(1) = 11 → character 11.
        assert_eq!(li.position(text, 15, PositionEncoding::Utf16), pos(0, 11));
        // UTF-8: character = raw byte column = 15.
        assert_eq!(li.position(text, 15, PositionEncoding::Utf8), pos(0, 15));
        // And the reverse must NOT off-by-one (the historical bug returned the wrong byte range).
        assert_eq!(li.offset(text, pos(0, 11), PositionEncoding::Utf16), 15);
        assert_eq!(li.offset(text, pos(0, 15), PositionEncoding::Utf8), 15);
    }

    #[test]
    fn astral_surrogate_pair_emoji() {
        // `🎮` (U+1F3AE) = 4 UTF-8 bytes, 2 UTF-16 units (surrogate pair), 1 UTF-32 unit.
        let text = "x = \"🎮\"\n"; // `🎮` starts at byte 5
        let li = LineIndex::new(text);
        // byte 9 = right after the emoji (the closing quote).
        assert_eq!(li.position(text, 9, PositionEncoding::Utf16), pos(0, 7)); // 5 ascii + 2
        assert_eq!(li.position(text, 9, PositionEncoding::Utf32), pos(0, 6)); // 5 ascii + 1
        assert_eq!(li.position(text, 9, PositionEncoding::Utf8), pos(0, 9));
        assert_eq!(li.offset(text, pos(0, 7), PositionEncoding::Utf16), 9);
        assert_eq!(li.offset(text, pos(0, 6), PositionEncoding::Utf32), 9);
    }

    #[test]
    fn crlf_and_clamping() {
        let text = "a\r\nbb\r\n";
        let li = LineIndex::new(text);
        // line 0 content is `a\r` (the `\r` is ordinary content); `\n` at byte 2.
        assert_eq!(li.position(text, 0, PositionEncoding::Utf16), pos(0, 0));
        assert_eq!(li.position(text, 3, PositionEncoding::Utf16), pos(1, 0)); // start of `bb`
        // a character past end-of-line clamps to the line terminator (before `\n`).
        assert_eq!(li.offset(text, pos(0, 99), PositionEncoding::Utf16), 2); // the `\n` byte
        // a line past EOF clamps to document end.
        assert_eq!(li.offset(text, pos(99, 0), PositionEncoding::Utf16), li.len);
    }

    #[test]
    fn mid_char_offset_floors_instead_of_panicking() {
        // A (defensively) mis-aligned offset inside a 3-byte char must NOT panic the str slice; it
        // floors to the char start. `된` occupies bytes 5,6,7.
        let text = "x = \"된\"\n";
        let li = LineIndex::new(text);
        assert_eq!(li.position(text, 6, PositionEncoding::Utf16), pos(0, 5)); // mid-`된` → floored
        assert_eq!(li.position(text, 7, PositionEncoding::Utf16), pos(0, 5)); // 3rd byte → floored
    }

    #[test]
    fn empty_document() {
        let li = LineIndex::new("");
        assert_eq!(li.position("", 0, PositionEncoding::Utf16), pos(0, 0));
        assert_eq!(li.offset("", pos(0, 0), PositionEncoding::Utf16), 0);
        assert_eq!(li.offset("", pos(5, 5), PositionEncoding::Utf16), 0);
    }

    #[test]
    fn roundtrip_corpus() {
        for enc in [
            PositionEncoding::Utf8,
            PositionEncoding::Utf16,
            PositionEncoding::Utf32,
        ] {
            assert_roundtrip("extends Node\n\tvar x := 1\n", enc);
            assert_roundtrip("var s = \"된장 🎮 café\"\nfunc f():\n\tpass\n", enc);
            assert_roundtrip("a\r\nb\r\n\r\n", enc);
            assert_roundtrip("", enc);
            assert_roundtrip("no_trailing_newline", enc);
        }
    }
}
