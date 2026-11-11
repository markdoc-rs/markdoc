//! Port of presets/default.ts: markdown-it default options, every rule enabled.

use super::{ComponentRules, Components, Preset};
use crate::markdown_it::{Options, Quotes};

pub fn options() -> Options {
    Options {
        // Enable HTML tags in source
        html: false,

        // Use '/' to close single tags (<br />)
        xhtml_out: false,

        // Convert '\n' in paragraphs into <br>
        breaks: false,

        // CSS language prefix for fenced blocks
        lang_prefix: "language-".into(),

        // autoconvert URL-like texts to links
        linkify: false,

        // Enable some language-neutral replacements + quotes beautification
        typographer: false,

        // Double + single quotes replacement pairs, when typographer enabled,
        // and smartquotes on. Could be either a String or an Array.
        //
        // For example, you can use '«»„“' for Russian, '„“‚‘' for German,
        // and ['«\xA0', '\xA0»', '‹\xA0', '\xA0›'] for French (including nbsp).
        quotes: Quotes::Str("\u{201c}\u{201d}\u{2018}\u{2019}".into()), /* “”‘’ */

        // Highlighter function (`highlight: null`): renderer-only, not ported.

        // Internal protection, recursion limit
        max_nesting: 100.0,

        // Not in markdown-it's presets (undefined): Markdoc's patch option.
        allow_indentation: false,
        extra: serde_json::Map::new(),
    }
}

pub fn preset() -> Preset {
    Preset {
        options: Some(options()),

        components: Some(Components {
            core: Some(ComponentRules::default()),
            block: Some(ComponentRules::default()),
            inline: Some(ComponentRules::default()),
        }),
    }
}
