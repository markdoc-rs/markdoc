//! Port of mdurl 2.x (index.mjs and lib/*.mjs), the URL utilities markdown-it uses, as
//! bundled in upstream Markdoc 0.5.11.
//!
//! JS strings are UTF-16: these take and return `&[u16]` / `Vec<u16>`.

pub mod decode;
pub mod encode;
pub mod format;
pub mod parse;

pub use decode::decode;
pub use encode::encode;
pub use format::format;
pub use parse::{Url, parse};

/// The property names of `Object.prototype` (node 22). `encodeCache[exclude]` and
/// `decodeCache[exclude]` are lookups on plain objects, so these names find an inherited
/// member instead of a memoized table.
pub(crate) const OBJECT_PROTOTYPE_KEYS: [&str; 12] = [
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "__proto__",
    "toLocaleString",
];

/// Whether `({})[key]` is an inherited member (truthy) rather than `undefined`.
fn object_prototype_has(key: &[u16]) -> bool {
    OBJECT_PROTOTYPE_KEYS
        .iter()
        .any(|k| k.encode_utf16().eq(key.iter().copied()))
}

/// The generated URLs of tests/markdown_it/core_cases.json ("urls") and node's digests over
/// them.
///
/// `generate` mirrors the generator of tests/markdown_it/core_values.cjs step for step
/// (xorshift32 over the same pieces); the `inputs` digest checks that both produce the same
/// URLs. Each function's output is written as the same u32 symbols as there, and folded with
/// FNV-1a into one digest per function. With `MDIT_MDURL_DUMP=FILE`, `check` also writes
/// every case's symbols (the format of `core_values.cjs --dump`), to find which case differs.
#[cfg(test)]
pub(crate) mod gen_cases {
    use std::io::Write;

    use super::Url;
    use crate::markdown_it::js::RangeError;

    pub const NULL: u32 = 0;
    pub const TRUE: u32 = 0x7FFF_FFF1;
    pub const FALSE: u32 = 0x7FFF_FFF0;
    pub const ERROR: u32 = 0xFFFF_FFFF;
    pub const FNV_OFFSET: u32 = 2166136261;

    pub struct Spec {
        seed: u32,
        random_cases: usize,
        units: Vec<u16>,
        pieces: Vec<(String, Vec<Vec<u16>>)>,
        systematic: Vec<(String, Vec<Vec<u16>>)>,
        count: usize,
        digests: serde_json::Map<String, serde_json::Value>,
    }

    pub fn units(v: &serde_json::Value) -> Vec<u16> {
        v.as_array()
            .expect("array of code units")
            .iter()
            .map(|x| x.as_u64().expect("code unit") as u16)
            .collect()
    }

    pub fn named_lists(v: &serde_json::Value) -> Vec<(String, Vec<Vec<u16>>)> {
        v.as_array()
            .expect("[name, list] pairs")
            .iter()
            .map(|p| {
                let name = p[0].as_str().expect("list name").to_owned();
                let list = p[1].as_array().expect("list").iter().map(units).collect();
                (name, list)
            })
            .collect()
    }

    /// tests/markdown_it/core_cases.json, written by core_values.cjs --write.
    pub fn cases_json() -> serde_json::Value {
        serde_json::from_str(include_str!("../../../tests/markdown_it/core_cases.json"))
            .expect("core_cases.json")
    }

    pub fn spec() -> Spec {
        let v = &cases_json()["urls"];
        Spec {
            seed: v["seed"].as_u64().expect("seed") as u32,
            random_cases: v["random_cases"].as_u64().expect("random_cases") as usize,
            units: units(&v["units"]),
            pieces: named_lists(&v["pieces"]),
            systematic: named_lists(&v["systematic"]),
            count: v["count"].as_u64().expect("count") as usize,
            digests: v["digests"].as_object().expect("digests").clone(),
        }
    }

    struct Rng(u32);

    impl Rng {
        fn rnd(&mut self, n: usize) -> usize {
            let mut s = self.0;
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            self.0 = s;
            (s % n as u32) as usize
        }
    }

