//! markdown-it's presets (presets/*.ts) and the `MarkdownItPreset` shape.

pub mod commonmark;
pub mod default;
pub mod zero;

use super::Options;

/// The rule names a preset enables in one component (`{ rules?, rules2? }`); `rules2` is
/// only read for `inline`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ComponentRules {
    pub rules: Option<Vec<String>>,
    pub rules2: Option<Vec<String>>,
}

/// `components: { core?, block?, inline? }`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Components {
    pub core: Option<ComponentRules>,
    pub block: Option<ComponentRules>,
    pub inline: Option<ComponentRules>,
}

/// `MarkdownItPreset`: options and enabled rules for each parser component.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Preset {
    pub options: Option<Options>,
    pub components: Option<Components>,
}

/// Is `name` one `config` (a plain object) only inherits, from `Object.prototype`
/// (`toString`, `constructor`, `__proto__`, ...)? `config[name]` is then a function (or for
/// `__proto__` `Object.prototype` itself): truthy, with neither `options` nor `components`.
pub fn is_inherited(name: &str) -> bool {
    super::mdurl::OBJECT_PROTOTYPE_KEYS.contains(&name)
}

/// The `config` table of markdownit.ts.
pub fn by_name(name: &str) -> Option<Preset> {
    match name {
        "default" => Some(default::preset()),
        "zero" => Some(zero::preset()),
        "commonmark" => Some(commonmark::preset()),
        _ => None,
    }
}

pub(crate) fn rules(names: &[&str]) -> Option<Vec<String>> {
    Some(names.iter().map(|&s| s.to_owned()).collect())
}
