//! Deterministic rule generation shared by the lookup benchmark and the
//! memory measurement: a large exact-name set plus wildcards, regexes and
//! allow rules, shaped like a big merged blocklist.

use hickory_filter::rules::{Action, Matcher, Rule};
use regex::Regex;

/// Names in the exact set the benchmark and measurement use.
pub const EXACT_NAMES: usize = 1_000_000;
const WILDCARDS: usize = 5_000;
const REGEXES: usize = 50;
const ALLOWS: usize = 100;

/// The `i`th generated exact name.
pub fn exact_name(i: usize) -> String {
    format!("host{i}.zone{}.example", i % 997)
}

fn rule(action: Action, matcher: Matcher) -> Rule {
    Rule { action, matcher }
}

/// `exact` block domains, plus wildcards, regexes and allow rules, produced
/// lazily so a measurement doesn't also hold the input in memory.
pub fn rules(exact: usize) -> impl Iterator<Item = Rule> {
    let domains = (0..exact).map(|i| rule(Action::Block, Matcher::Domain(exact_name(i))));
    let wildcards =
        (0..WILDCARDS).map(|i| rule(Action::Block, Matcher::Wildcard(format!("wild{i}.example"))));
    let regexes = (0..REGEXES).map(|i| {
        let pattern = format!("^t{i}x[0-9]+\\.");
        rule(
            Action::Block,
            Matcher::Regex(Regex::new(&pattern).expect("valid pattern")),
        )
    });
    let allows = (0..ALLOWS).map(|i| {
        rule(
            Action::Allow,
            Matcher::Domain(format!("ok{i}.allowed.example")),
        )
    });
    domains.chain(wildcards).chain(regexes).chain(allows)
}
