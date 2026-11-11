//! Port of linkify-it 6.1.0's `src/rebuilder.ts` (the version markdown-it 15.0.1 pins and
//! upstream Markdoc 0.5.11 bundles): the regular expressions linkify-it builds, as
//! hand-written matchers over UTF-16 code units.
//!
//! Each `REBuilder.get_*` regex is a function here that returns what V8's backtracking
//! matcher returns for it: the same match, at the same index, of the same length. None of
//! these regexes has the `u` flag, so they see code units (an astral character is two), and
//! most have `i`. Case-insensitivity is spelled out, never left to an engine's `i` flag: V8's
//! non-unicode `i` (ECMAScript's Canonicalize: upper-case a code unit, keep it if that leaves
//! one code unit, never map a non-ASCII unit to an ASCII one) lets an ASCII letter match only
//! its two ASCII cases, so `ſ`, `ı`, `İ`, the Kelvin sign and fullwidth letters match no
//! ASCII letter, and every class these regexes use (uc.micro's Z, P, Cc, Any, the hex, path
//! and e-mail classes) is unchanged by `i` (checked over all 65,536 code units in node with
//! `tests/next/tokenizer/linkify_classes.cjs`). [`eq_ci`] is that comparison.
//!
//! The character functions here ([`is_sep`], [`is_zcc`], [`zpcc_len`], [`pseudo_letter_len`],
//! [`eq_ci`]) answer as V8's regexes do for every BMP code unit, lone surrogates included,
//! and every astral code point: `linkify_classes.cjs --table` writes V8's answers to
//! `tests/next/tokenizer/linkify_classes.json`, which the test
//! `the_character_classes_are_v8s_for_every_code_point` checks.
//!
//! The regexes are greedy and backtrack, but their shapes let every match be computed
//! without backtracking; each function says why its answer is the regex's:
//!
//! - A host is `(?:(?:D)\.){min,max}F` followed by a terminator (a lookahead) and maybe a
//!   port. A label `D` can only consume pseudo-letters (`L`: a BMP code unit other than
//!   uc.micro's Z, P, Cc and `<`, `>`, U+FF5C, or any surrogate pair, astral punctuation
//!   included, or a lone surrogate; see [`pseudo_letter_len`]) and `-`, never a `.`, so each
//!   round of the loop has one way to succeed (the whole run of `L`/`-` up to a `.`), and the
//!   rounds are taken greedily. The final label `F` is then tried, end by end in the regex's
//!   own order, after each number of rounds from the most down. See `REBuilder::host_labels`.
//! - The path is the regex's last part and optional, so the loop's first successful round
//!   is never undone: each round takes the first alternative that matches.
//! - Bracket pairs, quoted strings, runs of dots or `!`, the e-mail name and the port each
//!   have one possible end, given what must follow them.
//!
//! The one regex matched with a regex engine is the IPv6 address: ASCII-only, without
//! lookarounds, and only ever asked whether it matches a whole string, which every engine
//! answers alike (see [`ipv6_address`]).

use std::sync::OnceLock;

use crate::markdown_it::common::ucmicro;
use crate::markdown_it::js;

/// `REBuilderOptions`, as `LinkifyIt` sets them (`schema_names` are the schemas' keys).
#[derive(Debug, Clone, PartialEq)]
pub struct REBuilderOptions {
    /// `'---'`: a `---` ends a link (a long dash).
    pub triple_dash: bool,
    /// `fuzzyIP`: an IPv4 address is a host for fuzzy links.
    pub fuzzy_ip: bool,
    /// `schema_names`, in the order `Object.keys` gives them.
    pub schema_names: Vec<Vec<u16>>,
    /// `tlds`: the top-level domains of fuzzy links.
    pub tlds: Vec<Vec<u16>>,
    /// `urlAuth`: `user:pass@` before a host.
    pub url_auth: bool,
    /// `maxLength`: the most rounds of the path loop, and how far past a schema a link is
    /// looked for.
    pub max_length: usize,
}

/// `REBuilder`: the matchers, over the options they are built from.
#[derive(Debug, Clone, PartialEq)]
pub struct REBuilder {
    pub opts: REBuilderOptions,
    /// The TLD alternatives of `get_tld()`, in the regex's order
    /// (`[...new Set(tlds)].sort().reverse()`).
    tld_list: Vec<Vec<u16>>,
}

/// A match of one of the searching regexes (`g`): `m.index`, `m[1].length` (the text before
/// the link proper) and `m.index + m[0].length`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found {
    pub index: usize,
    pub prefix: usize,
    pub end: usize,
}

impl REBuilder {
    /// `new REBuilder(opts)` followed by `set(opts)`.
    pub fn new(opts: REBuilderOptions) -> Self {
        let mut tlds: Vec<Vec<u16>> = Vec::new();
        for tld in &opts.tlds {
            if !tlds.contains(tld) {
                tlds.push(tld.clone());
            }
        }
        // `.sort()` compares UTF-16 code units, as `Vec<u16>` ordering does
        tlds.sort();
        tlds.reverse();
        // `tlds_src || '$#none#$'`: with no TLDs the alternative is a regex that never matches
        // (`$` then `#`), so no label is a TLD but an `xn--` one: an empty list.
        REBuilder {
            opts,
            tld_list: tlds,
        }
    }

    // --- `get_schema_search` / `get_schema_at_start` -----------------------------------

