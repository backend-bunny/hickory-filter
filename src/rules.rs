//! Rule lines: parsing of inline rules and of list files.
//!
//! One syntax serves both places. An inline rule is a line,
//! `<action> <type> <value>`; a list is declared with
//! `<action> list <type> <url>` and its file is read one rule per line in the
//! list's type. Matching lives elsewhere; this module only turns text into
//! rules and counts what it had to skip.

use std::{fmt, net::IpAddr};

use regex::Regex;

/// What a matching rule does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Allow,
    Block,
}

/// What a rule matches.
#[derive(Debug, Clone)]
pub enum Matcher {
    /// Exactly this normalised name.
    Domain(String),
    /// This normalised base name and every name below it.
    Wildcard(String),
    /// A pattern matched anywhere in the normalised name.
    Regex(Regex),
}

impl PartialEq for Matcher {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Domain(a), Self::Domain(b)) | (Self::Wildcard(a), Self::Wildcard(b)) => a == b,
            (Self::Regex(a), Self::Regex(b)) => a.as_str() == b.as_str(),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub action: Action,
    pub matcher: Matcher,
}

/// The format of a list file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListType {
    Domain,
    Hosts,
    Wildcard,
    Regex,
}

/// A list declared as `<action> list <type> <url>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListDecl {
    pub action: Action,
    pub list_type: ListType,
    pub url: String,
}

/// One parsed inline rule line.
#[derive(Debug, Clone, PartialEq)]
pub enum RuleLine {
    Rule(Rule),
    List(ListDecl),
}

/// An invalid rule line. Names the rule and says why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleError {
    pub rule: String,
    pub reason: String,
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rule `{}`: {}", self.rule, self.reason)
    }
}

impl std::error::Error for RuleError {}

/// Normalise a DNS name for matching: ASCII lower case, one trailing dot
/// ignored. Returns `None` when it isn't a usable name: empty, too long, an
/// empty or over-long label, a label starting or ending with `-`, a character
/// outside letters, digits, `-` and `_`, or an IP address literal.
pub fn normalize_name(name: &str) -> Option<String> {
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.is_empty() || name.len() > 253 {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    if lower.parse::<IpAddr>().is_ok() {
        return None;
    }
    for label in lower.split('.') {
        if label.is_empty() || label.len() > 63 || label.starts_with('-') || label.ends_with('-') {
            return None;
        }
        let valid = label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
        if !valid {
            return None;
        }
    }
    Some(lower)
}

/// The base name of a wildcard value, in any accepted spelling: `x`, `*.x`,
/// `.x`, `||x^`, or `address=/x/` followed by anything. A `||x^` line with
/// anything after the `^` (such as a `$option` suffix) is not accepted.
pub fn parse_wildcard(value: &str) -> Option<String> {
    let value = value.trim();
    let base = if let Some(rest) = value.strip_prefix("||") {
        rest.strip_suffix('^')?
    } else if let Some(rest) = value.strip_prefix("address=/") {
        rest.split_once('/')?.0
    } else if let Some(rest) = value.strip_prefix("*.") {
        rest
    } else if let Some(rest) = value.strip_prefix('.') {
        rest
    } else {
        value
    };
    normalize_name(base)
}

fn split_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim_start()),
        None => (s, ""),
    }
}

fn parse_action(word: &str) -> Option<Action> {
    match word {
        "allow" => Some(Action::Allow),
        "block" => Some(Action::Block),
        _ => None,
    }
}

fn parse_list_type(word: &str) -> Option<ListType> {
    match word {
        "domain" => Some(ListType::Domain),
        "hosts" => Some(ListType::Hosts),
        "wildcard" => Some(ListType::Wildcard),
        "regex" => Some(ListType::Regex),
        _ => None,
    }
}

