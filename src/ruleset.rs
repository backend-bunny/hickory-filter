//! The rule set: what decides whether a queried name is allowed or blocked.
//!
//! A `RuleSet` is immutable and built whole from rules. The live one sits in a
//! `LiveRules`, which a refresh replaces with a single atomic pointer swap, so
//! a query sees either the complete old set or the complete new one, and the
//! query path takes no lock and does no I/O.
//!
//! Matching order: allow before block, and within each side exact name, then
//! suffix (wildcard), then regex. Allow always wins. Exact and wildcard names
//! live in finite-state transducers keyed by the name's labels in reverse
//! order (`ads.example` is stored as `example.ads`), which keeps the
//! footprint small and makes a lookup one walk over the automaton.

use std::{
    borrow::{Borrow, Cow},
    fmt,
    sync::Arc,
};

use arc_swap::{ArcSwap, Guard};
use fst::raw::{Fst, Node};
use regex::RegexSet;

use crate::rules::{Action, Matcher, Rule};

/// What a rule set says about a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// An allow rule matched: forward it, whatever block rules also match.
    Allow,
    /// A block rule matched and no allow rule did.
    Block,
    /// No rule matched: handled as if no filter existed.
    Pass,
}

impl Verdict {
    pub fn is_blocked(self) -> bool {
        self == Self::Block
    }
}

/// Why a rule set could not be built.
#[derive(Debug)]
pub enum BuildError {
    /// The regular expressions together are too large to compile into one set.
    Regex(regex::Error),
    /// The name index could not be built.
    Names(fst::Error),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Regex(e) => write!(
                f,
                "the regular expressions are too large to compile together: {e}"
            ),
            Self::Names(e) => write!(f, "cannot build the name index: {e}"),
        }
    }
}

impl std::error::Error for BuildError {}

impl From<regex::Error> for BuildError {
    fn from(e: regex::Error) -> Self {
        Self::Regex(e)
    }
}

impl From<fst::Error> for BuildError {
    fn from(e: fst::Error) -> Self {
        Self::Names(e)
    }
}

/// Name lookups behind a small interface, so the storage can change without
/// touching parsing, refresh or the handler. Names are already normalised.
pub trait NameIndex {
    /// Is exactly this name in the index?
    fn contains_exact(&self, name: &str) -> bool;
    /// Is this name, or any parent of it at a label boundary, a wildcard base?
    fn matches_suffix(&self, name: &str) -> bool;
}

/// Names in two automata, exact names and wildcard bases, keyed by labels in
/// reverse order.
pub struct FstNames {
    exact: Fst<Vec<u8>>,
    suffix: Fst<Vec<u8>>,
}

impl FstNames {
    /// Build from normalised names in their ordinary order (`ads.example`).
    /// Duplicates are fine.
    pub fn build<E, S>(
        exact: impl IntoIterator<Item = E>,
        suffix: impl IntoIterator<Item = S>,
    ) -> Result<Self, fst::Error>
    where
        E: AsRef<str>,
        S: AsRef<str>,
    {
        let mut exact_keys = KeyArena::default();
        exact
            .into_iter()
            .for_each(|name| exact_keys.add(name.as_ref()));
        let mut suffix_keys = KeyArena::default();
        suffix
            .into_iter()
            .for_each(|name| suffix_keys.add(name.as_ref()));
        Self::from_keys(exact_keys, suffix_keys)
    }

    fn from_keys(exact: KeyArena, suffix: KeyArena) -> Result<Self, fst::Error> {
        Ok(Self {
            exact: exact.into_automaton()?,
            suffix: suffix.into_automaton()?,
        })
    }

    /// Bytes held by the two automata.
    pub fn size_bytes(&self) -> usize {
        self.exact.size() + self.suffix.size()
    }
}

impl Default for FstNames {
    fn default() -> Self {
        Self::build(Vec::<String>::new(), Vec::<String>::new())
            .expect("an empty name index always builds")
    }
}

/// Keys under construction: every name with its labels in reverse order
/// (`ads.example` becomes `example.ads`), packed into one buffer. Collecting
/// them this way costs two large allocations instead of one small allocation
/// per name, so a big build neither fragments the heap nor leaves freed
/// memory behind that the allocator keeps.
#[derive(Default)]
struct KeyArena {
    bytes: Vec<u8>,
    /// (start, length) of each key in `bytes`.
    ranges: Vec<(usize, usize)>,
}

impl KeyArena {
    fn add(&mut self, name: &str) {
        let start = self.bytes.len();
        for (position, label) in name.rsplit('.').enumerate() {
            if position > 0 {
                self.bytes.push(b'.');
            }
            self.bytes.extend_from_slice(label.as_bytes());
        }
        self.ranges.push((start, self.bytes.len() - start));
    }