    /// One of `get_schema_names()` at `q` (`(http:|https:|ftp:|\/\/|mailto:)` by default,
    /// each escaped, `i`): the first that matches, as `(index in schema_names, length)`.
    fn schema_at(&self, t: &[u16], q: usize) -> Option<(usize, usize)> {
        self.opts
            .schema_names
            .iter()
            .enumerate()
            .find(|(_, name)| starts_with_ci(t, q, name))
            .map(|(k, name)| (k, name.len()))
    }

    /// The text before a schema: `(?!_)(?:[><\uff5c]|ZPCc)` at `i`, its length.
    fn schema_prefix_at(t: &[u16], i: usize) -> usize {
        match t.get(i) {
            None | Some(&0x5F) => 0,
            Some(&c) if is_sep(c) => 1,
            Some(_) => zpcc_len(t, i),
        }
    }

    /// `get_schema_search()` (`gi`) from `last_index`:
    /// `(^|(?!_)(?:[><\uff5c]|ZPCc))(<schema names>)`. Returns the match and which schema
    /// name group 2 matched (its text is `t[end - len..end]`).
    pub fn schema_search(&self, t: &[u16], last_index: usize) -> Option<(Found, usize)> {
        if last_index > t.len() {
            return None;
        }
        (last_index..=t.len()).find_map(|i| self.schema_search_at(t, i))
    }

    fn schema_search_at(&self, t: &[u16], i: usize) -> Option<(Found, usize)> {
        // group 1: `^` first, then a prefix character
        if i == 0
            && let Some((k, len)) = self.schema_at(t, 0)
        {
            let found = Found {
                index: 0,
                prefix: 0,
                end: len,
            };
            return Some((found, k));
        }
        let prefix = Self::schema_prefix_at(t, i);
        if prefix > 0
            && let Some((k, len)) = self.schema_at(t, i + prefix)
        {
            let found = Found {
                index: i,
                prefix,
                end: i + prefix + len,
            };
            return Some((found, k));
        }
        None
    }

    /// `get_schema_at_start()` (`i`): `^` and `get_schema_search()`'s source; `exec` from 0.
    pub fn schema_at_start(&self, t: &[u16]) -> Option<(Found, usize)> {
        self.schema_search_at(t, 0)
    }

    // --- validators (`iy`, matched at `pos`) ----------------------------------------------

    /// `get_http_validator()` at `pos`: the length of
    /// `\/\/` + (urlAuth ? auth : '') + url_host_port + path, or `None`.
    pub fn http_validator(&self, t: &[u16], pos: usize) -> Option<usize> {
        if !(t.get(pos) == Some(&0x2F) && t.get(pos + 1) == Some(&0x2F)) {
            return None;
        }
        let start = pos + 2;
        let after_host = |p: usize| self.url_host_port(t, p);
        let host_end = self.with_auth(t, start, after_host)?;
        Some(self.path(t, host_end) - pos)
    }

    /// `get_relative_proto_validator()` at `pos`: the length of (urlAuth ? auth : '') +
    /// `(?:localhost|ipv6_url_host|(?:(?:domain)[.]){1,10}domain_root)` + port + host
    /// terminator + path, or `None`.
    pub fn relative_proto_validator(&self, t: &[u16], pos: usize) -> Option<usize> {
        let host = |p: usize| -> Option<usize> {
            // `localhost` first; if what follows it fails, the other alternatives
            if starts_with_ci_str(t, p, "localhost")
                && let Some(end) = self.port_and_terminator(t, p + 9)
            {
                return Some(end);
            }
            if let Some(end) = ipv6_url_host(t, p)
                && let Some(end) = self.port_and_terminator(t, end)
            {
                return Some(end);
            }
            self.host_labels(t, p, 1, 10, FinalLabel::DomainRoot, |e| {
                self.port_and_terminator(t, e)
            })
        };
        let host_end = self.with_auth(t, pos, host)?;
        Some(self.path(t, host_end) - pos)
    }

    /// `get_mailto_validator()` at `pos`: the length of
    /// `mail_name@mail_host` (mail_host with its terminator), or `None`.
    pub fn mailto_validator(&self, t: &[u16], pos: usize) -> Option<usize> {
        let at = mail_name_end(t, pos)?;
        if t.get(at) != Some(&0x40) {
            return None;
        }
        let end = self.mail_host(t, at + 1)?;
        Some(end - pos)
    }

    /// `(urlAuth ? get_auth() : '')` then `rest`: `(?:(?:(?!ZCc|[@/\[\]()]).){1,50}@)?` is
    /// greedy and optional, so `rest` is tried after the auth part when there is one, and then
    /// without it. The auth part has one possible end: its characters exclude `@`, which must
    /// follow them.
    fn with_auth(
        &self,
        t: &[u16],
        p: usize,
        rest: impl Fn(usize) -> Option<usize>,
    ) -> Option<usize> {
        if self.opts.url_auth {
            let mut k = 0;
            while k < 50
                && t.get(p + k).is_some_and(|&c| {
                    !is_zcc(c) && !matches!(c, 0x40 | 0x2F | 0x5B | 0x5D | 0x28 | 0x29)
                })
            {
                k += 1;
            }
            if k >= 1
                && t.get(p + k) == Some(&0x40)
                && let Some(end) = rest(p + k + 1)
            {
                return Some(end);
            }
        }
        rest(p)
    }

    // --- hosts --------------------------------------------------------------------------

    /// `get_url_host_port()` at `p`: `(?:ipv6_url_host|(?:(?:domain)\.){0,10}domain)` + port +
    /// host terminator; the end, or `None`.
    fn url_host_port(&self, t: &[u16], p: usize) -> Option<usize> {
        if let Some(end) = ipv6_url_host(t, p)
            && let Some(end) = self.port_and_terminator(t, end)
        {
            return Some(end);
        }
        self.host_labels(t, p, 0, 10, FinalLabel::Domain, |e| {
            self.port_and_terminator(t, e)
        })
    }

