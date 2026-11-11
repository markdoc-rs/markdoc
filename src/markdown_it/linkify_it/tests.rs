use super::*;

fn u(s: &str) -> Vec<u16> {
    js::to_utf16(s)
}

fn links(l: &LinkifyIt, s: &str) -> Vec<(usize, usize, String, String)> {
    l.match_all(&u(s))
        .unwrap()
        .unwrap_or_default()
        .into_iter()
        .map(|m| {
            (
                m.index,
                m.last_index,
                js::to_string(&m.schema),
                js::to_string(&m.url),
            )
        })
        .collect()
}

#[test]
fn default_schemas_and_fuzzy_emails() {
    let l = LinkifyIt::new();
    assert!(l.test(&u("see https://example.com/a?b#c.")).unwrap());
    assert_eq!(
        links(&l, "see https://example.com/a?b#c. and x@y.org"),
        [
            (4, 29, "https:".into(), "https://example.com/a?b#c".into()),
            (35, 42, "mailto:".into(), "mailto:x@y.org".into()),
        ]
    );
    // no fuzzy links by default
    assert!(!l.test(&u("www.example.com")).unwrap());
}

/// linkify-it's options from a request (`tests/next/tokenizer/linkify_oracle.cjs` takes the
/// same object): JS truthiness for the switches.
fn options_of(v: &serde_json::Value) -> LinkifyOptions {
    let mut o = LinkifyOptions::default();
    let flag = |k: &str, d: bool| {
        v.get(k).map_or(d, |x| {
            !matches!(x, serde_json::Value::Null | serde_json::Value::Bool(false))
        })
    };
    o.fuzzy_link = flag("fuzzyLink", o.fuzzy_link);
    o.fuzzy_email = flag("fuzzyEmail", o.fuzzy_email);
    o.fuzzy_ip = flag("fuzzyIP", o.fuzzy_ip);
    o.triple_dash = flag("---", o.triple_dash);
    o.url_auth = flag("urlAuth", o.url_auth);
    if let Some(n) = v.get("maxLength").and_then(serde_json::Value::as_u64) {
        o.max_length = n as usize;
    }
    o
}

fn match_json(m: &Match) -> serde_json::Value {
    serde_json::json!({
        "schema": js::to_string(&m.schema),
        "index": m.index,
        "lastIndex": m.last_index,
        "raw": js::to_string(&m.raw),
        "text": js::to_string(&m.text),
        "url": js::to_string(&m.url),
    })
}

/// The port's half of `tests/next/tokenizer/linkify_fuzz.py`: answers the JSONL requests in
/// `$LINKIFY_REQUESTS` (`{id, text, options?}`) into `$LINKIFY_ANSWERS`, as
/// `linkify_oracle.cjs` answers them. Does nothing without the variables.
#[test]
#[ignore = "run by tests/next/tokenizer/linkify_fuzz.py"]
fn answer_requests() {
    use std::io::{BufRead, Write};
    let (Ok(input), Ok(output)) = (
        std::env::var("LINKIFY_REQUESTS"),
        std::env::var("LINKIFY_ANSWERS"),
    ) else {
        return;
    };
    let reader = std::io::BufReader::new(std::fs::File::open(input).expect("requests"));
    let mut writer = std::io::BufWriter::new(std::fs::File::create(output).expect("answers"));
    let mut cache: std::collections::HashMap<String, LinkifyIt> = std::collections::HashMap::new();
    for line in reader.lines() {
        let line = line.expect("a line");
        if line.trim().is_empty() {
            continue;
        }
        let req: serde_json::Value = serde_json::from_str(&line).expect("a request");
        let options = req.get("options").cloned().unwrap_or(serde_json::json!({}));
        let l = cache
            .entry(options.to_string())
            .or_insert_with(|| LinkifyIt::with_options(options_of(&options)));
        let text = u(req["text"].as_str().expect("text"));
        let answer = serde_json::json!({
            "id": req["id"],
            "test": l.test(&text).unwrap(),
            "match": l.match_all(&text).unwrap().map(|ms| ms.iter().map(match_json).collect::<Vec<_>>()),
            "matchAtStart": l.match_at_start(&text).unwrap().as_ref().map(match_json),
        });
        writeln!(writer, "{answer}").expect("write");
    }
}