    /// Sort, drop duplicates and build the automaton, which needs its keys
    /// sorted and unique.
    fn into_automaton(mut self) -> Result<Fst<Vec<u8>>, fst::Error> {
        let bytes = &self.bytes;
        let key = |&(start, len): &(usize, usize)| &bytes[start..start + len];
        self.ranges.sort_unstable_by(|a, b| key(a).cmp(key(b)));
        self.ranges.dedup_by(|a, b| key(a) == key(b));
        Fst::from_iter_set(self.ranges.iter().map(key))
    }
}

/// Follow `byte` out of `node`, if there is such a transition.
fn step<'f>(fst: &'f Fst<Vec<u8>>, node: &Node<'f>, byte: u8) -> Option<Node<'f>> {
    let index = node.find_input(byte)?;
    Some(fst.node(node.transition_addr(index)))
}

/// Walk the name's labels from the TLD down, without allocating. With
/// `stop_at_bases`, succeed as soon as a stored key ends at a label boundary
/// of the name; otherwise only a key equal to the whole name matches.
fn walk(fst: &Fst<Vec<u8>>, name: &str, stop_at_bases: bool) -> bool {
    let mut node = fst.root();
    for (position, label) in name.rsplit('.').enumerate() {
        if position > 0 {
            // A label boundary: a base ending exactly here covers the name.
            if stop_at_bases && node.is_final() {
                return true;
            }
            match step(fst, &node, b'.') {
                Some(next) => node = next,
                None => return false,
            }
        }
        for &byte in label.as_bytes() {
            match step(fst, &node, byte) {
                Some(next) => node = next,
                None => return false,
            }
        }
    }
    node.is_final()
}

impl NameIndex for FstNames {
    fn contains_exact(&self, name: &str) -> bool {
        walk(&self.exact, name, false)
    }

    fn matches_suffix(&self, name: &str) -> bool {
        walk(&self.suffix, name, true)
    }
}

/// The allow side or the block side of a rule set.
#[derive(Default)]
struct Side {
    names: FstNames,
    regex: RegexSet,
}

impl Side {
    fn matches(&self, name: &str) -> bool {
        self.names.contains_exact(name)
            || self.names.matches_suffix(name)
            || self.regex.is_match(name)
    }
}

#[derive(Default)]
struct SideBuilder {
    exact: KeyArena,
    suffix: KeyArena,
    patterns: Vec<String>,
}

impl SideBuilder {
    fn add(&mut self, matcher: &Matcher) {
        match matcher {
            Matcher::Domain(name) => self.exact.add(name),
            Matcher::Wildcard(base) => self.suffix.add(base),
            Matcher::Regex(regex) => self.patterns.push(regex.as_str().to_owned()),
        }
    }

    fn finish(self) -> Result<Side, BuildError> {
        Ok(Side {
            names: FstNames::from_keys(self.exact, self.suffix)?,
            regex: RegexSet::new(&self.patterns)?,
        })
    }
}

/// An immutable set of allow and block rules.
#[derive(Default)]
pub struct RuleSet {
    allow: Side,
    block: Side,
}

impl RuleSet {
    /// Build a set from rules, owned or borrowed.
    pub fn from_rules<R: Borrow<Rule>>(
        rules: impl IntoIterator<Item = R>,
    ) -> Result<Self, BuildError> {
        let mut allow = SideBuilder::default();
        let mut block = SideBuilder::default();
        for rule in rules {
            let rule = rule.borrow();
            match rule.action {
                Action::Allow => allow.add(&rule.matcher),
                Action::Block => block.add(&rule.matcher),
            }
        }
        Ok(Self {
            allow: allow.finish()?,
            block: block.finish()?,
        })
    }

    /// Decide a queried name. The name is normalised here: ASCII lower case,
    /// one trailing dot ignored.
    pub fn verdict(&self, qname: &str) -> Verdict {
        let name = normalize_query(qname);
        if name.is_empty() {
            return Verdict::Pass;
        }
        if self.allow.matches(&name) {
            Verdict::Allow
        } else if self.block.matches(&name) {
            Verdict::Block
        } else {
            Verdict::Pass
        }
    }

    pub fn is_blocked(&self, qname: &str) -> bool {
        self.verdict(qname).is_blocked()
    }

    /// Bytes held by the exact and wildcard name automata (allow and block).
    /// The compiled regular expressions are not included.
    pub fn name_index_bytes(&self) -> usize {
        self.allow.names.size_bytes() + self.block.names.size_bytes()
    }
}

