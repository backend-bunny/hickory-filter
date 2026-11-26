//! The rule set: what decides whether a queried name is allowed or blocked.
//!
//! A `RuleSet` is immutable and built whole from rules. The live one sits in a
//! `LiveRules`, which a refresh replaces with a single atomic pointer swap, so
//! a query sees either the complete old set or the complete new one, and the
//! query path takes no lock and does no I/O.
//!
//! Matching order: allow before block, and within each side exact name, then
//! suffix (wildcard) walk, then regex. Allow always wins.

use std::{
    borrow::{Borrow, Cow},
    collections::HashSet,
    sync::Arc,
};

use arc_swap::{ArcSwap, Guard};
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

/// Name lookups behind a small interface, so the storage (hash sets today, a
/// reversed-domain automaton later) can change without touching parsing,
/// refresh or the handler. Names are already normalised.
pub trait NameIndex {
    /// Is exactly this name in the index?
    fn contains_exact(&self, name: &str) -> bool;
    /// Is this name, or any parent of it at a label boundary, a wildcard base?
    fn matches_suffix(&self, name: &str) -> bool;
}

#[derive(Default)]
struct HashNames {
    exact: HashSet<Box<str>>,
    suffix: HashSet<Box<str>>,
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

/// The allow side or the block side of a rule set.
#[derive(Default)]
struct Side {
    names: HashNames,
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
    names: HashNames,
    patterns: Vec<String>,
}

impl SideBuilder {
    fn add(&mut self, matcher: &Matcher) {
        match matcher {
            Matcher::Domain(name) => {
                self.names.exact.insert(name.as_str().into());
            }
            Matcher::Wildcard(base) => {
                self.names.suffix.insert(base.as_str().into());
            }
            Matcher::Regex(regex) => self.patterns.push(regex.as_str().to_owned()),
        }
    }

    fn finish(self) -> Result<Side, regex::Error> {
        Ok(Side {
            names: self.names,
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
    /// Build a set from rules, owned or borrowed. Fails only if the regexes
    /// together are too large to compile into one set.
    pub fn from_rules<R: Borrow<Rule>>(
        rules: impl IntoIterator<Item = R>,
    ) -> Result<Self, regex::Error> {
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
}
