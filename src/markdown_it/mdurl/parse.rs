//! Port of mdurl/lib/parse.mjs (mdurl 2.x, as bundled in upstream Markdoc 0.5.11).
//!
//! The regexes are ASCII patterns matched by hand over UTF-16 (see each matcher); `\s` is
//! JS's whitespace class ([`js::is_js_space`]) and `.` excludes JS's line terminators.

// Copyright Joyent, Inc. and other Node contributors.
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the
// "Software"), to deal in the Software without restriction, including
// without limitation the rights to use, copy, modify, merge, publish,
// distribute, sublicense, and/or sell copies of the Software, and to permit
// persons to whom the Software is furnished to do so, subject to the
// following conditions:
//
// The above copyright notice and this permission notice shall be included
// in all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
// MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN
// NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
// DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
// OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE
// USE OR OTHER DEALINGS IN THE SOFTWARE.

//
// Changes from joyent/node:
//
// 1. No leading slash in paths,
//    e.g. in `url.parse('http://foo?bar')` pathname is ``, not `/`
//
// 2. Backslashes are not replaced with slashes,
//    so `http:\\example.org\` is treated like a relative path
//
// 3. Trailing colon is treated like a part of the path,
//    i.e. in `http://example.org:foo` pathname is `:foo`
//
// 4. Nothing is URL-encoded in the resulting object,
//    (in joyent/node some chars in auth and paths are encoded)
//
// 5. `url.parse()` does not have `parseQueryString` argument
//
// 6. Removed extraneous result properties: `host`, `path`, `query`, etc.,
//    which can be constructed using other parts of the url.
//

use crate::markdown_it::js;

/// mdurl's `Url`: every part is `null` (`None`) unless the parser found it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Url {
    pub protocol: Option<Vec<u16>>,
    pub slashes: Option<bool>,
    pub auth: Option<Vec<u16>>,
    pub port: Option<Vec<u16>>,
    pub hostname: Option<Vec<u16>>,
    pub hash: Option<Vec<u16>>,
    pub search: Option<Vec<u16>>,
    pub pathname: Option<Vec<u16>>,
}

// Reference: RFC 3986, RFC 1808, RFC 2396

// define these here so at least they only have to be
// compiled once on the first module load.
//
// protocolPattern = /^([a-z0-9.+-]+:)/i       (`protocol_pattern`)
// portPattern = /:[0-9]*$/                     (`port_pattern`)

// Special case for a simple path URL
// simplePathPattern = /^(\/\/?(?!\/)[^\?\s]*)(\?[^\s]*)?$/   (`simple_path_pattern`)

// RFC 2396: characters reserved for delimiting URLs.
// We actually just auto-escape these.
// const delims = ['<', '>', '"', '`', ' ', '\r', '\n', '\t']

// RFC 2396: characters not allowed for various reasons.
// const unwise = ['{', '}', '|', '\\', '^', '`'].concat(delims)

// Allowed by RFCs, but cause of XSS attacks.  Always escape these.
// const autoEscape = ['\''].concat(unwise)

// Characters that are never ever allowed in a hostname.
// Note that any invalid chars are also handled, but these
// are the ones that are *expected* to be seen, so we fast-path
// them.
/// `nonHostChars = ['%', '/', '?', ';', '#'].concat(autoEscape)`, in that order.
const NON_HOST_CHARS: [u8; 20] = [
    b'%', b'/', b'?', b';', b'#',  // nonHostChars
    b'\'', // autoEscape
    b'{', b'}', b'|', b'\\', b'^', b'`', // unwise
    b'<', b'>', b'"', b'`', b' ', b'\r', b'\n', b'\t', // delims
];
/// `hostEndingChars = ['/', '?', '#']`.
const HOST_ENDING_CHARS: [u8; 3] = *b"/?#";
const HOSTNAME_MAX_LEN: usize = 255;
// hostnamePartPattern = /^[+a-z0-9A-Z_-]{0,63}$/            (`hostname_part_pattern`)
// hostnamePartStart = /^([+a-z0-9A-Z_-]{0,63})(.*)$/        (`hostname_part_start`)