    /// `get_mail_host()` at `p`: `(?:\[IPv6:ipv6\]|(?:(?:domain)\.){0,4}domain)` + host
    /// terminator; the end, or `None`.
    fn mail_host(&self, t: &[u16], p: usize) -> Option<usize> {
        if let Some(end) = ipv6_mail_host(t, p)
            && self.host_terminator(t, end)
        {
            return Some(end);
        }
        self.host_labels(t, p, 0, 4, FinalLabel::Domain, |e| {
            self.host_terminator(t, e).then_some(e)
        })
    }

    /// `get_fuzzy_mail_host()` at `p`:
    /// `(?:\[IPv6:ipv6\]|(?:(?:domain)[.]){1,4}domain_root)` + host terminator.
    fn fuzzy_mail_host(&self, t: &[u16], p: usize) -> Option<usize> {
        if let Some(end) = ipv6_mail_host(t, p)
            && self.host_terminator(t, end)
        {
            return Some(end);
        }
        self.host_labels(t, p, 1, 4, FinalLabel::DomainRoot, |e| {
            self.host_terminator(t, e).then_some(e)
        })
    }

    /// `get_fuzzy_url_host_port()` at `p`:
    /// `(?:(fuzzyIP ? ipv4 '|' : '')(?:(?:(?:domain)\.){1,10}(?:tld)))` + host terminator
    /// (no port).
    fn fuzzy_url_host(&self, t: &[u16], p: usize) -> Option<usize> {
        if self.opts.fuzzy_ip
            && let Some(end) = ipv4_address(t, p)
            && self.host_terminator(t, end)
        {
            return Some(end);
        }
        self.host_labels(t, p, 1, 10, FinalLabel::Tld, |e| {
            self.host_terminator(t, e).then_some(e)
        })
    }

    /// `(?:(?:domain)\.){min,max}F`, then `rest` (the port and terminator after the host,
    /// which returns where the match goes on): the regex's answer.
    ///
    /// A label (`get_domain()`: `xn--[a-z0-9\-]{1,59}`, or `L`, or `L(?:-|L){0,61}L`) can
    /// only take `L`s and `-`s, so the loop's round `j` must take all of the run of `L`/`-`
    /// that starts there and stop at a `.`: each round has one way to succeed, and the rounds
    /// are taken greedily, as many as can be. Then the regex tries `F`, in its own order of
    /// ends ([`REBuilder::final_label`]), at the place each number of rounds leaves, from the
    /// most rounds down.
    fn host_labels(
        &self,
        t: &[u16],
        p: usize,
        min: usize,
        max: usize,
        last: FinalLabel,
        rest: impl Fn(usize) -> Option<usize>,
    ) -> Option<usize> {
        // where each round starts, and where F starts after the last one
        let mut starts = [0usize; 11];
        starts[0] = p;
        let mut rounds = 0;
        while rounds < max {
            let a = starts[rounds];
            let run = label_run(t, a);
            if !run.is_domain(t, a) || t.get(run.end) != Some(&0x2E) {
                break;
            }
            rounds += 1;
            starts[rounds] = run.end + 1;
        }
        if rounds < min {
            return None;
        }
        (min..=rounds)
            .rev()
            .find_map(|c| self.final_label(t, starts[c], last, &rest))
    }

    /// `F` at `a`, then `rest`: the first end of `F`, in the order the regex tries them,
    /// after which `rest` succeeds.
    ///
    /// `rest` fails before an `L` and (unless `'---'` is set and two more follow) before a
    /// `-`, so mostly only `F` taking the whole run of `L`/`-` can succeed; but the ends are
    /// tried in the regex's order all the same, since with `'---'` an end before `---` can.
    fn final_label(
        &self,
        t: &[u16],
        a: usize,
        last: FinalLabel,
        rest: &impl Fn(usize) -> Option<usize>,
    ) -> Option<usize> {
        let xn = || xn_label_ends(t, a).find_map(rest);
        match last {
            // xn--[a-z0-9\-]{1,59} | (?:L) | (?:L(?:-|L){0,61}L)
            FinalLabel::Domain => xn().or_else(|| {
                let first = pseudo_letter_len(t, a);
                if first == 0 {
                    return None;
                }
                rest(a + first).or_else(|| {
                    // L(?:-|L){0,61}L: the loop takes as many items as it can, then gives
                    // them back one by one until an L follows
                    let items = label_items(t, a + first, 61);
                    let next = |j: usize| -> Option<usize> {
                        let start = if j == 0 { a + first } else { items.ends[j - 1] };
                        if j < items.count {
                            items.is_l[j].then_some(items.ends[j])
                        } else {
                            // after a full loop of 61 items, an L beyond it
                            match pseudo_letter_len(t, start) {
                                0 => None,
                                len => (items.count == 61).then_some(start + len),
                            }
                        }
                    };
                    (0..=items.count).rev().find_map(|j| next(j).and_then(rest))
                })
            }),
            // xn--[a-z0-9\-]{1,59} | L{1,63}
            FinalLabel::DomainRoot => xn().or_else(|| {
                let letters = letter_ends(t, a, 63);
                letters.iter().rev().find_map(|&e| rest(e))
            }),
            // <tlds> | xn--[a-z0-9\-]{1,59}
            FinalLabel::Tld => self
                .tld_list
                .iter()
                .filter(|tld| starts_with_ci(t, a, tld))
                .find_map(|tld| rest(a + tld.len()))
                .or_else(xn),
        }
    }

