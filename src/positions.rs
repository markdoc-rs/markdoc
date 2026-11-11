//! Positions in a source text.
//!
//! [`LineIndex`] turns a byte offset into a 1-based line and a 0-based UTF-16 column, as the
//! Language Server Protocol counts them. The parser places nodes with it. It used to live in
//! the old pipeline's `sourcemap` module, whose path still re-exports it (hidden) until the
//! parser's import moves here.

/// Byte offset to 1-based line and 0-based UTF-16 character, which is what
/// LSP positions are counted in.
///
/// [`line_col`](Self::line_col) takes O(log lines) for the line and O(1) for
/// the column: the UTF-16 length of the text before each line, and of every
/// prefix of it that ends at a multiple of `UNITS_STRIDE` bytes, is kept, so
/// a column is the length before the offset (a kept length, plus fewer than
/// `UNITS_STRIDE` bytes counted unless they are all ASCII) less the length
/// before its line. (Counting from the line's start each time made a long line
/// quadratic: one line of 40,000 tags took 18 s to locate.)
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Byte offset of the first byte of each line.
    starts: Vec<usize>,
    /// The UTF-16 units before each line.
    line_units: Vec<usize>,
    len: usize,
    /// `units[k]`: the UTF-16 units of the text's first `k * UNITS_STRIDE`
    /// bytes, a character cut at the end counted whole; the last is the
    /// text's.
    units: Vec<usize>,
    /// `ascii[k]`: whether the `k`th `UNITS_STRIDE` bytes are all ASCII (as
    /// many units as bytes). Their count cannot tell: a block that begins
    /// inside a character and ends in the first byte of one beyond U+FFFF has
    /// as many units as bytes.
    ascii: Vec<bool>,
}

/// How many bytes apart [`LineIndex`] keeps a UTF-16 length.
const UNITS_STRIDE: usize = 64;

/// The UTF-16 units a byte stands for, counted at its character's first byte:
/// a character of one to three UTF-8 bytes is one unit, one of four bytes
/// (beyond U+FFFF) a surrogate pair.
fn utf16_units(byte: u8) -> usize {
    usize::from(byte & 0xC0 != 0x80) + usize::from(byte >= 0xF0)
}

impl LineIndex {
    pub fn new(src: &str) -> Self {
        let mut starts = vec![0usize];
        let mut line_units = vec![0usize];
        let mut units = Vec::with_capacity(src.len() / UNITS_STRIDE + 2);
        let mut ascii = Vec::with_capacity(src.len() / UNITS_STRIDE + 1);
        let mut total = 0;
        for (i, &b) in src.as_bytes().iter().enumerate() {
            if i % UNITS_STRIDE == 0 {
                units.push(total);
                ascii.push(true);
            }
            if !b.is_ascii()
                && let Some(block) = ascii.last_mut()
            {
                *block = false;
            }
            total += utf16_units(b);
            if b == b'\n' {
                starts.push(i + 1);
                line_units.push(total);
            }
        }
        units.push(total);
        Self {
            starts,
            line_units,
            len: src.len(),
            units,
            ascii,
        }
    }

    /// The UTF-16 units of `bytes[..at]`, from the nearest kept length: a block
    /// of `UNITS_STRIDE` bytes that are all ASCII needs no counting.
    fn units_before(&self, bytes: &[u8], at: usize) -> usize {
        let block = at / UNITS_STRIDE;
        let from = block * UNITS_STRIDE;
        let counted = if self.ascii.get(block).copied().unwrap_or(true) {
            at - from
        } else {
            bytes[from..at].iter().map(|&b| utf16_units(b)).sum()
        };
        self.units[block] + counted
    }