/// `hostlessProtocol[proto]`: protocols that never have a hostname (a property lookup, so
/// case-sensitive).
fn hostless_protocol(proto: Option<&[u16]>) -> bool {
    // protocols that can allow "unsafe" and "unwise" chars.
    // protocols that never have a hostname.
    proto.is_some_and(|p| ["javascript", "javascript:"].iter().any(|k| eq(p, k)))
}

/// `slashedProtocol[proto]`: protocols that always contain a `//` bit (case-sensitive).
fn slashed_protocol(proto: Option<&[u16]>) -> bool {
    // protocols that always contain a // bit.
    proto.is_some_and(|p| {
        [
            "http", "https", "ftp", "gopher", "file", "http:", "https:", "ftp:", "gopher:", "file:",
        ]
        .iter()
        .any(|k| eq(p, k))
    })
}

fn eq(s: &[u16], ascii: &str) -> bool {
    s.len() == ascii.len() && s.iter().zip(ascii.bytes()).all(|(&a, b)| a == u16::from(b))
}

fn is(c: u16, ascii: u8) -> bool {
    c == u16::from(ascii)
}

/// `[+a-z0-9A-Z_-]`.
fn is_hostname_char(c: u16) -> bool {
    c < 0x80 && {
        let b = c as u8;
        b.is_ascii_alphanumeric() || b == b'+' || b == b'_' || b == b'-'
    }
}

/// JS's LineTerminator code points: what `.` does not match and multiline `^`/`$` see.
fn is_line_terminator(c: u16) -> bool {
    matches!(c, 0x0A | 0x0D | 0x2028 | 0x2029)
}

/// `protocolPattern.exec(rest)` → `proto[0]`: `/^([a-z0-9.+-]+:)/i`. ASCII only (without
/// the `u` flag, `i` folds no other character onto an ASCII letter).
fn protocol_pattern(rest: &[u16]) -> Option<&[u16]> {
    let n = rest
        .iter()
        .take_while(|&&c| {
            c < 0x80 && {
                let b = c as u8;
                b.is_ascii_alphanumeric() || b == b'.' || b == b'+' || b == b'-'
            }
        })
        .count();
    if n >= 1 && rest.get(n).is_some_and(|&c| is(c, b':')) {
        Some(&rest[..n + 1])
    } else {
        None
    }
}

/// `portPattern.exec(host)` → `port[0]`: `/:[0-9]*$/`. Only the last `:` can start a match
/// (an earlier one has that `:` between it and the end), so this is the last `:` when only
/// ASCII digits follow it.
fn port_pattern(host: &[u16]) -> Option<&[u16]> {
    let colon = host.iter().rposition(|&c| is(c, b':'))?;
    if host[colon + 1..]
        .iter()
        .all(|&c| c < 0x80 && (c as u8).is_ascii_digit())
    {
        Some(&host[colon..])
    } else {
        None
    }
}

/// `simplePathPattern.exec(rest)` → `[simplePath[1], simplePath[2]]`:
/// `/^(\/\/?(?!\/)[^\?\s]*)(\?[^\s]*)?$/` (no `m` flag: `$` is the end of the string).
/// Group 2 is `None` when it did not participate.
fn simple_path_pattern(rest: &[u16]) -> Option<(&[u16], Option<&[u16]>)> {
    if !rest.first().is_some_and(|&c| is(c, b'/')) {
        return None;
    }
    // `\/?(?!\/)`: a second slash is taken when there is one, and then no third may follow;
    // without it, `(?!\/)` fails on that same second slash, so there is no other way through
    let mut k = 1;
    if rest.get(1).is_some_and(|&c| is(c, b'/')) {
        if rest.get(2).is_some_and(|&c| is(c, b'/')) {
            return None;
        }
        k = 2;
    }
    // `[^\?\s]*` is greedy, and giving characters back cannot help: the next one would be
    // neither `?` nor the end
    let e = k + rest[k..]
        .iter()
        .take_while(|&&c| !is(c, b'?') && !js::is_js_space(u32::from(c)))
        .count();
    if e == rest.len() {
        return Some((&rest[..e], None));
    }
    // `(\?[^\s]*)?$`
    if is(rest[e], b'?')
        && rest[e + 1..]
            .iter()
            .all(|&c| !js::is_js_space(u32::from(c)))
    {
        return Some((&rest[..e], Some(&rest[e..])));
    }
    None
}

