//! Port of mdurl/lib/format.mjs.

use super::parse::Url;
use crate::markdown_it::js;

/// `format(url)`: the parts of `url` joined back into a string.
pub fn format(url: &Url) -> Vec<u16> {
    // JS truthiness: a part counts when it is a non-empty string
    fn part(p: &Option<Vec<u16>>) -> Option<&[u16]> {
        p.as_deref().filter(|v| !v.is_empty())
    }
    let mut result: Vec<u16> = Vec::new();

    result.extend_from_slice(part(&url.protocol).unwrap_or_default());
    if url.slashes == Some(true) {
        result.extend(js::to_utf16("//"));
    }
    if let Some(auth) = part(&url.auth) {
        result.extend_from_slice(auth);
        result.push(u16::from(b'@'));
    }

    match part(&url.hostname) {
        Some(hostname) if js::index_of(hostname, &[u16::from(b':')], 0).is_some() => {
            // ipv6 address
            result.push(u16::from(b'['));
            result.extend_from_slice(hostname);
            result.push(u16::from(b']'));
        }
        hostname => result.extend_from_slice(hostname.unwrap_or_default()),
    }

    if let Some(port) = part(&url.port) {
        result.push(u16::from(b':'));
        result.extend_from_slice(port);
    }
    result.extend_from_slice(part(&url.pathname).unwrap_or_default());
    result.extend_from_slice(part(&url.search).unwrap_or_default());
    result.extend_from_slice(part(&url.hash).unwrap_or_default());

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> Option<Vec<u16>> {
        Some(js::to_utf16(x))
    }

    #[test]
    fn format_matches_mdurl() {
        // node: mdurl.format({protocol:'http:', slashes:true, auth:'u:p', hostname:'::1',
        //   port:'8080', pathname:'/a', search:'?q', hash:'#h'}) === 'http://u:p@[::1]:8080/a?q#h'
        let url = Url {
            protocol: s("http:"),
            slashes: Some(true),
            auth: s("u:p"),
            port: s("8080"),
            hostname: s("::1"),
            hash: s("#h"),
            search: s("?q"),
            pathname: s("/a"),
        };
        assert_eq!(js::to_string(&format(&url)), "http://u:p@[::1]:8080/a?q#h");
        // empty strings count as absent: format({auth:'', port:'', hostname:'h'}) === 'h'
        let url = Url {
            auth: s(""),
            port: s(""),
            hostname: s("h"),
            ..Url::default()
        };
        assert_eq!(js::to_string(&format(&url)), "h");
    }
}