    /// `get_port()` then `get_host_terminator()` at `e`: the end, or `None`.
    ///
    /// `(?::(?:6(?:[0-4]\d{3}|5(?:[0-4]\d{2}|5(?:[0-2]\d|3[0-5])))|[1-5]?\d{1,4}))?` is
    /// optional: with a port, the terminator must hold after it, and it fails before a digit,
    /// so the port must take the whole run of digits after `:`; without one, the terminator
    /// at `:` fails when a digit follows (`(?!...:\d...)`).
    fn port_and_terminator(&self, t: &[u16], e: usize) -> Option<usize> {
        if t.get(e) == Some(&0x3A) && t.get(e + 1).is_some_and(|&c| is_digit(c)) {
            let digits = t[e + 1..]
                .iter()
                .take(6)
                .take_while(|&&c| is_digit(c))
                .count();
            let end = e + 1 + digits;
            return (is_port(&t[e + 1..end]) && self.host_terminator(t, end)).then_some(end);
        }
        self.host_terminator(t, e).then_some(e)
    }

    /// `get_host_terminator()` at `e` (a lookahead):
    /// `(?=$|[><\uff5c]|ZPCc)(?!-|_|:\d|\.-|\.(?!$|ZPCc))`, with `-(?!--)` for the first
    /// alternative when `'---'` is set.
    pub fn host_terminator(&self, t: &[u16], e: usize) -> bool {
        let n = t.len();
        if e >= n {
            return true;
        }
        let c = t[e];
        if !(is_sep(c) || zpcc_len(t, e) > 0) {
            return false;
        }
        let at = |k: usize| t.get(e + k).copied();
        let dash_ends = if self.opts.triple_dash {
            c == 0x2D && !(at(1) == Some(0x2D) && at(2) == Some(0x2D))
        } else {
            c == 0x2D
        };
        let dot_continues = c == 0x2E && e + 1 < n && zpcc_len(t, e + 1) == 0;
        !(dash_ends
            || c == 0x5F
            || (c == 0x3A && at(1).is_some_and(is_digit))
            || (c == 0x2E && at(1) == Some(0x2D))
            || dot_continues)
    }

    // --- the path -----------------------------------------------------------------------

    /// `get_path()` at `p`: where it ends (`p` when it matches nothing).
    ///
    /// `(?:[/?#](?:<alternatives>){1,maxLength}|\/)?` ends the regex and is optional, so
    /// the match is the first way through it: each round of the loop takes the first
    /// alternative that matches (`REBuilder::path_round`), and a loop that takes no round
    /// leaves `\/` (a lone slash) or nothing.
    pub fn path(&self, t: &[u16], p: usize) -> usize {
        let Some(&c) = t.get(p) else {
            return p;
        };
        if matches!(c, 0x2F | 0x3F | 0x23) {
            let mut q = p + 1;
            let mut rounds = 0;
            while rounds < self.opts.max_length {
                match self.path_round(t, q) {
                    Some(len) => {
                        q += len;
                        rounds += 1;
                    }
                    None => break,
                }
            }
            if rounds >= 1 {
                return q;
            }
        }
        if c == 0x2F { p + 1 } else { p }
    }

    /// One round of the path loop at `q`: the length the first matching alternative takes.
    fn path_round(&self, t: &[u16], q: usize) -> Option<usize> {
        let n = t.len();
        let c = *t.get(q)?;
        let next = t.get(q + 1).copied();
        // `(?!ZCc|$)` after one character
        let next_not_zcc_or_end = next.is_some_and(|d| !is_zcc(d));
        // nestedPairRE('[', ']'), ('(', ')'), ('{', '}')
        for (open, close) in [(0x5B, 0x5D), (0x28, 0x29), (0x7B, 0x7D)] {
            if c == open
                && let Some(end) = nested_pair(t, q, open, close, 4)
            {
                return Some(end - q);
            }
        }
        // \"(?:(?!ZCc|["]).){1,100}\"  and  \'(?:(?!ZCc|[']).){1,100}\'
        if (c == 0x22 || c == 0x27)
            && let Some(len) = quoted(t, q, c)
        {
            return Some(len);
        }
        // \'(?=L|[-])  (allow `I'm_king` if no pair found)
        if c == 0x27 && (next == Some(0x2D) || pseudo_letter_len(t, q + 1) > 0) {
            return Some(1);
        }
        if c == 0x2E {
            // \.{2,20}[:]?[a-zA-Z0-9%/&]: only the whole run of dots can be followed by the
            // class (a dot is not in it), so a run longer than 20 fails
            let dots = t[q..].iter().take(21).take_while(|&&d| d == 0x2E).count();
            if (2..=20).contains(&dots) {
                let e = q + dots;
                if t.get(e) == Some(&0x3A) && t.get(e + 1).is_some_and(|&d| is_path_class(d)) {
                    return Some(dots + 2);
                }
                if t.get(e).is_some_and(|&d| is_path_class(d)) {
                    return Some(dots + 1);
                }
            }
            // \.(?!ZCc|[.]|$)
            if next_not_zcc_or_end && next != Some(0x2E) {
                return Some(1);
            }
        }
        if c == 0x2D {
            let dashes = t[q..].iter().take(21).take_while(|&&d| d == 0x2D).count();
            if self.opts.triple_dash {
                // \-(?!--(?:[^-]|$))(?:-{0,19})
                let long_dash = dashes == 3;
                if !long_dash {
                    return Some(dashes.min(20));
                }
            } else {
                // \-{1,20}
                return Some(dashes.min(20));
            }
        }
        // ,(?!ZCc|$)  and  ;(?!ZCc|$)
        if (c == 0x2C || c == 0x3B) && next_not_zcc_or_end {
            return Some(1);
        }
        if c == 0x21 {
            // \!{1,20}(?!ZCc|[!]|$): only the whole run of `!` can be followed by a non-`!`
            let bangs = t[q..].iter().take(21).take_while(|&&d| d == 0x21).count();
            if bangs <= 20 && t.get(q + bangs).is_some_and(|&d| !is_zcc(d)) {
                return Some(bangs);
            }
        }
        // \?(?!ZCc|[?]|$)
        if c == 0x3F && next_not_zcc_or_end && next != Some(0x3F) {
            return Some(1);
        }
        // (?:)[\\/:%@#&=_~*]   (get_path_extra() is `new RegExp('')`, source `(?:)`)
        if matches!(
            c,
            0x5C | 0x2F | 0x3A | 0x25 | 0x40 | 0x23 | 0x26 | 0x3D | 0x5F | 0x7E | 0x2A
        ) {
            return Some(1);
        }
        // (?!ZPCc|[><\uff5c]).  (one code unit; `.` stops only at line terminators, which
        // are in ZPCc)
        if q < n && !is_sep(c) && zpcc_len(t, q) == 0 {
            return Some(1);
        }
        None
    }

