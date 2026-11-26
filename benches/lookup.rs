//! Lookup cost against a 1M-name rule set: `cargo bench`.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use hickory_filter::ruleset::{RuleSet, Verdict};

#[path = "workload.rs"]
mod workload;

fn lookups(c: &mut Criterion) {
    let set = RuleSet::from_rules(workload::rules(workload::EXACT_NAMES)).expect("build");

    let hit = workload::sample_exact(workload::EXACT_NAMES / 2);
    assert_eq!(set.verdict(&hit), Verdict::Block);
    c.bench_function("exact hit (1M names)", |b| {
        b.iter(|| set.verdict(black_box(&hit)))
    });

    let suffix = format!("a.b.{}", workload::wildcard_base(42));
    let suffix = suffix.as_str();
    assert_eq!(set.verdict(suffix), Verdict::Block);
    c.bench_function("wildcard hit, two labels below the base", |b| {
        b.iter(|| set.verdict(black_box(suffix)))
    });

    let regex = "t7x123.tracker.example";
    assert_eq!(set.verdict(regex), Verdict::Block);
    c.bench_function("regex hit (50 patterns)", |b| {
        b.iter(|| set.verdict(black_box(regex)))
    });

    let allowed = "ok5.allowed.example";
    assert_eq!(set.verdict(allowed), Verdict::Allow);
    c.bench_function("allow hit", |b| b.iter(|| set.verdict(black_box(allowed))));

    let miss = "www.news.example";
    assert_eq!(set.verdict("www.unlisted-site-q9.com"), Verdict::Pass);
    assert_eq!(set.verdict(miss), Verdict::Pass);
    c.bench_function("miss (walks suffixes, runs regex set)", |b| {
        b.iter(|| set.verdict(black_box(miss)))
    });

    let miss_com = "www.unlisted-site-q9.com";
    c.bench_function("miss under a populated suffix (com)", |b| {
        b.iter(|| set.verdict(black_box(miss_com)))
    });

    let upper = "WWW.News.Example.";
    c.bench_function("miss, mixed case with trailing dot", |b| {
        b.iter(|| set.verdict(black_box(upper)))
    });
}

criterion_group!(benches, lookups);
criterion_main!(benches);
