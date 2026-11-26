//! Deterministic rule generation shared by the lookup benchmark and the
//! memory measurement. The names are shaped like a large merged blocklist, not
//! like a counter: many registrable domains under a skewed spread of public
//! suffixes, a few with hundreds of subdomains, most with a handful, and
//! subdomain labels that mix common words, digits and random hex.

use std::iter;

use hickory_filter::rules::{Action, Matcher, Rule};
use regex::Regex;

/// Exact names in the set the benchmark and measurement use.
pub const EXACT_NAMES: usize = 1_000_000;
const WILDCARDS: usize = 5_000;
const REGEXES: usize = 50;
const ALLOWS: usize = 100;
/// Wildcard bases come from registrable domains numbered from here, so they
/// never collide with the exact names' domains.
const WILDCARD_DOMAIN_BASE: u64 = 10_000_000;

/// (public suffix, weight): a skewed spread, mostly `com`.
const SUFFIXES: [(&str, u64); 20] = [
    ("com", 450),
    ("net", 80),
    ("org", 60),
    ("ru", 50),
    ("io", 40),
    ("cn", 40),
    ("co.uk", 30),
    ("de", 30),
    ("info", 25),
    ("xyz", 25),
    ("top", 25),
    ("com.br", 25),
    ("site", 20),
    ("online", 20),
    ("biz", 15),
    ("club", 15),
    ("fr", 15),
    ("nl", 15),
    ("app", 10),
    ("dev", 10),
];

const WORDS: [&str; 16] = [
    "www", "cdn", "ads", "track", "api", "static", "img", "m", "mail", "pixel", "metrics", "edge",
    "assets", "s", "t", "ssl",
];

/// SplitMix64: a small deterministic generator, so runs are reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

fn random_label(rng: &mut Rng, charset: &[u8], min: u64, max: u64) -> String {
    let len = min + rng.below(max - min + 1);
    (0..len)
        .map(|_| char::from(charset[rng.below(charset.len() as u64) as usize]))
        .collect()
}

const ALNUM: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const HEX: &[u8] = b"0123456789abcdef";

/// The registrable domain with this number, such as `k3x9fq2.com`.
fn registrable_domain(id: u64) -> String {
    let mut rng = Rng(id);
    let mut label = random_label(&mut rng, ALNUM, 4, 14);
    if label.len() > 6 && rng.below(10) == 0 {
        label.insert(label.len() / 2, '-');
    }
    let total: u64 = SUFFIXES.iter().map(|(_, w)| w).sum();
    let mut roll = rng.below(total);
    let suffix = SUFFIXES
        .iter()
        .find(|(_, weight)| {
            if roll < *weight {
                true
            } else {
                roll -= weight;
                false
            }
        })
        .map_or("com", |(suffix, _)| suffix);
    format!("{label}.{suffix}")
}

/// A random subdomain part, one or two labels, such as `cdn12` or `a1b2c3.ads`.
fn random_subdomain(rng: &mut Rng) -> String {
    match rng.below(4) {
        0 => (*rng.pick(&WORDS)).to_owned(),
        1 => random_label(rng, HEX, 6, 8),
        2 => format!("{}{}", rng.pick(&WORDS), rng.below(100)),
        _ => format!("{}.{}", rng.pick(&WORDS), random_label(rng, HEX, 4, 6)),
    }
}

/// The names of one registrable domain: the apex, then a skewed number of
/// subdomains (a few domains have hundreds, most have a handful).
fn names_of_domain(id: u64) -> Vec<String> {
    let domain = registrable_domain(id);
    let mut rng = Rng(id ^ 0xD0_4A11);
    let subdomains = match rng.below(1000) {
        0..=9 => 50 + rng.below(350),
        10..=149 => 10 + rng.below(30),
        _ => rng.below(6),
    };
    let mut names = vec![domain.clone()];
    names.extend((0..subdomains).map(|_| format!("{}.{domain}", random_subdomain(&mut rng))));
    names
}

/// `total` exact names, domain after domain, produced lazily.
pub fn exact_names(total: usize) -> impl Iterator<Item = String> {
    let mut domain = 0u64;
    let mut queue: Vec<String> = Vec::new();
    iter::from_fn(move || {
        while queue.is_empty() {
            queue = names_of_domain(domain);
            queue.reverse();
            domain += 1;
        }
        queue.pop()
    })
    .take(total)
}

/// The `i`th exact name, for picking a known hit.
pub fn sample_exact(i: usize) -> String {
    exact_names(usize::MAX).nth(i).expect("endless iterator")
}

/// The base name of the `i`th wildcard rule.
pub fn wildcard_base(i: usize) -> String {
    registrable_domain(WILDCARD_DOMAIN_BASE + i as u64)
}

fn rule(action: Action, matcher: Matcher) -> Rule {
    Rule { action, matcher }
}

/// `exact` block domains, plus wildcards, regexes and allow rules, produced
/// lazily so a measurement doesn't also hold the input in memory.
pub fn rules(exact: usize) -> impl Iterator<Item = Rule> {
    let domains = exact_names(exact).map(|name| rule(Action::Block, Matcher::Domain(name)));
    let wildcards =
        (0..WILDCARDS).map(|i| rule(Action::Block, Matcher::Wildcard(wildcard_base(i))));
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