    // --- searches (`gi`) ------------------------------------------------------------------

    /// `get_fuzzy_mail_host_search()` (`gi`) from `last_index`: `@` + fuzzy mail host.
    pub fn fuzzy_mail_host_search(&self, t: &[u16], last_index: usize) -> Option<Found> {
        let mut i = last_index;
        while i < t.len() {
            let at = i + t[i..].iter().position(|&c| c == 0x40)?;
            if let Some(end) = self.fuzzy_mail_host(t, at + 1) {
                return Some(Found {
                    index: at,
                    prefix: 0,
                    end,
                });
            }
            i = at + 1;
        }
        None
    }

    /// `get_fuzzy_link_search()` (`gi`) from `last_index`:
    /// ``(^|(?![.:/\-_@])(?:[$+<=>^`|\uff5c]|ZPCc))(?:(?![$+<=>^`|\uff5c])fuzzy_url_host_port path)``.
    pub fn fuzzy_link_search(&self, t: &[u16], last_index: usize) -> Option<Found> {
        if last_index > t.len() {
            return None;
        }
        (last_index..=t.len()).find_map(|i| {
            let mut prefixes = [None, None];
            if i == 0 {
                prefixes[0] = Some(0);
            }
            if let Some(&c) = t.get(i)
                && !matches!(c, 0x2E | 0x3A | 0x2F | 0x2D | 0x5F | 0x40)
            {
                let len = if is_fuzzy_prefix_symbol(c) {
                    1
                } else {
                    zpcc_len(t, i)
                };
                if len > 0 {
                    prefixes[1] = Some(len);
                }
            }
            prefixes.into_iter().flatten().find_map(|prefix| {
                let h = i + prefix;
                if t.get(h).is_some_and(|&c| is_fuzzy_prefix_symbol(c)) {
                    return None;
                }
                let host_end = self.fuzzy_url_host(t, h)?;
                Some(Found {
                    index: i,
                    prefix,
                    end: self.path(t, host_end),
                })
            })
        })
    }
}

/// The kinds of final label a host ends with.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FinalLabel {
    /// `get_domain()`
    Domain,
    /// `get_domain_root()`: `xn--[a-z0-9\-]{1,59}|L{1,63}`
    DomainRoot,
    /// `(?:<tlds>|xn--[a-z0-9\-]{1,59})`
    Tld,
}

/// The run of `L`s and `-`s that starts at a position: where it ends, how many items it has
/// (an astral `L` is one), and whether its first and last items are `L`s. Counting stops
/// past 63 items, which no label takes.
struct LabelRun {
    end: usize,
    items: usize,
    first_l: bool,
    last_l: bool,
    too_long: bool,
}

fn label_run(t: &[u16], a: usize) -> LabelRun {
    let mut run = LabelRun {
        end: a,
        items: 0,
        first_l: false,
        last_l: false,
        too_long: false,
    };
    loop {
        let (len, is_l) = if t.get(run.end) == Some(&0x2D) {
            (1, false)
        } else {
            match pseudo_letter_len(t, run.end) {
                0 => break,
                len => (len, true),
            }
        };
        if run.items == 63 {
            run.too_long = true;
            break;
        }
        if run.items == 0 {
            run.first_l = is_l;
        }
        run.last_l = is_l;
        run.items += 1;
        run.end += len;
    }
    run
}

impl LabelRun {
    /// Whether `get_domain()` can match the whole run:
    /// `xn--[a-z0-9\-]{1,59}|L|L(?:-|L){0,61}L`.
    fn is_domain(&self, t: &[u16], a: usize) -> bool {
        !self.too_long
            && self.items >= 1
            && ((self.first_l && self.last_l) || is_xn_label(t, a, self.end))
    }
}

/// Up to `max` items of `(?:-|L)` from `p`: where each ends and whether it is an `L`.
struct LabelItems {
    count: usize,
    ends: [usize; 61],
    is_l: [bool; 61],
}