/// Normalise a queried name without rejecting anything: queries can carry
/// names the rule parser would refuse, and they must still match by case.
fn normalize_query(name: &str) -> Cow<'_, str> {
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(name.to_ascii_lowercase())
    } else {
        Cow::Borrowed(name)
    }
}

/// The live rule set. Readers load it without locking; a refresh swaps in a
/// completely built replacement.
pub struct LiveRules {
    current: ArcSwap<RuleSet>,
}

impl LiveRules {
    pub fn new(initial: RuleSet) -> Self {
        Self {
            current: ArcSwap::from_pointee(initial),
        }
    }

    /// The current set. A query loads once and uses that one set throughout.
    pub fn load(&self) -> Guard<Arc<RuleSet>> {
        self.current.load()
    }

    /// Replace the set atomically.
    pub fn swap(&self, new: RuleSet) {
        self.current.store(Arc::new(new));
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        thread,
    };

    use super::*;
    use crate::rules::{RuleLine, parse_rule_line};

    fn rule(line: &str) -> Rule {
        match parse_rule_line(line).expect(line) {
            RuleLine::Rule(rule) => rule,
            other => panic!("{line}: {other:?}"),
        }
    }

    fn set(lines: &[&str]) -> RuleSet {
        RuleSet::from_rules(lines.iter().map(|l| rule(l))).expect("build")
    }

    #[test]
    fn domain_rule_blocks_the_exact_name_only() {
        let rules = set(&["block domain ads.example"]);
        assert!(rules.is_blocked("ads.example"));
        assert!(!rules.is_blocked("x.ads.example"));
        assert!(!rules.is_blocked("example"));
    }

    #[test]
    fn names_match_case_insensitively_with_a_trailing_dot_ignored() {
        let rules = set(&["block domain Ads.Example."]);
        assert!(rules.is_blocked("ADS.example"));
        assert!(rules.is_blocked("ads.example."));
        assert!(rules.is_blocked("Ads.Example"));
    }

    #[test]
    fn wildcard_blocks_the_base_and_every_name_below_it() {
        let rules = set(&["block wildcard *.ads.example"]);
        assert!(rules.is_blocked("ads.example"));
        assert!(rules.is_blocked("x.ads.example"));
        assert!(rules.is_blocked("a.b.ads.example"));
    }

    #[test]
    fn wildcard_matches_at_label_boundaries_only() {
        let rules = set(&["block wildcard *.ads.example"]);
        assert!(!rules.is_blocked("badads.example"));
        assert!(!rules.is_blocked("ads.example.org"));
        assert!(!rules.is_blocked("example"));
    }

    #[test]
    fn every_wildcard_spelling_blocks_the_same_names() {
        for spelling in [
            "ads.example",
            "*.ads.example",
            ".ads.example",
            "||ads.example^",
            "address=/ads.example/#",
        ] {
            let rules = set(&[&format!("block wildcard {spelling}")]);
            assert!(rules.is_blocked("ads.example"), "{spelling}");
            assert!(rules.is_blocked("x.ads.example"), "{spelling}");
            assert!(!rules.is_blocked("badads.example"), "{spelling}");
        }
    }

    #[test]
    fn regex_matches_anywhere_unless_anchored() {
        let rules = set(&[r"block regex ^ad[0-9]+\."]);
        assert!(rules.is_blocked("ad42.tracker.example"));
        assert!(!rules.is_blocked("bad42.tracker.example"));
        let unanchored = set(&[r"block regex track"]);
        assert!(unanchored.is_blocked("my.tracker.example"));
    }

    #[test]
    fn regex_sees_the_normalised_name() {
        let rules = set(&[r"block regex ^ads\.example$"]);
        assert!(rules.is_blocked("ADS.Example."));
    }

    #[test]
    fn allow_inside_a_blocked_wildcard_wins() {
        let rules = set(&[
            "block wildcard *.ads.example",
            "allow domain ok.ads.example",
        ]);
        assert_eq!(rules.verdict("ok.ads.example"), Verdict::Allow);
        assert!(rules.is_blocked("bad.ads.example"));
        assert!(rules.is_blocked("ads.example"));
    }

    #[test]
    fn an_allow_rule_overrides_block_regex_dot_star() {
        let rules = set(&["block regex .*", "allow domain intranet.example"]);
        assert_eq!(rules.verdict("intranet.example"), Verdict::Allow);
        assert!(rules.is_blocked("anything.example"));
    }