/// `rest.match(/^\/\/[^@\/]+@[^@\/]+/)` (truthiness).
fn slashes_auth_pattern(rest: &[u16]) -> bool {
    if !(rest.len() >= 2 && is(rest[0], b'/') && is(rest[1], b'/')) {
        return false;
    }
    let not_at_or_slash = |c: u16| !is(c, b'@') && !is(c, b'/');
    let a = 2 + rest[2..]
        .iter()
        .take_while(|&&c| not_at_or_slash(c))
        .count();
    a > 2
        && rest.get(a).is_some_and(|&c| is(c, b'@'))
        && rest.get(a + 1).is_some_and(|&c| not_at_or_slash(c))
}

/// `part.match(hostnamePartPattern)` (truthiness): `/^[+a-z0-9A-Z_-]{0,63}$/`.
fn hostname_part_pattern(part: &[u16]) -> bool {
    part.len() <= 63 && part.iter().all(|&c| is_hostname_char(c))
}

/// `part.match(hostnamePartStart)` → `[bit[1], bit[2]]`: `/^([+a-z0-9A-Z_-]{0,63})(.*)$/`.
/// Group 1 takes the longest valid prefix (at most 63); `(.*)$` must then reach the end,
/// which it cannot past a line terminator: then there is no match at all.
fn hostname_part_start(part: &[u16]) -> Option<(&[u16], &[u16])> {
    if part.iter().any(|&c| is_line_terminator(c)) {
        return None;
    }
    let n = part
        .iter()
        .take(63)
        .take_while(|&&c| is_hostname_char(c))
        .count();
    Some((&part[..n], &part[n..]))
}

/// `s.indexOf(c)`, with JS's `-1`.
fn index_of(s: &[u16], c: u8) -> i64 {
    js::index_of(s, &[u16::from(c)], 0).map_or(-1, |i| i as i64)
}

/// `s.lastIndexOf(c, from)`, with JS's `-1`.
fn last_index_of(s: &[u16], c: u8, from: Option<usize>) -> i64 {
    js::last_index_of(s, &[u16::from(c)], from).map_or(-1, |i| i as i64)
}

/// `parts.join('.')`.
fn join_dots(parts: &[Vec<u16>]) -> Vec<u16> {
    let mut out = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push(u16::from(b'.'));
        }
        out.extend_from_slice(part);
    }
    out
}

/// `parse(url, slashesDenoteHost)` (`urlParse`).
pub fn parse(url: &[u16], slashes_denote_host: bool) -> Url {
    // `if (url && url instanceof Url) return url`: callers pass strings

    let mut u = Url::default();
    u.parse(url, slashes_denote_host);
    u
}

