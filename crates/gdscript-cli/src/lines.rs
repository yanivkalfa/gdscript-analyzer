//! Byte offset → **1-based** `(line, column)` — THE correctness-critical conversion (Playbook §3).
//!
//! The core emits 0-based UTF-8 byte offsets ([`gdscript_base::TextRange`]); every CLI consumer
//! (human snippets, GitHub annotations, SARIF, rdjson) needs 1-based line:col, and the column UNIT
//! differs: rdjson counts UTF-8 **bytes**, the others count **characters**. One converter computes
//! both, once per file, so no emitter re-derives positions.

/// A per-file newline-offset table.
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Byte offset of each line's first byte. `line_starts[0] == 0`; always non-empty.
    line_starts: Vec<u32>,
    /// Total length in bytes (for clamping).
    len: u32,
}

/// A 1-based source position with both column units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column counted in Unicode scalar values (human / GitHub / SARIF).
    pub char_col: u32,
    /// 1-based column counted in UTF-8 bytes (rdjson).
    pub byte_col: u32,
}

impl LineIndex {
    /// Build the index by scanning for `\n` (a `\r` in `\r\n` is ordinary line content).
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

    /// A byte `offset` → a 1-based [`LineCol`]. The offset is clamped to EOF and floored to a UTF-8
    /// char boundary (so a defensively-misaligned offset never panics the slice).
    #[must_use]
    pub fn line_col(&self, text: &str, offset: u32) -> LineCol {
        let offset = offset.min(self.len);
        let line = self
            .line_starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1);
        let line_start = self.line_starts[line];
        let line_text = &text[line_start as usize..];
        // 0-based byte column within the line, clamped + floored to a char boundary.
        let mut col = (offset - line_start) as usize;
        col = col.min(line_text.len());
        while col > 0 && !line_text.is_char_boundary(col) {
            col -= 1;
        }
        let char_col = line_text[..col].chars().count();
        LineCol {
            line: u32::try_from(line + 1).unwrap_or(u32::MAX),
            char_col: u32::try_from(char_col + 1).unwrap_or(u32::MAX),
            byte_col: u32::try_from(col + 1).unwrap_or(u32::MAX),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lc(text: &str, offset: u32) -> (u32, u32, u32) {
        let p = LineIndex::new(text).line_col(text, offset);
        (p.line, p.char_col, p.byte_col)
    }

    #[test]
    fn ascii_is_1_based() {
        let text = "extends Node\nfunc f():\n";
        assert_eq!(lc(text, 0), (1, 1, 1)); // start of `extends`
        assert_eq!(lc(text, 8), (1, 9, 9)); // `Node`
        assert_eq!(lc(text, 13), (2, 1, 1)); // start of `func` (line 2)
    }

    #[test]
    fn multibyte_char_vs_byte_columns() {
        // `된장` = 2 chars, 3 UTF-8 bytes each. The `"` after them: byte 15.
        let text = "var x = \"된장\"\n"; // `"`@8, 된@9(3B), 장@12(3B), `"`@15
        assert_eq!(lc(text, 8), (1, 9, 9)); // opening quote (8 ASCII before it)
        // before byte 15: `var x = ` (8) + `"` (1) + `된장` (2 chars) = 11 chars; byte col = 15+1.
        assert_eq!(lc(text, 15), (1, 12, 16));
    }

    #[test]
    fn astral_emoji() {
        // `🎮` = 1 char, 4 UTF-8 bytes. Byte 9 = the `"` after it.
        let text = "x = \"🎮\"\n"; // 🎮@5(4B), `"`@9
        assert_eq!(lc(text, 9), (1, 7, 10)); // char col 7 (5 ascii + 1), byte col 10
    }

    #[test]
    fn crlf_bom_and_clamping() {
        let text = "\u{feff}a\r\nbb\r\n"; // BOM(3B) + `a\r\n` + `bb\r\n`
        // BOM is 3 bytes on line 1; `a` starts at byte 3.
        assert_eq!(lc(text, 3), (1, 2, 4)); // `a` — char col 2 (after the BOM char), byte col 4
        // line 2 (`bb`) starts after `\u{feff}a\r\n` = bytes 3+1+2 = 6.
        assert_eq!(lc(text, 6).0, 2);
        // an offset past EOF clamps to the document end without panicking.
        let p = LineIndex::new(text).line_col(text, 9999);
        assert!(p.line >= 1);
    }

    #[test]
    fn mid_char_offset_floors_not_panics() {
        let text = "x=\"된\"\n"; // 된 at bytes 3,4,5
        // byte 4 is mid-`된`; must floor to the char start (byte 3), not panic.
        assert_eq!(lc(text, 4), lc(text, 3));
    }

    #[test]
    fn empty_file() {
        assert_eq!(lc("", 0), (1, 1, 1));
    }
}