fn validate_list_url(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("missing URL".to_owned());
    }
    if url.contains(char::is_whitespace) {
        return Err("the URL must not contain whitespace".to_owned());
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(format!(
            "`{url}` is not a URL; use a `file://`, `http://` or `https://` URL"
        ));
    };
    match scheme.to_ascii_lowercase().as_str() {
        "file" => Ok(()),
        "http" | "https" => {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
            let host = authority.rsplit('@').next().unwrap_or("");
            if host.is_empty() {
                Err(format!("`{url}` has no host"))
            } else {
                Ok(())
            }
        }
        other => Err(format!(
            "unsupported URL scheme `{other}`; use `file`, `http` or `https`"
        )),
    }
}

/// Parse one inline rule line: `<action> <domain|wildcard|regex> <value>`, or
/// a list declaration `<action> list <type> <url>`.
pub fn parse_rule_line(line: &str) -> Result<RuleLine, RuleError> {
    let err = |reason: String| RuleError {
        rule: line.to_owned(),
        reason,
    };
    let (action_word, rest) = split_word(line);
    if action_word.is_empty() {
        return Err(err(
            "empty rule; expected `<action> <type> <value>`".to_owned()
        ));
    }
    let Some(action) = parse_action(action_word) else {
        return Err(err(format!(
            "unknown action `{action_word}`; expected `allow` or `block`"
        )));
    };
    let (type_word, value) = split_word(rest);
    let value = value.trim_end();
    if type_word.is_empty() {
        return Err(err(
            "missing type and value; expected `<action> <type> <value>`".to_owned(),
        ));
    }
    if type_word == "list" {
        let (list_type_word, url) = split_word(value);
        let Some(list_type) = parse_list_type(list_type_word) else {
            return Err(err(format!(
                "unknown list type `{list_type_word}`; accepted list types are `domain`, `hosts`, `wildcard`, `regex`"
            )));
        };
        validate_list_url(url).map_err(err)?;
        return Ok(RuleLine::List(ListDecl {
            action,
            list_type,
            url: url.to_owned(),
        }));
    }
    if !matches!(type_word, "domain" | "wildcard" | "regex") {
        return Err(err(format!(
            "unknown type `{type_word}`; accepted types are `domain`, `wildcard`, `regex` (or `list <type> <url>`)"
        )));
    }
    if value.is_empty() {
        return Err(err(format!("missing value after `{type_word}`")));
    }
    let matcher = match type_word {
        "domain" => Matcher::Domain(
            normalize_name(value)
                .ok_or_else(|| err(format!("`{value}` is not a valid DNS name")))?,
        ),
        "wildcard" => Matcher::Wildcard(parse_wildcard(value).ok_or_else(|| {
            err(format!(
                "`{value}` is not a valid wildcard; its base name must be a valid DNS name (accepted spellings: `x`, `*.x`, `.x`, `||x^`, `address=/x/…`)"
            ))
        })?),
        _ => Matcher::Regex(
            Regex::new(value).map_err(|e| err(format!("invalid regular expression: {e}")))?,
        ),
    };
    Ok(RuleLine::Rule(Rule { action, matcher }))
}

/// What one line of a list file turned out to be.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Rule(Rule),
    /// A blank line or a comment.
    Ignored,
    /// A line that isn't valid for the list's type.
    Skipped,
}

/// The rules of a list, and how many lines were skipped as invalid.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParsedList {
    pub rules: Vec<Rule>,
    pub skipped: usize,
}

/// Remove a comment. `#` starts one anywhere, except in a regex list, where a
/// `#` only counts at the start of the line or after whitespace (so a pattern
/// containing `#` isn't cut into a different, valid pattern). In a wildcard
/// list a line starting with `!` is a comment.
fn strip_comment(list_type: ListType, line: &str) -> &str {
    match list_type {
        ListType::Wildcard if line.trim_start().starts_with('!') => "",
        ListType::Regex => {
            let mut prev_is_space = true;
            for (i, c) in line.char_indices() {
                if c == '#' && prev_is_space {
                    return &line[..i];
                }
                prev_is_space = c.is_whitespace();
            }
            line
        }
        _ => line.split('#').next().unwrap_or(""),
    }
}