    pub fn generate(spec: &Spec) -> Vec<Vec<u16>> {
        let mut rng = Rng(spec.seed);
        let list = |name: &str| -> &Vec<Vec<u16>> {
            &spec
                .pieces
                .iter()
                .find(|(n, _)| n == name)
                .expect("piece list")
                .1
        };
        let pick = |rng: &mut Rng, name: &str| -> Vec<u16> {
            let l = list(name);
            l[rng.rnd(l.len())].clone()
        };
        let host = |rng: &mut Rng| -> Vec<u16> {
            let n = rng.rnd(4);
            let mut out = Vec::new();
            for i in 0..n {
                if i > 0 {
                    out.extend(pick(rng, "sep"));
                }
                out.extend(pick(rng, "label"));
            }
            out
        };
        let structured = |rng: &mut Rng| -> Vec<u16> {
            let mut out = Vec::new();
            out.extend(pick(rng, "ws"));
            out.extend(pick(rng, "scheme"));
            out.extend(pick(rng, "slashes"));
            out.extend(pick(rng, "auth"));
            out.extend(host(rng));
            out.extend(pick(rng, "port"));
            out.extend(pick(rng, "path"));
            out.extend(pick(rng, "query"));
            out.extend(pick(rng, "frag"));
            out.extend(pick(rng, "ws"));
            out
        };
        let soup = |rng: &mut Rng| -> Vec<u16> {
            let len = rng.rnd(24);
            let mut out = Vec::new();
            for _ in 0..len {
                if rng.rnd(4) == 0 {
                    let l = &spec.pieces[rng.rnd(spec.pieces.len())].1;
                    out.extend_from_slice(&l[rng.rnd(l.len())]);
                } else {
                    out.push(spec.units[rng.rnd(spec.units.len())]);
                }
            }
            out
        };
        let cat = |parts: &[&dyn Fn() -> Vec<u16>]| -> Vec<u16> {
            parts.iter().flat_map(|p| p()).collect()
        };
        let s = |x: &str| x.encode_utf16().collect::<Vec<u16>>();

        let mut cases: Vec<Vec<u16>> = Vec::new();
        for c in 0..128u16 {
            let c1 = || vec![c];
            cases.push(cat(&[
                &|| s("http://h/a"),
                &c1,
                &|| s("b?q"),
                &c1,
                &|| s("#f"),
                &c1,
            ]));
            cases.push(vec![c]);
            cases.push(cat(&[&|| s("%"), &c1, &|| s("A")]));
            cases.push(cat(&[&|| s("//h"), &c1, &|| s("x/")]));
            cases.push(cat(&[&|| s("mailto:a"), &c1, &|| s("@b"), &c1, &|| {
                s(".c")
            }]));
        }
        let sys = |name: &str| -> &Vec<Vec<u16>> {
            &spec
                .systematic
                .iter()
                .find(|(n, _)| n == name)
                .expect("systematic list")
                .1
        };
        for label in list("label") {
            for prefix in sys("host_prefix") {
                for suffix in sys("host_suffix") {
                    cases.push([prefix.as_slice(), label, suffix].concat());
                }
            }
        }
        for ip in sys("ipv6") {
            for (k, open) in sys("ipv6_open").iter().enumerate() {
                for prefix in sys("ipv6_prefix") {
                    cases.push([prefix.as_slice(), open, ip, &sys("ipv6_close")[k]].concat());
                }
            }
        }
        for b in 0..256u32 {
            let hex = format!("{b:02X}");
            let lower = hex.to_lowercase();
            cases.push(s(&format!("%{hex}")));
            cases.push(s(&format!("%{lower}")));
            cases.push(s(&format!("/%{hex}%80")));
            cases.push(s(&format!("%{hex}%BF%BF")));
            cases.push(s(&format!("%{hex}%80%80%80")));
            cases.push(s(&format!("%{hex}%41")));
            cases.push(s(&format!("%{hex}%{hex}")));
        }
        for _ in 0..spec.random_cases {
            let m = rng.rnd(10);
            let u = if m < 6 {
                structured(&mut rng)
            } else if m < 8 {
                soup(&mut rng)
            } else if m < 9 {
                host(&mut rng)
            } else {
                let mut u = structured(&mut rng);
                let n = rng.rnd(4) + 1;
                for _ in 0..n {
                    let at = rng.rnd(u.len() + 1);
                    let unit = spec.units[rng.rnd(spec.units.len())];
                    u.insert(at, unit);
                }
                u
            };
            cases.push(u);
        }
        cases
    }

