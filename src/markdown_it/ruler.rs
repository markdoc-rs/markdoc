//! Port of ruler.ts: an ordered list of named rules that can be enabled, disabled, replaced
//! and inserted, with named alternative chains (`alt`) and a cache of the active rules.
//!
//! The cache (`__cache__`) is compiled lazily by [`Ruler::get_rules`], which only needs
//! `&self` (rules call it through `state.md`), so it lives in a `OnceLock` that every
//! mutating method resets, as ruler.ts sets `__cache__ = null`.

use std::collections::HashMap;
use std::sync::OnceLock;

/// The `Error` ruler.ts throws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulerError(pub String);

impl std::fmt::Display for RulerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RulerError {}

/// One entry of `__rules__`.
#[derive(Debug, Clone)]
pub struct Rule<F> {
    pub name: String,
    pub enabled: bool,
    /// JS `fn`.
    pub func: F,
    pub alt: Vec<String>,
}

/// `Ruler<Args, Result>`, generic over the rule function type (`CoreRule`, `BlockRule`,
/// `InlineRule`, `InlineRule2`).
#[derive(Debug, Clone)]
pub struct Ruler<F> {
    /// `__rules__`
    rules: Vec<Rule<F>>,
    /// `__cache__`: chain name (`""` is the default chain) to the enabled rules in order.
    cache: OnceLock<HashMap<String, Vec<F>>>,
}

impl<F: Copy> Default for Ruler<F> {
    fn default() -> Self {
        Self::new()
    }
}

fn owned(alt: &[&str]) -> Vec<String> {
    alt.iter().map(|&s| s.to_owned()).collect()
}

impl<F: Copy> Ruler<F> {
    pub fn new() -> Self {
        Ruler {
            rules: Vec::new(),
            cache: OnceLock::new(),
        }
    }

    /// `__rules__`, read-only (changing it directly would bypass the cache reset).
    pub fn rules(&self) -> &[Rule<F>] {
        &self.rules
    }

    /// `__find__(name)`; `None` where JS returns `-1`.
    fn find(&self, name: &str) -> Option<usize> {
        self.rules.iter().position(|rule| rule.name == name)
    }

    /// `__compile__()`.
    fn compile(&self) -> HashMap<String, Vec<F>> {
        // collect unique names (a Set in JS: the order does not matter for lookups)
        let mut chains: Vec<&str> = Vec::new();
        for rule in &self.rules {
            if !rule.enabled {
                continue;
            }
            for alt_name in &rule.alt {
                if !alt_name.is_empty() && !chains.contains(&alt_name.as_str()) {
                    chains.push(alt_name);
                }
            }
        }

        let mut cache = HashMap::new();

        // Collect default chain
        cache.insert(
            String::new(),
            self.rules
                .iter()
                .filter(|r| r.enabled)
                .map(|r| r.func)
                .collect(),
        );

        // Collect alt chains
        for chain in chains {
            let fns = self
                .rules
                .iter()
                .filter(|r| r.enabled && r.alt.iter().any(|a| a == chain))
                .map(|r| r.func)
                .collect();
            cache.insert(chain.to_owned(), fns);
        }
        cache
    }

    fn reset_cache(&mut self) {
        self.cache = OnceLock::new();
    }

    /// `at(name, fn, { alt })`: replace the rule `name`. Errors (JS throws) if there is none.
    pub fn at(&mut self, name: &str, func: F, alt: &[&str]) -> Result<(), RulerError> {
        let Some(index) = self.find(name) else {
            return Err(RulerError(format!("Parser rule not found: {name}")));
        };

        self.rules[index].func = func;
        self.rules[index].alt = owned(alt);
        self.reset_cache();
        Ok(())
    }

    /// `before(beforeName, ruleName, fn, { alt })`: insert a rule before `before_name`.
    pub fn before(
        &mut self,
        before_name: &str,
        rule_name: &str,
        func: F,
        alt: &[&str],
    ) -> Result<(), RulerError> {
        let Some(index) = self.find(before_name) else {
            return Err(RulerError(format!("Parser rule not found: {before_name}")));
        };

        self.rules.insert(
            index,
            Rule {
                name: rule_name.to_owned(),
                enabled: true,
                func,
                alt: owned(alt),
            },
        );

        self.reset_cache();
        Ok(())
    }

    /// `after(afterName, ruleName, fn, { alt })`: insert a rule after `after_name`.
    pub fn after(
        &mut self,
        after_name: &str,
        rule_name: &str,
        func: F,
        alt: &[&str],
    ) -> Result<(), RulerError> {
        let Some(index) = self.find(after_name) else {
            return Err(RulerError(format!("Parser rule not found: {after_name}")));
        };

        self.rules.insert(
            index + 1,
            Rule {
                name: rule_name.to_owned(),
                enabled: true,
                func,
                alt: owned(alt),
            },
        );

        self.reset_cache();
        Ok(())
    }

    /// `push(ruleName, fn, { alt })`: append a rule.
    pub fn push(&mut self, rule_name: &str, func: F, alt: &[&str]) {
        self.rules.push(Rule {
            name: rule_name.to_owned(),
            enabled: true,
            func,
            alt: owned(alt),
        });

        self.reset_cache();
    }

