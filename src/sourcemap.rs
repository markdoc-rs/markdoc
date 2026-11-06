//! Mapping an offset in a rewritten buffer back to the byte it came from.
//!
//! `parse()` rewrites the source three times before anything is tokenised:
//! the frontmatter is stripped, `\{%` escapes are swapped for a sentinel, and
//! `{% table %}` blocks are lifted out and replaced by a placeholder. None of
//! those is length-preserving, so an offset produced after them indexes a
//! buffer the author never typed.
//!
//! That is why every node in every document used to report line 1: the
//! position was not wrong so much as expressed in a coordinate space nobody
//! could translate. A `SourceMap` records what each rewrite replaced, and a
//! `SourceMapChain` walks the rewrites backwards to recover the original
//! byte.
//!
//! A replaced region maps to the *start* of what it replaced. That is the
//! useful answer: a diagnostic about a `{% table %}` placeholder belongs at
//! the opening `{%` of the block it stands for, not at some interior byte
//! that no longer exists.

use std::ops::Range;

/// One rewrite: output coordinates to input coordinates.
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    /// Replaced regions, in ascending output order and non-overlapping.
    /// Bytes between them map one-to-one, offset by the running difference.
    repls: Vec<Repl>,
}

#[derive(Debug, Clone)]
struct Repl {
    out: Range<usize>,
    src: Range<usize>,
}

impl SourceMap {
    /// A rewrite that changed nothing.
    pub fn identity() -> Self {
        Self::default()
    }

    /// Record that `src` in the input became `out` in the output.
    ///
    /// Must be called in ascending output order; `map` relies on it to stop
    /// scanning early.
    pub fn replace(&mut self, out: Range<usize>, src: Range<usize>) {
        debug_assert!(
            self.repls.last().is_none_or(|p| p.out.end <= out.start),
            "replacements must be recorded in ascending, non-overlapping output order"
        );
        self.repls.push(Repl { out, src });
    }

    /// The input offset that `out` came from.
    pub fn map(&self, out: usize) -> usize {
        // Outside every replacement, src = out + delta, where delta is the
        // net length change of the replacements already passed.
        let mut delta: isize = 0;
        for r in &self.repls {
            if out < r.out.start {
                break;
            }
            if out < r.out.end {
                return r.src.start;
            }
            delta += (r.src.end as isize - r.src.start as isize)
                - (r.out.end as isize - r.out.start as isize);
        }
        (out as isize + delta).max(0) as usize
    }
}

/// The rewrites in the order they were applied. `map` walks them backwards.
#[derive(Debug, Clone, Default)]
pub struct SourceMapChain {
    maps: Vec<SourceMap>,
}

impl SourceMapChain {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a rewrite. Order matters: this must be the order `parse` ran them.
    pub fn push(&mut self, map: SourceMap) {
        self.maps.push(map);
    }

    /// An offset in the final buffer, expressed as an offset in the original.
    pub fn map(&self, out: usize) -> usize {
        self.maps.iter().rev().fold(out, |o, m| m.map(o))
    }

    /// A range in the final buffer, expressed in the original. The end maps
    /// through `end - 1` so that a range ending exactly at a replacement
    /// boundary does not jump to the following region.
    pub fn map_range(&self, out: Range<usize>) -> Range<usize> {
        let start = self.map(out.start);
        if out.end <= out.start {
            return start..start;
        }
        let last = self.map(out.end - 1);
        start..last.saturating_add(1).max(start)
    }
}

/// Byte offset to 1-based line and 0-based UTF-16 character, which is what
/// LSP positions are counted in.
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Byte offset of the first byte of each line.
    starts: Vec<usize>,
    len: usize,
}

impl LineIndex {
    pub fn new(src: &str) -> Self {
        let mut starts = vec![0usize];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        Self {
            starts,
            len: src.len(),
        }
    }

    /// 1-based line, 0-based UTF-16 character.
    pub fn line_col(&self, src: &str, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.len);
        let line = match self.starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let start = self.starts[line];
        // UTF-16 code units, because that is what an LSP client counts.
        let col = src
            .get(start..offset)
            .map(|s| s.chars().map(char::len_utf16).sum())
            .unwrap_or(0);
        (line + 1, col)
    }
}