/// Parse one line of a list in `list_type`. Every produced rule has `action`.
pub fn parse_list_line(list_type: ListType, action: Action, raw: &str) -> Line {
    let line = strip_comment(list_type, raw).trim();
    if line.is_empty() {
        return Line::Ignored;
    }
    let matcher = match list_type {
        ListType::Domain => normalize_name(line).map(Matcher::Domain),
        ListType::Hosts => {
            let mut fields = line.split_whitespace();
            match (fields.next(), fields.next(), fields.next()) {
                (Some(address), Some(name), None) if address.parse::<IpAddr>().is_ok() => {
                    normalize_name(name).map(Matcher::Domain)
                }
                _ => None,
            }
        }
        ListType::Wildcard => parse_wildcard(line).map(Matcher::Wildcard),
        ListType::Regex => Regex::new(line).ok().map(Matcher::Regex),
    };
    match matcher {
        Some(matcher) => Line::Rule(Rule { action, matcher }),
        None => Line::Skipped,
    }
}

/// Parse a whole list file. Invalid lines are skipped and counted; they never
/// fail the list.
pub fn parse_list(list_type: ListType, action: Action, text: &str) -> ParsedList {
    let mut parsed = ParsedList::default();
    for raw in text.lines() {
        match parse_list_line(list_type, action, raw) {
            Line::Rule(rule) => parsed.rules.push(rule),
            Line::Ignored => {}
            Line::Skipped => parsed.skipped += 1,
        }
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain(name: &str) -> Matcher {
        Matcher::Domain(name.to_owned())
    }

    fn wildcard(name: &str) -> Matcher {
        Matcher::Wildcard(name.to_owned())
    }

    fn inline(line: &str) -> Result<RuleLine, RuleError> {
        parse_rule_line(line)
    }

    fn inline_rule(line: &str) -> Rule {
        match inline(line).expect(line) {
            RuleLine::Rule(rule) => rule,
            other => panic!("{line}: expected a rule, got {other:?}"),
        }
    }

    // --- 3.1: inline rule lines and name normalisation ---

    #[test]
    fn inline_rule_accepted() {
        assert_eq!(
            inline_rule("block domain ads.example"),
            Rule {
                action: Action::Block,
                matcher: domain("ads.example")
            }
        );
        assert_eq!(
            inline_rule("allow wildcard *.ok.example").action,
            Action::Allow
        );
    }

    #[test]
    fn unknown_type_is_rejected_naming_the_rule_and_the_accepted_types() {
        let err = inline("block glob *.example").expect_err("unknown type");
        let text = err.to_string();
        assert!(text.contains("block glob *.example"), "{text}");
        assert!(text.contains("domain"), "{text}");
        assert!(text.contains("wildcard"), "{text}");
        assert!(text.contains("regex"), "{text}");
    }

    #[test]
    fn unknown_action_and_missing_parts_are_rejected() {
        for (line, needle) in [
            ("deny domain ads.example", "unknown action `deny`"),
            ("", "empty rule"),
            ("block", "missing type"),
            ("block domain", "missing value"),
            ("block wildcard   ", "missing value"),
            ("block domain not a name", "not a valid DNS name"),
        ] {
            let text = inline(line).expect_err(line).to_string();
            assert!(text.contains(needle), "{line}: {text}");
            assert!(text.contains(&format!("`{line}`")), "{line}: {text}");
        }
    }

    #[test]
    fn names_are_case_insensitive_and_a_trailing_dot_is_ignored() {
        assert_eq!(
            inline_rule("block domain Ads.Example.").matcher,
            domain("ads.example")
        );
        assert_eq!(
            normalize_name("ADS.example"),
            Some("ads.example".to_owned())
        );
        assert_eq!(
            normalize_name("ads.example."),
            Some("ads.example".to_owned())
        );
    }

    #[test]
    fn name_validation() {
        for ok in [
            "a",
            "a.b",
            "xn--bcher-kva.example",
            "_dmarc.example",
            "a-b.example",
            "1.example",
        ] {
            assert!(normalize_name(ok).is_some(), "{ok}");
        }
        let long_label = "a".repeat(64);
        let long_name = format!("{}.example", "a.".repeat(130));
        for bad in [
            "",
            ".",
            "a..b",
            "..",
            "-a.example",
            "a-.example",
            "exa mple",
            "exa_mple!",
            "bücher.example",
            "0.0.0.0",
            "::1",
            "*.example",
            long_label.as_str(),
            long_name.as_str(),
        ] {
            assert!(normalize_name(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn list_declarations() {
        assert_eq!(
            inline("block list hosts https://lists.example/hosts.txt").expect("valid"),
            RuleLine::List(ListDecl {
                action: Action::Block,
                list_type: ListType::Hosts,
                url: "https://lists.example/hosts.txt".to_owned()
            })
        );
        assert!(inline("allow list domain file:///etc/hickory-filter/allow.txt").is_ok());
        for (line, needle) in [
            (
                "block list hosts ftp://lists.example/a.txt",
                "unsupported URL scheme",
            ),
            ("block list hosts https:///a.txt", "no host"),
            ("block list hosts lists.example/a.txt", "not a URL"),
            ("block list hosts", "missing URL"),
            (
                "block list glob https://lists.example/a.txt",
                "unknown list type",
            ),
            (
                "block list hosts https://lists.example/a b.txt",
                "whitespace",
            ),
        ] {
            let text = inline(line).expect_err(line).to_string();
            assert!(text.contains(needle), "{line}: {text}");
        }
    }

    // --- 3.2: wildcard spellings ---

    #[test]
    fn every_wildcard_spelling_means_the_same_base_name() {
        for spelling in [
            "ads.example",
            "*.ads.example",
            ".ads.example",
            "||ads.example^",
            "address=/ads.example/#",
            "address=/ads.example/0.0.0.0",
            "address=/ads.example/",
            "address=/ads.example/::",
            "ADS.Example.",
            "||Ads.Example.^",
            "  ||ads.example^  ",
        ] {
            assert_eq!(
                parse_wildcard(spelling),
                Some("ads.example".to_owned()),
                "{spelling}"
            );
            let rule = inline_rule(&format!("block wildcard {spelling}"));
            assert_eq!(rule.matcher, wildcard("ads.example"), "{spelling}");
        }
    }

    #[test]
    fn invalid_wildcards_are_rejected() {
        for bad in [
            "||exa mple^",
            "||ads.example",
            "||ads.example^$third-party",
            "||ads.example^$important",
            "||^",
            "address=/ads.example",
            "address=//0.0.0.0",
            "*.",
            "*",
            "**.example",
            "a*.example",
            "ads.example/path",
            "http://ads.example",
            "",
        ] {
            assert_eq!(parse_wildcard(bad), None, "{bad:?}");
        }
        let text = inline("block wildcard ||exa mple^")
            .expect_err("invalid")
            .to_string();
        assert!(text.contains("||exa mple^"), "{text}");
    }

    // --- 3.3: list line syntax, comments, skipping ---

    fn load(list_type: ListType, text: &str) -> ParsedList {
        parse_list(list_type, Action::Block, text)
    }

    fn matchers(parsed: &ParsedList) -> Vec<Matcher> {
        parsed.rules.iter().map(|r| r.matcher.clone()).collect()
    }

    #[test]
    fn hosts_lines_accept_any_address_and_any_whitespace() {
        let parsed = load(
            ListType::Hosts,
            "0.0.0.0 a.example\n127.0.0.1\tb.example\n::  c.example\n  2001:db8::1 \t d.example  \n",
        );
        assert_eq!(parsed.skipped, 0);
        assert_eq!(
            matchers(&parsed),
            vec![
                domain("a.example"),
                domain("b.example"),
                domain("c.example"),
                domain("d.example")
            ]
        );
        // The hosts rule is an exact `domain` rule: x.a.example is a different name.
        assert_ne!(matchers(&parsed)[0], domain("x.a.example"));
    }

    #[test]
    fn invalid_hosts_lines_are_skipped_and_counted() {
        let parsed = load(
            ListType::Hosts,
            "a.example\nnot-an-ip b.example\n0.0.0.0 c.example d.example\n0.0.0.0\n0.0.0.0 e.example\n0.0.0.0 0.0.0.0\n",
        );
        assert_eq!(matchers(&parsed), vec![domain("e.example")]);
        assert_eq!(parsed.skipped, 5);
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let parsed = load(
            ListType::Domain,
            "# header\n\nads.example # inline comment\n   \n\t# indented\n",
        );
        assert_eq!(matchers(&parsed), vec![domain("ads.example")]);
        assert_eq!(parsed.skipped, 0);
    }

    #[test]
    fn domain_lines_with_more_than_a_name_are_skipped() {
        let parsed = load(
            ListType::Domain,
            "a.example b.example\nbad name\nok.example\n",
        );
        assert_eq!(matchers(&parsed), vec![domain("ok.example")]);
        assert_eq!(parsed.skipped, 2);
    }

    #[test]
    fn wildcard_lists_skip_option_suffixes_and_treat_bang_as_a_comment() {
        let parsed = load(
            ListType::Wildcard,
            "! Title: some list\n||ads.example^$third-party\n||track.example^\n*.cdn.example\naddress=/dns.example/#\n||x.example^$important,third-party\n",
        );
        assert_eq!(
            matchers(&parsed),
            vec![
                wildcard("track.example"),
                wildcard("cdn.example"),
                wildcard("dns.example")
            ]
        );
        assert_eq!(parsed.skipped, 2);
    }

    #[test]
    fn invalid_regex_lines_are_skipped_the_list_survives() {
        let parsed = load(
            ListType::Regex,
            "(unclosed\n^ad\\.\n(?=lookahead)x\n(a)\\1\n",
        );
        assert_eq!(parsed.rules.len(), 1);
        match &parsed.rules[0].matcher {
            Matcher::Regex(re) => assert_eq!(re.as_str(), "^ad\\."),
            other => panic!("{other:?}"),
        }
        // Unclosed group, look-around and a backreference are all refused.
        assert_eq!(parsed.skipped, 3);
    }

    #[test]
    fn regex_lines_keep_a_hash_inside_the_pattern() {
        // `#` starts a comment only at the start or after whitespace, so a
        // pattern with a `#` in it is not cut into a different pattern.
        let parsed = load(ListType::Regex, "a#b\n^ad\\. # a comment\n# whole line\n");
        let sources: Vec<String> = parsed
            .rules
            .iter()
            .map(|r| match &r.matcher {
                Matcher::Regex(re) => re.as_str().to_owned(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(sources, vec!["a#b".to_owned(), "^ad\\.".to_owned()]);
        assert_eq!(parsed.skipped, 0);
    }

    #[test]
    fn a_list_never_fails_as_a_whole_and_keeps_its_action() {
        let text = "!!!\n\u{0}\u{1}garbage\n(((\n";
        for list_type in [
            ListType::Domain,
            ListType::Hosts,
            ListType::Wildcard,
            ListType::Regex,
        ] {
            let parsed = parse_list(list_type, Action::Allow, text);
            assert!(parsed.rules.iter().all(|r| r.action == Action::Allow));
        }
        let parsed = parse_list(ListType::Domain, Action::Allow, "ok.example\n");
        assert_eq!(parsed.rules[0].action, Action::Allow);
    }

    #[test]
    fn inline_regex() {
        assert!(matches!(
            inline_rule(r"block regex ^ad[0-9]+\.").matcher,
            Matcher::Regex(_)
        ));
        let text = inline("block regex (unclosed")
            .expect_err("invalid")
            .to_string();
        assert!(text.contains("block regex (unclosed"), "{text}");
        assert!(text.contains("regular expression"), "{text}");
    }
}