    /// One function's output on one case, as the symbols core_values.cjs writes.
    #[derive(Default)]
    pub struct Syms(pub Vec<u32>);

    impl Syms {
        pub fn str(&mut self, s: &[u16]) {
            self.0.push(s.len() as u32 + 1);
            self.0.extend(s.iter().map(|&c| u32::from(c)));
        }

        /// A string or `null`.
        pub fn opt(&mut self, s: Option<&[u16]>) {
            match s {
                Some(s) => self.str(s),
                None => self.0.push(NULL),
            }
        }

        pub fn boolean(&mut self, b: bool) {
            self.0.push(if b { TRUE } else { FALSE });
        }

        /// A string, or the message of what JS threw; the string, if any.
        pub fn result(&mut self, r: Result<Vec<u16>, RangeError>) -> Option<Vec<u16>> {
            match r {
                Ok(s) => {
                    self.str(&s);
                    Some(s)
                }
                Err(e) => {
                    self.0.push(ERROR);
                    self.str(&e.0.encode_utf16().collect::<Vec<u16>>());
                    None
                }
            }
        }

        pub fn url(&mut self, u: &Url) {
            self.opt(u.protocol.as_deref());
            match u.slashes {
                Some(b) => self.boolean(b),
                None => self.0.push(NULL),
            }
            self.opt(u.auth.as_deref());
            self.opt(u.port.as_deref());
            self.opt(u.hostname.as_deref());
            self.opt(u.hash.as_deref());
            self.opt(u.search.as_deref());
            self.opt(u.pathname.as_deref());
        }
    }

    // FNV-1a over the little-endian bytes of each symbol
    pub fn fold(mut h: u32, sym: u32) -> u32 {
        for b in sym.to_le_bytes() {
            h = (h ^ u32::from(b)).wrapping_mul(16777619);
        }
        h
    }