impl Url {
    /// `Url.prototype.parse`.
    fn parse(&mut self, url: &[u16], slashes_denote_host: bool) -> &mut Self {
        // (JS's function-wide `hec` is a `let` inside each loop below)
        let mut lower_proto: Option<Vec<u16>> = None;
        // JS leaves `slashes` undefined unless the `if` below assigns it; only its
        // truthiness is read
        let mut slashes = false;
        let mut rest: Vec<u16> = url.to_vec();

        // trim before proceeding.
        // This is to support parse stuff like "  http://foo.com  \n"
        rest = js::trim(&rest).to_vec();

        if !slashes_denote_host && js::split(url, &[u16::from(b'#')]).len() == 1 {
            // Try fast path regexp
            let simple_path = simple_path_pattern(&rest);
            if let Some((path, search)) = simple_path {
                self.pathname = Some(path.to_vec());
                if let Some(search) = search.filter(|s| !s.is_empty()) {
                    self.search = Some(search.to_vec());
                }
                return self;
            }
        }

        let proto: Option<Vec<u16>> = protocol_pattern(&rest).map(<[u16]>::to_vec);
        if let Some(proto) = &proto {
            lower_proto = Some(js::to_lower_case(proto));
            self.protocol = Some(proto.clone());
            rest = js::substr(&rest, proto.len() as i64, None).to_vec();
        }
        let proto = proto.as_deref();

        // figure out if it's got a host
        // user@server is *always* interpreted as a hostname, and url
        // resolution will treat //foo/bar as host=foo,path=bar because that's
        // how the browser resolves relative URLs.
        if slashes_denote_host || proto.is_some() || slashes_auth_pattern(&rest) {
            slashes = js::substr(&rest, 0, Some(2)) == js::to_utf16("//").as_slice();
            if slashes && !(proto.is_some() && hostless_protocol(proto)) {
                rest = js::substr(&rest, 2, None).to_vec();
                self.slashes = Some(true);
            }
        }

        if !hostless_protocol(proto) && (slashes || (proto.is_some() && !slashed_protocol(proto))) {
            // there's a hostname.
            // the first instance of /, ?, ;, or # ends the host.
            //
            // If there is an @ in the hostname, then non-host chars *are* allowed
            // to the left of the last @ sign, unless some host-ending character
            // comes *before* the @-sign.
            // URLs are obnoxious.
            //
            // ex:
            // http://a@b@c/ => user:a@b host:c
            // http://a@b?@c => user:a host:c path:/?@c

            // v0.12 TODO(isaacs): This is not quite how Chrome does things.
            // Review our test case against browsers more comprehensively.

            // find the first instance of any hostEndingChars
            let mut host_end: i64 = -1;
            for i in 0..HOST_ENDING_CHARS.len() {
                let hec = index_of(&rest, HOST_ENDING_CHARS[i]);
                if hec != -1 && (host_end == -1 || hec < host_end) {
                    host_end = hec;
                }
            }

            // at this point, either we have an explicit point where the
            // auth portion cannot go past, or the last @ char is the decider.
            let at_sign: i64;
            if host_end == -1 {
                // atSign can be anywhere.
                at_sign = last_index_of(&rest, b'@', None);
            } else {
                // atSign must be in auth portion.
                // http://a@b/c@d => host:b auth:a path:/c@d
                at_sign = last_index_of(&rest, b'@', Some(host_end as usize));
            }

            // Now we have a portion which is definitely the auth.
            // Pull that off.
            if at_sign != -1 {
                let auth = js::slice(&rest, 0, Some(at_sign)).to_vec();
                rest = js::slice(&rest, at_sign + 1, None).to_vec();
                self.auth = Some(auth);
            }

            // the host is the remaining to the left of the first non-host char
            host_end = -1;
            for i in 0..NON_HOST_CHARS.len() {
                let hec = index_of(&rest, NON_HOST_CHARS[i]);
                if hec != -1 && (host_end == -1 || hec < host_end) {
                    host_end = hec;
                }
            }
            // if we still have not hit it, then the entire thing is a host.
            if host_end == -1 {
                host_end = rest.len() as i64;
            }

            // `rest[hostEnd - 1] === ':'` (undefined, so false, at -1)
            if host_end >= 1 && is(rest[(host_end - 1) as usize], b':') {
                host_end -= 1;
            }
            let host = js::slice(&rest, 0, Some(host_end)).to_vec();
            rest = js::slice(&rest, host_end, None).to_vec();

            // pull out port.
            self.parse_host(&host);

            // we've indicated that there is a hostname,
            // so even if it's empty, it has to be present.
            // (`this.hostname || ''`: parse_host only ever sets a non-empty one)
            let mut hostname: Vec<u16> = self.hostname.take().unwrap_or_default();

            // if hostname begins with [ and ends with ]
            // assume that it's an IPv6 address.
            let ipv6_hostname = hostname.first().is_some_and(|&c| is(c, b'['))
                && hostname.last().is_some_and(|&c| is(c, b']'));

            // validate a little.
            if !ipv6_hostname {
                // `split(/\./)` splits exactly as `split('.')` does
                let hostparts: Vec<Vec<u16>> = js::split(&hostname, &[u16::from(b'.')])
                    .into_iter()
                    .map(<[u16]>::to_vec)
                    .collect();
                let l = hostparts.len();
                for i in 0..l {
                    let part = &hostparts[i];
                    if part.is_empty() {
                        continue;
                    }
                    if !hostname_part_pattern(part) {
                        let mut newpart: Vec<u16> = Vec::new();
                        let k = part.len();
                        for j in 0..k {
                            if part[j] > 127 {
                                // we replace non-ASCII char with a temporary placeholder
                                // we need this to make sure size of hostname is not
                                // broken by replacing non-ASCII by nothing
                                newpart.push(u16::from(b'x'));
                            } else {
                                newpart.push(part[j]);
                            }
                        }
                        // we test again with ASCII char only
                        if !hostname_part_pattern(&newpart) {
                            let mut valid_parts: Vec<Vec<u16>> = hostparts[..i].to_vec();
                            let mut not_host: Vec<Vec<u16>> = hostparts[i + 1..].to_vec();
                            let bit = hostname_part_start(part);
                            if let Some((bit1, bit2)) = bit {
                                valid_parts.push(bit1.to_vec());
                                not_host.insert(0, bit2.to_vec());
                            }
                            if !not_host.is_empty() {
                                let mut joined = join_dots(&not_host);
                                joined.extend_from_slice(&rest);
                                rest = joined;
                            }
                            hostname = join_dots(&valid_parts);
                            break;
                        }
                    }
                }
            }

            if hostname.len() > HOSTNAME_MAX_LEN {
                hostname = Vec::new();
            }

            // strip [ and ] from the hostname
            // the host field still retains them, though
            if ipv6_hostname {
                hostname = js::substr(&hostname, 1, Some(hostname.len() as i64 - 2)).to_vec();
            }
            self.hostname = Some(hostname);
        }

        // chop off from the tail first.
        let hash = index_of(&rest, b'#');
        if hash != -1 {
            // got a fragment string.
            self.hash = Some(js::substr(&rest, hash, None).to_vec());
            rest = js::slice(&rest, 0, Some(hash)).to_vec();
        }
        let qm = index_of(&rest, b'?');
        if qm != -1 {
            self.search = Some(js::substr(&rest, qm, None).to_vec());
            rest = js::slice(&rest, 0, Some(qm)).to_vec();
        }
        if !rest.is_empty() {
            self.pathname = Some(rest);
        }
        if slashed_protocol(lower_proto.as_deref())
            && self.hostname.as_ref().is_some_and(|h| !h.is_empty())
            && self.pathname.as_ref().is_none_or(|p| p.is_empty())
        {
            self.pathname = Some(Vec::new());
        }

        self
    }

    /// `Url.prototype.parseHost`.
    fn parse_host(&mut self, host: &[u16]) {
        let mut host = host;
        let port = port_pattern(host);
        if let Some(port) = port {
            if !eq(port, ":") {
                self.port = Some(js::substr(port, 1, None).to_vec());
            }
            host = js::substr(host, 0, Some((host.len() - port.len()) as i64));
        }
        if !host.is_empty() {
            self.hostname = Some(host.to_vec());
        }
    }
}