fn label_items(t: &[u16], p: usize, max: usize) -> LabelItems {
    let mut items = LabelItems {
        count: 0,
        ends: [0; 61],
        is_l: [false; 61],
    };
    let mut e = p;
    while items.count < max {
        let (len, is_l) = if t.get(e) == Some(&0x2D) {
            (1, false)
        } else {
            match pseudo_letter_len(t, e) {
                0 => break,
                len => (len, true),
            }
        };
        e += len;
        items.ends[items.count] = e;
        items.is_l[items.count] = is_l;
        items.count += 1;
    }
    items
}

/// Where each of up to `max` `L`s from `p` ends.
fn letter_ends(t: &[u16], p: usize, max: usize) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut e = p;
    while ends.len() < max {
        match pseudo_letter_len(t, e) {
            0 => break,
            len => {
                e += len;
                ends.push(e);
            }
        }
    }
    ends
}

/// The ends `xn--[a-z0-9\-]{1,59}` (with `i`) can take at `a`, longest first.
fn xn_label_ends(t: &[u16], a: usize) -> impl Iterator<Item = usize> {
    let tail = if starts_with_ci_str(t, a, "xn--") {
        t[a + 4..]
            .iter()
            .take(59)
            .take_while(|&&c| c == 0x2D || (c < 0x80 && (c as u8).is_ascii_alphanumeric()))
            .count()
    } else {
        0
    };
    (1..=tail).rev().map(move |k| a + 4 + k)
}

/// Whether `t[a..end]` is `xn--[a-z0-9\-]{1,59}` (with `i`) as a whole.
fn is_xn_label(t: &[u16], a: usize, end: usize) -> bool {
    starts_with_ci_str(t, a, "xn--")
        && (a + 5..=a + 63).contains(&end)
        && t[a + 4..end]
            .iter()
            .all(|&c| c == 0x2D || (c < 0x80 && (c as u8).is_ascii_alphanumeric()))
}

/// `nestedPairRE(open, close, depth)` at `q` (where `t[q]` is `open`): the end of
/// `open(?:atom|<pair one level less>){0,1000}close`, atom `(?:(?!ZCc|open|close).)`, the
/// innermost `open atom{0,1000} close`.
///
/// Neither an atom nor a pair can take a lone `close`, so the loop stops at the first
/// `close` it does not take inside a pair, and that `close` must follow: a pair has one
/// possible end, and giving back rounds leaves an atom or an `open` where `close` must be.
fn nested_pair(t: &[u16], q: usize, open: u16, close: u16, depth: u32) -> Option<usize> {
    let mut i = q + 1;
    let mut rounds = 0;
    while rounds < 1000 {
        let Some(&c) = t.get(i) else {
            break;
        };
        if is_zcc(c) || c == close {
            break;
        }
        if c == open {
            match (depth > 1).then(|| nested_pair(t, i, open, close, depth - 1)) {
                Some(Some(end)) => i = end,
                _ => break,
            }
        } else {
            i += 1;
        }
        rounds += 1;
    }
    (t.get(i) == Some(&close)).then_some(i + 1)
}

/// `quote(?:(?!ZCc|[quote]).){1,100}quote` at `q`: its length. The quote must follow the
/// longest run of other characters (the run cannot take it).
fn quoted(t: &[u16], q: usize, quote: u16) -> Option<usize> {
    let run = t[q + 1..]
        .iter()
        .take(100)
        .take_while(|&&c| !is_zcc(c) && c != quote)
        .count();
    (run >= 1 && t.get(q + 1 + run) == Some(&quote)).then_some(run + 2)
}

/// `get_mail_name()` at `p` (`[mc](?:[mc]|[.](?=[mc])){0,63}`, mc the e-mail character
/// class): where its greedy match ends.
fn mail_name_end(t: &[u16], p: usize) -> Option<usize> {
    if !t.get(p).is_some_and(|&c| is_mail_char(c)) {
        return None;
    }
    let mut e = p + 1;
    for _ in 0..63 {
        match t.get(e) {
            Some(&c) if is_mail_char(c) => e += 1,
            Some(&0x2E) if t.get(e + 1).is_some_and(|&c| is_mail_char(c)) => e += 1,
            _ => break,
        }
    }
    Some(e)
}

/// `get_mail_name_validator()` (no flags) over `name`:
/// `(?:^|[><\uff5c]|"|\(|ZCc)(mail_name)$`, searched from 0. Returns where group 1 (the
/// name) starts.
///
/// The name must reach the end: its loop cannot take what fails `$`, and giving back rounds
/// leaves a name character or a `.` before the end. So the answer is the first start that
/// is the beginning of `name` or follows one of the prefix characters, from which a name of
/// at most 64 characters runs to the end.
pub fn mail_name_validator(name: &[u16]) -> Option<usize> {
    let n = name.len();
    // `ok[i]`: every character from `i` to the end can be a later round of the name loop
    let mut ok = vec![true; n + 1];
    for i in (0..n).rev() {
        let c = name[i];
        let round =
            is_mail_char(c) || (c == 0x2E && name.get(i + 1).is_some_and(|&d| is_mail_char(d)));
        ok[i] = round && ok[i + 1];
    }
    let name_from = |x: usize| x < n && is_mail_char(name[x]) && ok[x + 1] && n - x <= 64;
    (0..=n).find_map(|i| {
        if i == 0 && name_from(0) {
            return Some(0);
        }
        let prefix = name
            .get(i)
            .is_some_and(|&c| is_sep(c) || c == 0x22 || c == 0x28 || is_zcc(c));
        (prefix && name_from(i + 1)).then_some(i + 1)
    })
}

// --- IP addresses -----------------------------------------------------------------------