/// `tests/next/tokenizer/linkify_cases.jsonl` (written by linkify_golden.py from upstream's
/// dist): linkify-it's answers for hand-written edge cases (schemes in every case and
/// look-alike letters, hosts, IPs, ports, paths, e-mails, every option) and generated texts.
#[test]
fn the_regression_cases_answer_as_upstream() {
    let path = format!(
        "{}/tests/next/tokenizer/linkify_cases.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).expect("linkify_cases.jsonl");
    let mut cache: std::collections::HashMap<String, LinkifyIt> = std::collections::HashMap::new();
    let mut checked = 0;
    let mut failures = Vec::new();
    for line in text.split('\n').filter(|l| !l.trim().is_empty()) {
        let case: serde_json::Value = serde_json::from_str(line).expect("a case");
        let options = &case["options"];
        let l = cache
            .entry(options.to_string())
            .or_insert_with(|| LinkifyIt::with_options(options_of(options)));
        let t = u(case["text"].as_str().expect("text"));
        let answer = serde_json::json!({
            "test": l.test(&t).unwrap(),
            "match": l.match_all(&t).unwrap().map(|ms| ms.iter().map(match_json).collect::<Vec<_>>()),
            "matchAtStart": l.match_at_start(&t).unwrap().as_ref().map(match_json),
        });
        if answer != case["answer"] {
            failures.push(format!("{line}\n  port {answer}"));
        }
        checked += 1;
    }
    assert!(checked >= 400, "{checked} cases");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// `tests/next/tokenizer/linkify_classes.json`, written by `linkify_classes.cjs --table` from
/// V8's own regexes (the dist's REBuilder sources): what the character functions the
/// matchers are built on must answer, for every BMP code unit (lone surrogates included,
/// alone and where uc.micro's `Any` takes a low one together with the unit before it) and for
/// every astral code point, punctuation included.
#[test]
fn the_character_classes_are_v8s_for_every_code_point() {
    use rebuilder::{eq_ci, is_sep, is_zcc, pseudo_letter_len, zpcc_len};

    let path = format!(
        "{}/tests/next/tokenizer/linkify_classes.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).expect("linkify_classes.json");
    let table: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    // the units of the port's literals (schemes, TLDs, `localhost`, `xn--`, `IPv6:`)
    let mut pats: Vec<u16> =
        u("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789:/-._@");
    pats.extend([0x440, 0x444, 0x420, 0x424]);
    const A: u16 = 0x61;
    const LOW: u16 = 0xDC00;
    let bmp = |c: u16| -> (Vec<u64>, String) {
        let eq: Vec<u16> = pats.iter().copied().filter(|&p| eq_ci(c, p)).collect();
        let lens = [
            u64::from(is_sep(c)),
            u64::from(is_zcc(c)),
            zpcc_len(&[c], 0) as u64,
            pseudo_letter_len(&[c], 0) as u64,
            pseudo_letter_len(&[c, A], 0) as u64,
            pseudo_letter_len(&[c, LOW], 0) as u64,
            pseudo_letter_len(&[A, c], 1) as u64,
            pseudo_letter_len(&[A, c, LOW], 1) as u64,
        ];
        let eq = if eq.is_empty() {
            "-".to_owned()
        } else {
            js::to_string(&eq)
        };
        (lens.to_vec(), eq)
    };
    let astral = |cp: u32| -> Vec<u64> {
        let mut pair = [0u16; 2];
        char::from_u32(cp)
            .expect("a scalar value")
            .encode_utf16(&mut pair);
        let [hi, lo] = pair;
        vec![
            zpcc_len(&pair, 0) as u64,
            pseudo_letter_len(&pair, 0) as u64,
            pseudo_letter_len(&[hi, lo, A], 0) as u64,
            pseudo_letter_len(&[A, hi, lo], 1) as u64,
        ]
    };
    let runs = |key: &str| -> Vec<(u32, u32, Vec<serde_json::Value>)> {
        table[key]
            .as_array()
            .expect("runs")
            .iter()
            .map(|run| {
                let run = run.as_array().expect("a run");
                let bound = |i: usize| run[i].as_u64().expect("a bound") as u32;
                (bound(0), bound(1), run[2..].to_vec())
            })
            .collect()
    };
    let number = |v: &serde_json::Value| v.as_u64().expect("a length");

    let mut next = 0u32;
    for (start, end, want) in runs("bmp") {
        assert_eq!(start, next, "the BMP runs leave a gap");
        let (lens, eq) = (
            want[..8].iter().map(number).collect::<Vec<_>>(),
            want[8].as_str().expect("eq").to_owned(),
        );
        for c in start..=end {
            assert_eq!(bmp(c as u16), (lens.clone(), eq.clone()), "U+{c:04X}");
        }
        next = end + 1;
    }
    assert_eq!(next, 0x10000);
    for (start, end, want) in runs("astral") {
        assert_eq!(start, next, "the astral runs leave a gap");
        let want: Vec<u64> = want.iter().map(number).collect();
        for cp in start..=end {
            assert_eq!(astral(cp), want, "U+{cp:04X}");
        }
        next = end + 1;
    }
    assert_eq!(next, 0x110000);
}

#[test]
fn a_lone_low_surrogate_is_taken_the_first_way_only() {
    // Divergence (see the module docs): node matches "http://a\udc00" ([0, 9]) and
    // "http://a\udc00.com" ([0, 13]) by backtracking into uc.micro's fourth `Any`
    // alternative, which takes the lone low surrogate together with the `a` before it; the
    // port takes `a` as the first alternative does, and finds no host. Where that fourth
    // alternative is the first to match (a `.` before the lone low surrogate), both agree:
    // "http://a.\udc00" is [0, 10] in node and here.
    let l = LinkifyIt::new();
    let mut a_low = u("http://a");
    a_low.push(0xDC00);
    assert_eq!(l.match_all(&a_low).unwrap(), None);
    let mut a_low_com = a_low.clone();
    a_low_com.extend(u(".com"));
    assert_eq!(l.match_all(&a_low_com).unwrap(), None);
    let mut a_dot_low = u("http://a.");
    a_dot_low.push(0xDC00);
    let m = l.match_all(&a_dot_low).unwrap().expect("a link");
    assert_eq!((m[0].index, m[0].last_index), (0, 10));
}

#[test]
fn max_length_zero_throws_v8s_syntax_error_where_the_path_regex_is_built() {
    // node (linkify-it 6.1.0, work/utf16/lk1.cjs and linkify_max_length_zero.cjs): with
    // `maxLength: 0` linkify-it builds the path regex with `{1,0}`, which V8 rejects; every
    // call that needs it (a web or `//` schema's validator, the fuzzy link search) throws
    // `SyntaxError: Invalid regular expression: /(?:[/?#](?:...){1,0}|\/)?/: numbers out of
    // order in {} quantifier` (6,592 characters, 6,613 with `---`); e-mails and texts
    // without a schema answer as usual.
    for triple_dash in [false, true] {
        let l = LinkifyIt::with_options(LinkifyOptions {
            max_length: 0,
            triple_dash,
            ..LinkifyOptions::default()
        });
        let err = l.test(&u("see http://a.com/x")).unwrap_err();
        assert_eq!(err.name, "SyntaxError");
        assert_eq!(
            err.message.encode_utf16().count(),
            if triple_dash { 6613 } else { 6592 }
        );
        assert!(err.message.starts_with(
            "Invalid regular expression: /(?:[/?#](?:\\[(?:(?:(?![ \\xA0\\u1680\\u2000-"
        ));
        assert!(
            err.message
                .ends_with("\\uff5c]).){1,0}|\\/)?/: numbers out of order in {} quantifier")
        );
        assert_eq!(l.match_all(&u("see http://a.com/x")).unwrap_err(), err);
        assert_eq!(l.match_at_start(&u("http://a.com")).unwrap_err(), err);
        assert_eq!(
            l.test_schema_at(&u("http://a.com"), &u("http:"), 5)
                .unwrap_err(),
            err
        );
        assert_eq!(
            l.test_schema_at(&u("//a.com"), &u("//"), 2).unwrap_err(),
            err
        );
        assert_eq!(l.test(&u("hello world")), Ok(false));
        assert_eq!(l.test(&u("see a.com/x")), Ok(false));
        assert_eq!(l.test(&u("see a@b.com")), Ok(true));
        assert_eq!(l.match_all(&u("see a.com")), Ok(None));
        assert_eq!(l.match_at_start(&u("hello")), Ok(None));
        assert_eq!(l.match_all(&u("")), Ok(None));
        // the fuzzy link search builds it too: `match` throws for any text, `test` once no
        // schema link answered first (none can: `text.slice(0, pos + 0)` leaves no tail)
        let fuzzy = LinkifyIt::with_options(LinkifyOptions {
            max_length: 0,
            fuzzy_link: true,
            triple_dash,
            ..LinkifyOptions::default()
        });
        assert_eq!(fuzzy.match_all(&u("hello")).unwrap_err(), err);
        assert_eq!(fuzzy.test(&u("hello")).unwrap_err(), err);
        assert_eq!(fuzzy.test(&u("mailto:a@b.com")).unwrap_err(), err);
    }
}