    /// 1-based line, 0-based UTF-16 character. `src` is the text the index was
    /// made from; an offset inside a character has the character 0.
    pub fn line_col(&self, src: &str, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.len);
        let line = match self.starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let start = self.starts[line];
        // UTF-16 code units, because that is what an LSP client counts.
        let col = if src.len() != self.len {
            // Not the text the index was made from: count as ever.
            src.get(start..offset)
                .map(|s| s.chars().map(char::len_utf16).sum())
                .unwrap_or(0)
        } else if src.is_char_boundary(offset) {
            self.units_before(src.as_bytes(), offset) - self.line_units[line]
        } else {
            0
        };
        (line + 1, col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_counts_lines_from_one() {
        let src = "alpha\nbeta\ngamma";
        let idx = LineIndex::new(src);
        assert_eq!(idx.line_col(src, 0), (1, 0));
        assert_eq!(idx.line_col(src, 6), (2, 0));
        assert_eq!(idx.line_col(src, 8), (2, 2));
        assert_eq!(idx.line_col(src, 11), (3, 0));
    }

    #[test]
    fn line_col_counts_utf16_units_not_bytes() {
        // 'é' is 2 bytes in UTF-8 but 1 UTF-16 unit; '𝄞' is 4 bytes, 2 units.
        let src = "é𝄞x";
        let idx = LineIndex::new(src);
        assert_eq!(idx.line_col(src, 0), (1, 0));
        assert_eq!(idx.line_col(src, 2), (1, 1)); // after é
        assert_eq!(idx.line_col(src, 6), (1, 3)); // after 𝄞
    }

    /// What `line_col` counted before it kept UTF-16 lengths: the line's start
    /// to the offset, every time.
    fn line_col_by_counting(src: &str, offset: usize) -> (usize, usize) {
        let offset = offset.min(src.len());
        let start = src.as_bytes()[..offset]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1);
        let line = src.as_bytes()[..start]
            .iter()
            .filter(|&&b| b == b'\n')
            .count();
        let col = src
            .get(start..offset)
            .map(|s| s.chars().map(char::len_utf16).sum())
            .unwrap_or(0);
        (line + 1, col)
    }

    #[test]
    fn line_col_is_what_counting_from_the_line_start_gives() {
        // Every offset (character boundaries, inside characters and past the end) of texts
        // mixing one- to four-byte characters, long lines and line ends, so that lines and
        // characters straddle the kept lengths (every 64 bytes).
        let mut texts = vec![String::new(), "\n".into(), "\n\n".into(), "a".into()];
        let pieces = ["a", "é", "€", "𝄞", "\n", "\r\n", " ", "{% x /%}"];
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        for len in [10, 63, 64, 65, 200, 1000] {
            let mut s = String::new();
            for _ in 0..len {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                s.push_str(pieces[(state % pieces.len() as u64) as usize]);
            }
            texts.push(s);
        }
        texts.push("𝄞".repeat(100));
        texts.push(format!("{}\n{}", "é".repeat(70), "x".repeat(130)));
        // A block (bytes 64 to 127) that begins inside `é` and ends in the first byte of
        // `𝄞` has as many units as bytes, yet is not ASCII.
        texts.push(format!(
            "{}é{}𝄞{}",
            "x".repeat(63),
            "x".repeat(62),
            "x".repeat(70)
        ));
        for src in &texts {
            let idx = LineIndex::new(src);
            for offset in 0..=src.len() + 2 {
                assert_eq!(
                    idx.line_col(src, offset),
                    line_col_by_counting(src, offset),
                    "offset {offset} of {src:?}"
                );
            }
        }
    }

    #[test]
    fn line_col_of_another_text_counts_it() {
        // Given a text other than its own, the index counts in that text from its own line
        // starts, as it always did.
        let idx = LineIndex::new("ab\ncd");
        assert_eq!(idx.line_col("éé\nxyz", 2), (1, 1));
        assert_eq!(idx.line_col("é", 1), (1, 0));
    }

    #[test]
    fn line_col_on_one_long_line_is_not_quadratic() {
        // 40,000 tags on one line (the parser locates both ends of every node): a few
        // milliseconds now; counting from the line start took seconds.
        let src = "a {% x /%}".repeat(40_000) + "é";
        let idx = LineIndex::new(&src);
        let started = std::time::Instant::now();
        let mut sum = 0;
        for offset in (0..src.len()).step_by(5) {
            sum += idx.line_col(&src, offset).1;
        }
        assert!(sum > 0);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }
}