/// `get_ipv6_url_host()` at `p` (`\[ipv6\]`): the end, after `]`.
fn ipv6_url_host(t: &[u16], p: usize) -> Option<usize> {
    if t.get(p) != Some(&0x5B) {
        return None;
    }
    ipv6_then_bracket(t, p + 1)
}

/// `get_ipv6_mail_host()` at `p` (`\[IPv6:ipv6\]`, with `i`): the end, after `]`.
fn ipv6_mail_host(t: &[u16], p: usize) -> Option<usize> {
    if t.get(p) != Some(&0x5B) || !starts_with_ci_str(t, p + 1, "IPv6:") {
        return None;
    }
    ipv6_then_bracket(t, p + 6)
}

/// `ipv6\]` at `p`: the address takes only hex digits, `:` and `.`, so it must be the
/// whole run of those, and `]` must follow it.
fn ipv6_then_bracket(t: &[u16], p: usize) -> Option<usize> {
    let run = t[p.min(t.len())..]
        .iter()
        .take(46)
        .take_while(|&&c| c == 0x3A || c == 0x2E || (c < 0x80 && (c as u8).is_ascii_hexdigit()))
        .count();
    let end = p + run;
    if run > 45 || t.get(end) != Some(&0x5D) {
        return None;
    }
    let text: String = t[p..end].iter().map(|&c| char::from(c as u8)).collect();
    ipv6_address(&text).then_some(end + 1)
}

/// Whether the whole of `text` (ASCII) matches `get_ipv6_addr()`. The regex is ASCII-only
/// and has no lookarounds; whether a whole string matches does not depend on the engine, so
/// Rust's `regex` answers as V8 does. The source is the one rebuilder.ts builds (`i` adds
/// nothing: its letters are `[0-9A-Fa-f]`).
pub fn ipv6_address(text: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let h16 = "[0-9A-Fa-f]{1,4}";
        let ls32 = format!("(?:(?:{h16}:{h16})|{IPV4})");
        let source = format!(
            "^(?:(?:{h16}:){{6}}{ls32}|::(?:{h16}:){{5}}{ls32}|(?:{h16})?::(?:{h16}:){{4}}{ls32}|(?:(?:{h16}:){{0,1}}{h16})?::(?:{h16}:){{3}}{ls32}|(?:(?:{h16}:){{0,2}}{h16})?::(?:{h16}:){{2}}{ls32}|(?:(?:{h16}:){{0,3}}{h16})?::{h16}:{ls32}|(?:(?:{h16}:){{0,4}}{h16})?::{ls32}|(?:(?:{h16}:){{0,5}}{h16})?::{h16}|(?:(?:{h16}:){{0,6}}{h16})?::)$"
        );
        regex::Regex::new(&source).expect("the IPv6 regex is valid")
    })
    .is_match(text)
}

/// `get_ipv4_addr()`'s source.
const IPV4: &str = "(?:(?:25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9][0-9]|[0-9])[.]){3}(?:25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9][0-9]|[0-9])";

/// `get_ipv4_addr()` at `p`, where a host terminator must follow: the end. An octet is
/// followed by `.` or by the terminator (which fails before a digit), so each must be a
/// whole run of digits: `25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9][0-9]|[0-9]`, 0 to 255 without
/// a leading zero.
fn ipv4_address(t: &[u16], p: usize) -> Option<usize> {
    let mut e = p;
    for k in 0..4 {
        let digits = t[e.min(t.len())..]
            .iter()
            .take(4)
            .take_while(|&&c| is_digit(c))
            .count();
        let octet = &t[e..e + digits];
        let valid = match digits {
            1 => true,
            2 => octet[0] != 0x30,
            3 => {
                let v = octet
                    .iter()
                    .fold(0u32, |v, &d| v * 10 + u32::from(d - 0x30));
                octet[0] != 0x30 && v <= 255
            }
            _ => false,
        };
        if !valid {
            return None;
        }
        e += digits;
        if k < 3 {
            if t.get(e) != Some(&0x2E) {
                return None;
            }
            e += 1;
        }
    }
    Some(e)
}

/// Whether a run of 1 to 5 digits is a whole match of
/// `6(?:[0-4]\d{3}|5(?:[0-4]\d{2}|5(?:[0-2]\d|3[0-5])))|[1-5]?\d{1,4}`: up to four digits,
/// or five starting with 1 to 5, or 60000 to 65535.
fn is_port(digits: &[u16]) -> bool {
    match digits.len() {
        1..=4 => true,
        5 => match digits[0] {
            0x31..=0x35 => true,
            0x36 => {
                digits
                    .iter()
                    .fold(0u32, |v, &d| v * 10 + u32::from(d - 0x30))
                    <= 65535
            }
            _ => false,
        },
        _ => false,
    }
}

// --- characters ---------------------------------------------------------------------------

fn is_digit(c: u16) -> bool {
    (0x30..=0x39).contains(&c)
}

/// `[><\uff5c]` (`get_text_separators()`).
pub fn is_sep(c: u16) -> bool {
    c == 0x3E || c == 0x3C || c == 0xFF5C
}

/// uc.micro's Z: `[ \xA0  -     　]`.
fn is_z(c: u16) -> bool {
    matches!(
        c,
        0x20 | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000
    )
}

/// uc.micro's Cc: `[\0-\x1F\x7F-\x9F]`.
fn is_cc(c: u16) -> bool {
    c <= 0x1F || (0x7F..=0x9F).contains(&c)
}

/// `src_ZCc` (Z or Cc), one code unit.
pub fn is_zcc(c: u16) -> bool {
    is_z(c) || is_cc(c)
}