    /// `enable(list, ignoreInvalid)`: enable the named rules and return the names found.
    ///
    /// As in JS, an unknown name (without `ignore_invalid`) fails after the names before it
    /// were enabled, and without resetting the cache.
    pub fn enable<S: AsRef<str>>(
        &mut self,
        list: &[S],
        ignore_invalid: bool,
    ) -> Result<Vec<String>, RulerError> {
        let mut result = Vec::new();

        // Search by name and enable
        for name in list {
            let name = name.as_ref();
            let Some(idx) = self.find(name) else {
                if ignore_invalid {
                    continue;
                }
                return Err(RulerError(format!(
                    "Rules manager: invalid rule name {name}"
                )));
            };
            self.rules[idx].enabled = true;
            result.push(name.to_owned());
        }

        self.reset_cache();
        Ok(result)
    }

    /// `enableOnly(list, ignoreInvalid)`: disable every rule, then enable the named ones.
    pub fn enable_only<S: AsRef<str>>(
        &mut self,
        list: &[S],
        ignore_invalid: bool,
    ) -> Result<(), RulerError> {
        for rule in &mut self.rules {
            rule.enabled = false;
        }

        self.enable(list, ignore_invalid).map(|_| ())
    }

    /// `disable(list, ignoreInvalid)`: disable the named rules and return the names found.
    /// Fails like [`Ruler::enable`].
    pub fn disable<S: AsRef<str>>(
        &mut self,
        list: &[S],
        ignore_invalid: bool,
    ) -> Result<Vec<String>, RulerError> {
        let mut result = Vec::new();

        // Search by name and disable
        for name in list {
            let name = name.as_ref();
            let Some(idx) = self.find(name) else {
                if ignore_invalid {
                    continue;
                }
                return Err(RulerError(format!(
                    "Rules manager: invalid rule name {name}"
                )));
            };
            self.rules[idx].enabled = false;
            result.push(name.to_owned());
        }

        self.reset_cache();
        Ok(result)
    }

    /// `getRules(chainName)`: the enabled rules of a chain (`""` is the default chain), in
    /// order; empty for an unknown chain.
    pub fn get_rules(&self, chain_name: &str) -> &[F] {
        let cache = self.cache.get_or_init(|| self.compile());

        // Chain can be empty, if rules disabled. But we still have to return Array.
        cache.get(chain_name).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type R = fn() -> u8;
    fn one() -> u8 {
        1
    }
    fn two() -> u8 {
        2
    }
    fn three() -> u8 {
        3
    }

    fn run(ruler: &Ruler<R>, chain: &str) -> Vec<u8> {
        ruler.get_rules(chain).iter().map(|f| f()).collect()
    }

    fn names(ruler: &Ruler<R>) -> Vec<&str> {
        ruler.rules().iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn push_before_after_at_keep_order_and_chains() {
        let mut r: Ruler<R> = Ruler::new();
        r.push("a", one, &["x"]);
        r.push("c", three, &[]);
        r.before("c", "b", two, &["x", "y"]).unwrap();
        assert_eq!(names(&r), ["a", "b", "c"]);
        assert_eq!(run(&r, ""), [1, 2, 3]);
        assert_eq!(run(&r, "x"), [1, 2]);
        assert_eq!(run(&r, "y"), [2]);
        assert_eq!(run(&r, "nope"), Vec::<u8>::new());
        r.after("a", "a2", three, &["y"]).unwrap();
        assert_eq!(names(&r), ["a", "a2", "b", "c"]);
        assert_eq!(run(&r, "y"), [3, 2]);
        // at() replaces fn and alt (alt defaults to [])
        r.at("b", one, &[]).unwrap();
        assert_eq!(run(&r, ""), [1, 3, 1, 3]);
        assert_eq!(run(&r, "x"), [1]);
        assert_eq!(
            r.at("zz", one, &[]),
            Err(RulerError("Parser rule not found: zz".into()))
        );
        assert_eq!(
            r.before("zz", "q", one, &[]),
            Err(RulerError("Parser rule not found: zz".into()))
        );
        assert_eq!(
            r.after("zz", "q", one, &[]),
            Err(RulerError("Parser rule not found: zz".into()))
        );
    }

    #[test]
    fn enable_disable_and_enable_only() {
        let mut r: Ruler<R> = Ruler::new();
        r.push("a", one, &["x"]);
        r.push("b", two, &["x"]);
        r.push("c", three, &[]);
        assert_eq!(r.disable(&["a", "zz"], true).unwrap(), ["a"]);
        assert_eq!(run(&r, ""), [2, 3]);
        assert_eq!(run(&r, "x"), [2]);
        // chains only come from enabled rules
        r.disable(&["b"], false).unwrap();
        assert_eq!(run(&r, "x"), Vec::<u8>::new());
        assert_eq!(r.enable(&["b"], false).unwrap(), ["b"]);
        assert_eq!(run(&r, "x"), [2]);
        r.enable_only(&["c"], false).unwrap();
        assert_eq!(run(&r, ""), [3]);
        assert_eq!(
            r.disable(&["c", "zz"], false),
            Err(RulerError("Rules manager: invalid rule name zz".into()))
        );
    }

    #[test]
    fn a_failed_enable_keeps_earlier_changes_and_the_stale_cache() {
        // ruler.ts throws mid-loop: the names before the bad one stay enabled and
        // `__cache__ = null` is never reached.
        let mut r: Ruler<R> = Ruler::new();
        r.push("a", one, &[]);
        r.push("b", two, &[]);
        r.disable(&["a", "b"], false).unwrap();
        assert_eq!(run(&r, ""), Vec::<u8>::new()); // compiles the cache
        assert!(r.enable(&["a", "zz", "b"], false).is_err());
        assert!(r.rules()[0].enabled && !r.rules()[1].enabled);
        assert_eq!(run(&r, ""), Vec::<u8>::new()); // stale, as in JS
        r.push("c", three, &[]);
        assert_eq!(run(&r, ""), [1, 3]);
    }
}