    #[test]
    fn allow_wins_whichever_type_matches() {
        let rules = set(&[
            "block domain a.example",
            "allow wildcard example",
            "block wildcard b.example",
            "allow regex ^b\\.",
        ]);
        assert_eq!(rules.verdict("a.example"), Verdict::Allow);
        assert_eq!(rules.verdict("b.example"), Verdict::Allow);
    }

    #[test]
    fn unmatched_names_pass_through() {
        let rules = set(&["block domain ads.example"]);
        assert_eq!(rules.verdict("news.example"), Verdict::Pass);
        assert_eq!(
            RuleSet::default().verdict("anything.example"),
            Verdict::Pass
        );
        assert_eq!(rules.verdict(""), Verdict::Pass);
        assert_eq!(rules.verdict("."), Verdict::Pass);
    }

    #[test]
    fn hosts_style_domain_rules_do_not_match_subdomains() {
        let rules = set(&["block domain a.example"]);
        assert!(rules.is_blocked("a.example"));
        assert!(!rules.is_blocked("x.a.example"));
    }

    #[test]
    fn queries_with_unusual_characters_do_not_panic_and_match_by_case() {
        let rules = set(&["block domain _dmarc.example"]);
        assert!(rules.is_blocked("_DMARC.example"));
        assert_eq!(rules.verdict("a b.example"), Verdict::Pass);
        assert_eq!(rules.verdict("bücher.example"), Verdict::Pass);
    }

    #[test]
    fn rules_can_be_built_from_owned_or_borrowed_values() {
        let owned = vec![rule("block domain a.example")];
        assert!(
            RuleSet::from_rules(&owned)
                .expect("borrowed")
                .is_blocked("a.example")
        );
        assert!(
            RuleSet::from_rules(owned)
                .expect("owned")
                .is_blocked("a.example")
        );
    }

    // --- the live set: atomic replacement ---

    /// A set of `count` names under `side`, all blocked together or not at all.
    fn generation(side: &str, count: usize) -> RuleSet {
        let rules: Vec<Rule> = (0..count)
            .map(|i| rule(&format!("block domain n{i}.{side}.example")))
            .collect();
        RuleSet::from_rules(&rules).expect("build")
    }

    #[test]
    fn readers_during_repeated_swaps_see_a_complete_old_or_new_set() {
        const COUNT: usize = 300;
        let live = Arc::new(LiveRules::new(generation("a", COUNT)));
        let done = Arc::new(AtomicBool::new(false));

        let readers: Vec<_> = (0..4)
            .map(|_| {
                let live = Arc::clone(&live);
                let done = Arc::clone(&done);
                thread::spawn(move || {
                    let mut loads = 0u64;
                    while !done.load(Ordering::Relaxed) {
                        // One load, then every check against that one set.
                        let rules = live.load();
                        let a = (0..COUNT)
                            .filter(|i| rules.is_blocked(&format!("n{i}.a.example")))
                            .count();
                        let b = (0..COUNT)
                            .filter(|i| rules.is_blocked(&format!("n{i}.b.example")))
                            .count();
                        let complete_a = a == COUNT && b == 0;
                        let complete_b = b == COUNT && a == 0;
                        assert!(
                            complete_a || complete_b,
                            "saw a mix: {a} of A and {b} of B blocked"
                        );
                        loads += 1;
                    }
                    loads
                })
            })
            .collect();

        // The writer builds each replacement from scratch, like a refresh.
        for round in 0..200 {
            let side = if round % 2 == 0 { "b" } else { "a" };
            live.swap(generation(side, COUNT));
        }
        done.store(true, Ordering::Relaxed);

        let total: u64 = readers
            .into_iter()
            .map(|r| r.join().expect("reader panicked"))
            .sum();
        assert!(total > 0, "readers never ran");
    }

    #[test]
    fn a_loaded_set_stays_valid_after_a_swap() {
        let live = LiveRules::new(set(&["block domain old.example"]));
        let before = live.load();
        live.swap(set(&["block domain new.example"]));
        assert!(before.is_blocked("old.example"));
        assert!(!before.is_blocked("new.example"));
        let after = live.load();
        assert!(after.is_blocked("new.example"));
        assert!(!after.is_blocked("old.example"));
    }

    #[test]
    fn the_name_index_reports_its_size() {
        // Each automaton has a small fixed header; names add to it.
        let empty = RuleSet::default().name_index_bytes();
        assert!(empty < 512, "{empty}");
        assert_eq!(set(&["block regex x"]).name_index_bytes(), empty);
        let some = set(&["block domain a.example", "block wildcard b.example"]);
        assert!(some.name_index_bytes() > empty);
    }

    // --- the FST index against a plain hash-set implementation ---

    use std::collections::HashSet;