fn is_high(c: u16) -> bool {
    (0xD800..=0xDBFF).contains(&c)
}

fn is_low(c: u16) -> bool {
    (0xDC00..=0xDFFF).contains(&c)
}

/// `src_ZPCc` (`Z|P|Cc`) at `p`: how many code units it matches (an astral punctuation
/// character is a surrogate pair), or 0.
pub fn zpcc_len(t: &[u16], p: usize) -> usize {
    let Some(&c) = t.get(p) else {
        return 0;
    };
    if is_z(c) || is_cc(c) {
        return 1;
    }
    if is_high(c) {
        return match t.get(p + 1) {
            Some(&d) if is_low(d) => {
                let cp = 0x10000 + ((u32::from(c) - 0xD800) << 10) + (u32::from(d) - 0xDC00);
                if ucmicro::is_p(cp) { 2 } else { 0 }
            }
            _ => 0,
        };
    }
    if !is_low(c) && ucmicro::is_p(u32::from(c)) {
        1
    } else {
        0
    }
}

/// `get_pseudo_letter()` at `p`: the length of the first alternative that matches, or 0.
///
/// Its source is `(?:(?![><\uff5c]|ZPCc)${src_Any})`, and uc.micro's `Any` is a top-level
/// alternation, so the lookahead binds only to Any's first alternative:
///
/// ```text
/// (?:(?![><\uff5c]|ZPCc)[\0-\uD7FF\uE000-\uFFFF]  1. a BMP unit: not a separator, Z, P or Cc
///   |[\uD800-\uDBFF][\uDC00-\uDFFF]                2. any surrogate pair
///   |[\uD800-\uDBFF](?![\uDC00-\uDFFF])            3. a lone high surrogate
///   |(?:[^\uD800-\uDBFF]|^)[\uDC00-\uDFFF])        4. a low surrogate, with the unit before
///                                                     it (any but a high one), or at the start
/// ```
///
/// So every astral character is a pseudo-letter, uc.micro's astral punctuation included, and
/// so is every lone surrogate; with a lone low surrogate after it, a separator, a space or a
/// punctuation mark is one too (the fourth alternative takes both).
///
/// Where the first alternative matches, the fourth would match as well (two units) if a lone
/// low surrogate followed, and a regex that fails after the first backtracks into it. The
/// matchers built on this length do not; markdown-it never gives linkify-it a lone low
/// surrogate (see "Divergences" in [`super`]'s docs).
pub fn pseudo_letter_len(t: &[u16], p: usize) -> usize {
    let Some(&c) = t.get(p) else {
        return 0;
    };
    let low_follows = t.get(p + 1).is_some_and(|&d| is_low(d));
    if is_high(c) {
        // 2. a pair, 3. a lone high surrogate
        return if low_follows { 2 } else { 1 };
    }
    if !is_low(c) && !is_sep(c) && zpcc_len(t, p) == 0 {
        return 1;
    }
    // 4. `[^\uD800-\uDBFF]` takes `c` (not a high surrogate) and a low surrogate follows; or
    // `^`, and `c` is the low surrogate
    if low_follows {
        2
    } else if is_low(c) && p == 0 {
        1
    } else {
        0
    }
}

/// The e-mail name class ``[-!#$%&'*+/=?^_`{|}~a-zA-Z0-9]``.
fn is_mail_char(c: u16) -> bool {
    c < 0x80 && {
        let b = c as u8;
        b.is_ascii_alphanumeric() || b"-!#$%&'*+/=?^_`{|}~".contains(&b)
    }
}

/// `[a-zA-Z0-9%/&]`
fn is_path_class(c: u16) -> bool {
    c < 0x80 && {
        let b = c as u8;
        b.is_ascii_alphanumeric() || b == b'%' || b == b'/' || b == b'&'
    }
}

/// ``[$+<=>^`|\uff5c]``: what may precede a fuzzy link but not start its host.
fn is_fuzzy_prefix_symbol(c: u16) -> bool {
    matches!(
        c,
        0x24 | 0x2B | 0x3C | 0x3D | 0x3E | 0x5E | 0x60 | 0x7C | 0xFF5C
    )
}

/// ECMAScript's Canonicalize for a non-unicode `i` regex (which V8 follows): the code unit
/// upper-cased, unless that is not one code unit, or maps a non-ASCII unit to ASCII.
pub fn canon(c: u16) -> u16 {
    if c < 0x80 {
        return (c as u8).to_ascii_uppercase().into();
    }
    if is_high(c) || is_low(c) {
        return c;
    }
    match js::to_upper_case(&[c])[..] {
        [u] if u >= 0x80 => u,
        _ => c,
    }
}

/// Whether two code units match each other in a non-unicode `i` regex.
pub fn eq_ci(c: u16, p: u16) -> bool {
    c == p
        || (p < 0x80 && c < 0x80 && canon(c) == canon(p))
        || (p >= 0x80 && c >= 0x80 && canon(c) == canon(p))
}

/// Whether `t[q..]` starts with `s` in a non-unicode `i` regex.
fn starts_with_ci(t: &[u16], q: usize, s: &[u16]) -> bool {
    q.checked_add(s.len())
        .and_then(|end| t.get(q..end))
        .is_some_and(|w| w.iter().zip(s).all(|(&c, &p)| eq_ci(c, p)))
}

fn starts_with_ci_str(t: &[u16], q: usize, s: &str) -> bool {
    q.checked_add(s.len())
        .and_then(|end| t.get(q..end))
        .is_some_and(|w| w.iter().zip(s.bytes()).all(|(&c, p)| eq_ci(c, p.into())))
}