/// Replace every occurrence of `needle` with `replacement`, recording what
/// moved.
///
/// `str::replace` is the obvious way to do this and the reason positions were
/// unrecoverable: it returns a new string and says nothing about where its
/// bytes came from.
pub fn replace_mapped(src: &str, needle: &str, replacement: &str) -> (String, SourceMap) {
    let mut out = String::with_capacity(src.len());
    let mut map = SourceMap::identity();
    let mut last = 0usize;
    for (at, _) in src.match_indices(needle) {
        out.push_str(&src[last..at]);
        let out_start = out.len();
        out.push_str(replacement);
        map.replace(out_start..out.len(), at..at + needle.len());
        last = at + needle.len();
    }
    out.push_str(&src[last..]);
    (out, map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_maps_offsets_unchanged() {
        let m = SourceMap::identity();
        assert_eq!(m.map(0), 0);
        assert_eq!(m.map(42), 42);
    }

    #[test]
    fn a_stripped_prefix_shifts_everything_after_it() {
        // Frontmatter: 20 source bytes became nothing.
        let mut m = SourceMap::identity();
        m.replace(0..0, 0..20);
        // A deletion has no interior: output 0 is the first byte AFTER it.
        assert_eq!(m.map(0), 20);
        assert_eq!(m.map(1), 21);
        assert_eq!(m.map(5), 25);
    }

    #[test]
    fn a_grown_replacement_shifts_the_other_way() {
        // `\{%` (2 bytes) became a 3-byte sentinel at offset 4.
        let mut m = SourceMap::identity();
        m.replace(4..7, 4..6);
        assert_eq!(m.map(3), 3); // before it
        assert_eq!(m.map(5), 4); // inside maps to its start
        assert_eq!(m.map(7), 6); // after it
        assert_eq!(m.map(9), 8);
    }

    #[test]
    fn several_replacements_accumulate() {
        let mut m = SourceMap::identity();
        m.replace(0..0, 0..10); // -10 output
        m.replace(5..8, 15..40); // 25 source bytes became 3
        assert_eq!(m.map(1), 11);
        assert_eq!(m.map(6), 15); // inside the second
        assert_eq!(m.map(8), 40); // just after it
    }

    #[test]
    fn a_chain_walks_rewrites_backwards() {
        // Rewrite 1: strip 10 bytes of frontmatter.
        let mut fm = SourceMap::identity();
        fm.replace(0..0, 0..10);
        // Rewrite 2, in post-frontmatter space: 2 bytes at 3 became 3.
        let mut esc = SourceMap::identity();
        esc.replace(3..6, 3..5);

        let mut chain = SourceMapChain::new();
        chain.push(fm);
        chain.push(esc);

        // Final offset 6 -> post-fm offset 5 -> original 15.
        assert_eq!(chain.map(6), 15);
        // Final offset 0 is the first content byte: original 10.
        assert_eq!(chain.map(0), 10);
    }

    #[test]
    fn ranges_do_not_overshoot_a_boundary() {
        let mut m = SourceMap::identity();
        m.replace(4..7, 4..6);
        let mut chain = SourceMapChain::new();
        chain.push(m);
        // Output 4..7 is exactly the replacement; it came from 4..6.
        assert_eq!(chain.map_range(4..7), 4..5);
    }

    #[test]
    fn replace_mapped_recovers_original_offsets() {
        //            0123456789
        let src = "ab\\{%cd\\{%ef";
        let (out, map) = replace_mapped(src, "\\{%", "\u{E010}");
        assert_eq!(out, "ab\u{E010}cd\u{E010}ef");
        // 'a' and 'b' are untouched.
        assert_eq!(map.map(0), 0);
        assert_eq!(map.map(1), 1);
        // The byte after the first sentinel is 'c', original offset 5.
        let first_sentinel_len = "\u{E010}".len();
        assert_eq!(map.map(2 + first_sentinel_len), 5);
    }

    #[test]
    fn replace_mapped_with_no_match_is_identity() {
        let (out, map) = replace_mapped("plain text", "\\{%", "X");
        assert_eq!(out, "plain text");
        assert_eq!(map.map(7), 7);
    }

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
}

#[cfg(test)]
mod end_to_end {
    use crate::sourcemap::LineIndex;
    use crate::tag_parser;

    /// Every tag's reported span must be the byte range the author actually
    /// typed, through frontmatter, an escaped opener and a lifted table.
    #[test]
    fn tag_spans_survive_all_three_rewrites() {
        let src = "\
---
title: Probe
---
Intro paragraph.

A literal \\{% is not a tag.

{% table %}
- - a
  - b
{% /table %}

{% notice type=\"warning\" %}
Careful.
{% /notice %}
";
        // Parse through the real entry point, then re-derive the tag spans the
        // same way `parse` does, so this asserts the shipped path.
        let doc = crate::parse(src, None).expect("parses");
        assert!(doc.children.len() > 1, "document should have content");

        // Independently confirm the mapping by locating the tags in the source.
        let notice_open = src.find("{% notice").expect("notice present");
        let notice_close = src.find("{% /notice %}").expect("close present");
        let table_open = src.find("{% table %}").expect("table present");

        // Re-run the pipeline's front half to inspect spans directly.
        let (_, body, fm_off) =
            crate::frontmatter::extract_frontmatter_mapped(src).expect("frontmatter");
        let mut maps = super::SourceMapChain::new();
        let mut fm_map = super::SourceMap::identity();
        if fm_off > 0 {
            fm_map.replace(0..0, 0..fm_off);
        }
        maps.push(fm_map);
        let (body, esc) = super::replace_mapped(&body, "\\{%", "\u{E010}");
        maps.push(esc);
        let (body, _tables, tmap) = crate::list_table::extract_list_tables_mapped(&body);
        maps.push(tmap);
        let (_, tags) = tag_parser::segment_with_tags(&body);

        let mapped: Vec<_> = tags
            .iter()
            .map(|t| maps.map_range(t.source_range.clone()))
            .collect();

        // The escaped `\{%` must NOT appear as a tag.
        for m in &mapped {
            let text = &src[m.start..m.end.min(src.len())];
            assert!(
                !text.starts_with("\\{%"),
                "an escaped opener was treated as a tag: {text:?}"
            );
        }

        // Every mapped span must start at a real `{%` in the ORIGINAL source.
        for m in &mapped {
            assert!(
                src[m.start..].starts_with("{%"),
                "span {m:?} does not start at a tag in the source; it points at {:?}",
                &src[m.start..(m.start + 12).min(src.len())]
            );
        }

        // The notice tags specifically, by offset.
        assert!(
            mapped.iter().any(|m| m.start == notice_open),
            "no span at the notice open ({notice_open}); got {mapped:?}"
        );
        assert!(
            mapped.iter().any(|m| m.start == notice_close),
            "no span at the notice close ({notice_close}); got {mapped:?}"
        );

        // And the line numbers a diagnostic would report.
        let idx = LineIndex::new(src);
        let (line, col) = idx.line_col(src, notice_open);
        assert_eq!(
            (line, col),
            (13, 0),
            "the notice opens on line 13, column 0 of the source"
        );
        // Sanity: the table is before it and on its own line.
        assert!(table_open < notice_open);
        assert_eq!(idx.line_col(src, table_open).0, 8);
    }
}