    pub type Function = (&'static str, fn(&mut Syms, &[u16]));

    /// Runs each function over every generated URL and compares its digest with node's.
    pub fn check(functions: &[Function]) {
        let spec = spec();
        let cases = generate(&spec);
        assert_eq!(
            cases.len(),
            spec.count,
            "the generators disagree on the number of cases"
        );
        let mut inputs = FNV_OFFSET;
        let mut digests = vec![FNV_OFFSET; functions.len()];
        let mut dump = std::env::var_os("MDIT_MDURL_DUMP")
            .map(|p| std::io::BufWriter::new(std::fs::File::create(p).expect("dump file")));
        for (i, u) in cases.iter().enumerate() {
            let mut s = Syms::default();
            s.str(u);
            inputs = s.0.iter().fold(inputs, |h, &x| fold(h, x));
            let mut outputs = serde_json::Map::new();
            for (k, (name, f)) in functions.iter().enumerate() {
                let mut o = Syms::default();
                f(&mut o, u);
                digests[k] = o.0.iter().fold(digests[k], |h, &x| fold(h, x));
                if dump.is_some() {
                    outputs.insert((*name).to_owned(), o.0.into());
                }
            }
            if let Some(d) = dump.as_mut() {
                let line = serde_json::json!({"i": i, "input": u, "outputs": outputs});
                writeln!(d, "{line}").expect("dump");
            }
        }
        let node = |name: &str| spec.digests[name].as_u64().expect("digest") as u32;
        assert_eq!(
            inputs,
            node("inputs"),
            "the generators disagree on the URLs"
        );
        let differ: Vec<&str> = functions
            .iter()
            .zip(&digests)
            .filter(|((name, _), d)| **d != node(name))
            .map(|((name, _), _)| *name)
            .collect();
        assert!(
            differ.is_empty(),
            "over {} generated URLs, the output digest differs from node's for {differ:?} \
             (MDIT_MDURL_DUMP=FILE and core_values.cjs --dump show which URLs)",
            cases.len()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::gen_cases::{Function, Syms, check};
    use super::*;
    use crate::markdown_it::{MarkdownIt, js};

    #[test]
    fn generated_urls_match_node() {
        let functions: [Function; 15] = [
            ("parse_true", |o, s| o.url(&parse(s, true))),
            ("parse_false", |o, s| o.url(&parse(s, false))),
            ("format_true", |o, s| o.str(&format(&parse(s, true)))),
            ("format_false", |o, s| o.str(&format(&parse(s, false)))),
            ("encode", |o, s| o.str(&encode(s, None, None))),
            ("encode_component", |o, s| {
                o.str(&encode(s, Some(encode::COMPONENT_CHARS), None))
            }),
            ("encode_empty", |o, s| o.str(&encode(s, Some(""), None))),
            ("encode_not_escaped", |o, s| {
                o.str(&encode(s, None, Some(false)))
            }),
            ("encode_component_not_escaped", |o, s| {
                o.str(&encode(s, Some(encode::COMPONENT_CHARS), Some(false)))
            }),
            ("encode_proto", |o, s| {
                o.str(&encode(s, Some("toString"), None))
            }),
            ("decode", |o, s| o.str(&decode(s, None))),
            ("decode_percent", |o, s| {
                o.str(&decode(s, Some(";/?:@&=+$,#%")))
            }),
            ("decode_component", |o, s| {
                o.str(&decode(s, Some(decode::COMPONENT_CHARS)))
            }),
            ("decode_proto", |o, s| o.str(&decode(s, Some("__proto__")))),
            ("decode_encode", |o, s| {
                o.str(&decode(&encode(s, None, None), None))
            }),
        ];
        check(&functions);
    }

    #[test]
    fn generated_urls_normalize_like_markdown_it() {
        thread_local! {
            static MD: MarkdownIt = MarkdownIt::default();
        }
        let functions: [Function; 3] = [
            ("normalize_link", |o: &mut Syms, s| {
                MD.with(|md| o.str(&md.normalize_link(s)))
            }),
            ("normalize_link_text", |o, s| {
                MD.with(|md| o.str(&md.normalize_link_text(s)))
            }),
            ("validate_link", |o, s| {
                MD.with(|md| o.boolean(md.validate_link(s)))
            }),
        ];
        check(&functions);
    }

    fn u(s: &str) -> Vec<u16> {
        js::to_utf16(s)
    }

    /// The fields in core_values.cjs's order: protocol, slashes, auth, port, hostname, hash,
    /// search, pathname (`""` stands for `null`, `"true"` for `slashes: true`).
    fn fields(url: &str, slashes_denote_host: bool) -> [String; 8] {
        let p = parse(&u(url), slashes_denote_host);
        let s = |v: &Option<Vec<u16>>| v.as_deref().map_or("<null>".to_owned(), js::to_string);
        [
            s(&p.protocol),
            p.slashes.map_or("<null>".into(), |b| b.to_string()),
            s(&p.auth),
            s(&p.port),
            s(&p.hostname),
            s(&p.hash),
            s(&p.search),
            s(&p.pathname),
        ]
    }

    #[test]
    fn parse_examples() {
        // node: mdurl.parse(url, true|false) (tests/markdown_it/core_values.cjs --examples)
        let n = "<null>";
        for (url, expected) in [
            (
                "  HTTP://u:p@WWW.Example.com:8080/p/a?q=1#f  ",
                [
                    "HTTP:",
                    "true",
                    "u:p",
                    "8080",
                    "WWW.Example.com",
                    "#f",
                    "?q=1",
                    "/p/a",
                ],
            ),
            ("http://a@b@c/", ["http:", "true", "a@b", n, "c", n, n, "/"]),
            (
                "http://a@b?@c",
                ["http:", "true", "a", n, "b", n, "?@c", ""],
            ),
            (
                "mailto:user@host",
                ["mailto:", n, "user", n, "host", n, n, n],
            ),
            ("javascript://x", ["javascript:", n, n, n, n, n, n, "//x"]),
            ("//h/p", [n, "true", n, n, "h", n, n, "/p"]),
            (
                "http://[::1]:80/x",
                ["http:", "true", n, "80", "::1", n, n, "/x"],
            ),
            (
                "http://ex ample.com/",
                ["http:", "true", n, n, "ex", n, n, " ample.com/"],
            ),
            ("skype:name", ["skype:", n, n, n, "name", n, n, n]),
            (
                "http://a.b!c.d/e",
                ["http:", "true", n, n, "a.b", n, n, "!c.d/e"],
            ),
            (
                "http://\u{e4}\u{f6}.de:abc/",
                ["http:", "true", n, n, "\u{e4}\u{f6}.de", n, n, ":abc/"],
            ),
            ("file:", ["file:", n, n, n, n, n, n, n]),
            ("http://x.y:", ["http:", "true", n, n, "x.y", n, n, ":"]),
            ("a:b", ["a:", n, n, n, "b", n, n, n]),
        ] {
            assert_eq!(fields(url, true), expected.map(String::from), "{url:?}");
        }
        let long = format!("http://{}.com", "a".repeat(64));
        assert_eq!(fields(&long, true)[4], "a".repeat(63));
        assert_eq!(fields(&long, true)[7], "a.com");
        for (url, expected) in [
            ("/a/b?c", [n, n, n, n, n, n, "?c", "/a/b"]),
            ("//a/b", [n, n, n, n, n, n, n, "//a/b"]),
            ("///a", [n, n, n, n, n, n, n, "///a"]),
            ("/a b", [n, n, n, n, n, n, n, "/a b"]),
            ("/a?b c", [n, n, n, n, n, n, "?b c", "/a"]),
            ("#x", [n, n, n, n, n, "#x", n, n]),
            ("a/b", [n, n, n, n, n, n, n, "a/b"]),
        ] {
            assert_eq!(fields(url, false), expected.map(String::from), "{url:?}");
        }
    }

    #[test]
    fn normalize_link_examples() {
        // node: md.normalizeLink / md.normalizeLinkText (core_values.cjs --examples)
        let md = MarkdownIt::default();
        let link = |x: &str| js::to_string(&md.normalize_link(&u(x)));
        let text = |x: &str| js::to_string(&md.normalize_link_text(&u(x)));
        assert_eq!(
            link("http://ex\u{e4}mple.com/\u{e4}?q=\u{e4}#\u{e4}"),
            "http://xn--exmple-cua.com/%C3%A4?q=%C3%A4#%C3%A4"
        );
        // protocols are compared case-sensitively: HTTP: hostnames are not punycoded
        assert_eq!(link("HTTP://\u{e4}.com"), "HTTP://%C3%A4.com");
        assert_eq!(link("mailto:u@\u{e4}.de"), "mailto:u@xn--4ca.de");
        assert_eq!(link("//\u{e4}/x"), "//xn--4ca/x");
        assert_eq!(link("skype:\u{e4}"), "skype:%C3%A4");
        assert_eq!(link("http://[::1]/"), "http://[::1]/");
        assert_eq!(link("%zz%41"), "%25zz%41");
        assert_eq!(
            text("http://xn--exmple-cua.com/%C3%A4?q=%25#%2F"),
            "http://ex\u{e4}mple.com/\u{e4}?q=%25#%2F"
        );
        // punycode throws (overflow): the hostname stays as it was
        assert_eq!(text("http://xn--99999999999/"), "http://xn--99999999999/");
        assert_eq!(text("mailto:u@xn--tda.de"), "mailto:u@\u{fc}.de");
        assert_eq!(text("%3B%2f%25"), "%3B%2F%25");
        assert_eq!(text("http://%C3%A4@x/"), "http://\u{e4}@x/");
    }
}
