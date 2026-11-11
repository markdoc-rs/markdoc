use crate::{DeltaBlock, FoldPair, GenUnicode};
use std::collections::HashSet;

impl GenUnicode {
    /// TO_UPPERCASE is Canonicalize for patterns without the `u` or `v` flag
    /// (<https://tc39.es/ecma262/#sec-runtime-semantics-canonicalize-ch>): a character maps to
    /// the result of toUppercase only if that is a single character, and a non-ASCII
    /// character never maps to an ASCII one.
    pub(crate) fn generate_uppercase(&mut self) {
        // toUppercase uses the full mappings: a character whose unconditional
        // SpecialCasing.txt uppercase is longer than one character (U+1F80 GREEK SMALL
        // LETTER ALPHA WITH PSILI AND YPOGEGRAMMENI is U+1F08 U+0399) canonicalizes to itself,
        // although UnicodeData.txt gives it a simple uppercase (U+1F88).
        let multi_char_uppercase: HashSet<u32> = self
            .special_case_mappings
            .iter()
            .filter(|m| m.conditions.is_empty() && m.uppercase.len() > 1)
            .map(|m| m.codepoint.value())
            .collect();

        let mut fold_pairs = Vec::new();

        for data in &self.data {
            let Some(uppercase) = data.simple_uppercase_mapping else {
                continue;
            };
            let orig = data.codepoint.value();
            let folded = uppercase.value();
            if multi_char_uppercase.contains(&orig) {
                continue;
            }
            // U+017F LATIN SMALL LETTER LONG S and U+0131 LATIN SMALL LETTER DOTLESS I
            // uppercase to ASCII letters, which Canonicalize does not allow.
            if orig >= 0x80 && folded < 0x80 {
                continue;
            }
            fold_pairs.push(FoldPair { orig, folded });
        }

        let mut delta_blocks: Vec<DeltaBlock> = Vec::new();
        for fp in &fold_pairs {
            match delta_blocks.last_mut() {
                Some(ref mut db) if db.can_append(*fp) => db.append(*fp),
                _ => delta_blocks.push(DeltaBlock::create(*fp)),
            }
        }

        let mut lines = Vec::new();
        for db in &delta_blocks {
            lines.push(format!(
                "FoldRange::from({start:#04X}, {length}, {delta}, {modulo}),",
                start = db.first().orig,
                length = db.length(),
                delta = db.delta(),
                modulo = db.stride().unwrap_or(1),
            ));
        }

        self.scope.raw(format!(
            "pub(crate) const TO_UPPERCASE: [FoldRange; {}] = [\n    {}\n];",
            delta_blocks.len(),
            lines.join("\n    ")
        ));
    }
}