    /// The straightforward implementation the FST replaced, kept as an oracle.
    struct HashNames {
        exact: HashSet<String>,
        suffix: HashSet<String>,
    }

    impl NameIndex for HashNames {
        fn contains_exact(&self, name: &str) -> bool {
            self.exact.contains(name)
        }

        fn matches_suffix(&self, name: &str) -> bool {
            let mut rest = name;
            loop {
                if self.suffix.contains(rest) {
                    return true;
                }
                match rest.split_once('.') {
                    Some((_, parent)) => rest = parent,
                    None => return false,
                }
            }
        }
    }

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    const VOCAB: [&str; 24] = [
        "com",
        "net",
        "org",
        "example",
        "ads",
        "track",
        "cdn",
        "a",
        "b",
        "x1",
        "x2",
        "mail",
        "www",
        "api",
        "tele",
        "metry",
        "pix",
        "el",
        "_dmarc",
        "xn--bcher-kva",
        "9",
        "zz",
        "t",
        "q",
    ];

    /// Small vocabulary, so names share prefixes, parents and children often.
    fn random_name(rng: &mut Lcg, max_depth: usize) -> String {
        let depth = 1 + rng.below(max_depth);
        (0..depth)
            .map(|_| VOCAB[rng.below(VOCAB.len())])
            .collect::<Vec<_>>()
            .join(".")
    }

    #[test]
    fn the_fst_index_agrees_with_the_hash_set_oracle() {
        let mut rng = Lcg(0x5eed);
        let exact: Vec<String> = (0..3000).map(|_| random_name(&mut rng, 5)).collect();
        let suffix: Vec<String> = (0..300).map(|_| random_name(&mut rng, 3)).collect();

        let fst = FstNames::build(exact.clone(), suffix.clone()).expect("build");
        let oracle = HashNames {
            exact: exact.iter().cloned().collect(),
            suffix: suffix.iter().cloned().collect(),
        };

        // Probes: random names, every stored name, its parents, a child, and
        // the name glued to a prefix without a label boundary.
        let mut probes: Vec<String> = (0..20_000).map(|_| random_name(&mut rng, 6)).collect();
        for name in exact.iter().chain(&suffix) {
            probes.push(name.clone());
            probes.push(format!("child.{name}"));
            probes.push(format!("bad{name}"));
            probes.push(format!("{name}x"));
            let mut rest = name.as_str();
            while let Some((_, parent)) = rest.split_once('.') {
                probes.push(parent.to_owned());
                rest = parent;
            }
        }

        let mut exact_hits = 0;
        let mut suffix_hits = 0;
        for probe in &probes {
            let (e, s) = (oracle.contains_exact(probe), oracle.matches_suffix(probe));
            assert_eq!(fst.contains_exact(probe), e, "exact: {probe}");
            assert_eq!(fst.matches_suffix(probe), s, "suffix: {probe}");
            exact_hits += usize::from(e);
            suffix_hits += usize::from(s);
        }
        // The probe set exercises both outcomes of both lookups.
        assert!(
            exact_hits > 1000 && exact_hits < probes.len(),
            "{exact_hits}"
        );
        assert!(
            suffix_hits > 1000 && suffix_hits < probes.len(),
            "{suffix_hits}"
        );
    }

    #[test]
    fn the_fst_index_handles_edges() {
        let names = |exact: &[&str], suffix: &[&str]| {
            FstNames::build(exact.iter().copied(), suffix.iter().copied()).expect("build")
        };
        let empty = names(&[], &[]);
        assert!(!empty.contains_exact("a.example"));
        assert!(!empty.matches_suffix("a.example"));

        // A single-label wildcard base covers a whole TLD, and itself.
        let tld = names(&[], &["example"]);
        assert!(tld.matches_suffix("example"));
        assert!(tld.matches_suffix("a.b.example"));
        assert!(!tld.matches_suffix("examples"));
        assert!(!tld.matches_suffix("notexample"));

        // Duplicates are fine and an exact name is not a wildcard.
        let dup = names(&["a.example", "a.example"], &[]);
        assert!(dup.contains_exact("a.example"));
        assert!(!dup.contains_exact("example"));
        assert!(!dup.contains_exact("b.a.example"));
        assert!(!dup.matches_suffix("a.example"));

        // Odd queries don't panic: empty labels, leading dot, long names.
        let any = names(&["a.example"], &["b.example"]);
        for odd in [
            "",
            ".",
            "a..example",
            ".a.example",
            "..",
            "a.example.",
            &"x.".repeat(200),
        ] {
            let _ = (any.contains_exact(odd), any.matches_suffix(odd));
        }
    }
}
