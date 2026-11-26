//! Metrics: names, the recorder, and the helpers that keep labels bounded.
//!
//! Everything is recorded through the `metrics` facade and rendered by one
//! Prometheus recorder, installed once per process. Labels are bounded by
//! construction: the query type is one of a fixed set, the outcome one of four,
//! and lists are identified by their configured action, type and URL with any
//! credentials removed. No metric carries a client address or a queried name.

use std::{
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use hickory_server::proto::rr::RecordType;
use metrics::{counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};

use crate::{
    lists::{LastResult, ListState},
    rules::Action,
    ruleset::RuleCounts,
};

pub const QUERIES_TOTAL: &str = "hickory_filter_queries_total";
pub const QUERY_DURATION: &str = "hickory_filter_query_duration_seconds";
pub const RULES: &str = "hickory_filter_rules";
pub const RULESET_SWAPS_TOTAL: &str = "hickory_filter_ruleset_swaps_total";
pub const RULESET_LAST_SWAP: &str = "hickory_filter_ruleset_last_swap_timestamp_seconds";
pub const LIST_RULES: &str = "hickory_filter_list_rules";
pub const LIST_SKIPPED_LINES: &str = "hickory_filter_list_skipped_lines";
pub const LIST_LAST_SUCCESS: &str = "hickory_filter_list_last_success_timestamp_seconds";
pub const LIST_LAST_REFRESH_SUCCESS: &str = "hickory_filter_list_last_refresh_success";

/// The metrics the resolver library records for the response cache and the
/// upstreams, exposed next to ours (labels: name server, protocol, status).
pub const UPSTREAM_QUERIES_TOTAL: &str = "hickory_resolver_outgoing_queries_total";
pub const UPSTREAM_RESPONSES_TOTAL: &str = "hickory_resolver_name_server_query_responses_total";
pub const CACHE_HIT_TOTAL: &str = "hickory_resolver_cache_hit_total";
pub const CACHE_MISS_TOTAL: &str = "hickory_resolver_cache_miss_total";

/// The metrics this crate records itself.
pub const OWN_METRICS: &[&str] = &[
    QUERIES_TOTAL,
    QUERY_DURATION,
    RULES,
    RULESET_SWAPS_TOTAL,
    RULESET_LAST_SWAP,
    LIST_RULES,
    LIST_SKIPPED_LINES,
    LIST_LAST_SUCCESS,
    LIST_LAST_REFRESH_SUCCESS,
];

/// Answer-time buckets, in seconds: from a cache hit to a slow upstream.
const DURATION_BUCKETS: &[f64] = &[
    0.000_05, 0.000_1, 0.000_25, 0.000_5, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5,
    1.0, 2.5, 5.0,
];

static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the recorder (once per process) and describe the metrics. Calling it
/// again returns the same handle.
pub fn install() -> &'static PrometheusHandle {
    HANDLE.get_or_init(|| {
        let handle = PrometheusBuilder::new()
            .set_buckets_for_metric(
                Matcher::Suffix("_duration_seconds".to_owned()),
                DURATION_BUCKETS,
            )
            .expect("the buckets are valid")
            .install_recorder()
            .expect("no other metrics recorder is installed");
        describe();
        handle
    })
}

/// The current metrics in the Prometheus text format.
pub fn render() -> String {
    install().render()
}

fn describe() {
    describe_counter!(
        QUERIES_TOTAL,
        "Queries answered, by outcome (allowed, blocked, passed, upstream_error) and query type"
    );
    describe_histogram!(
        QUERY_DURATION,
        metrics::Unit::Seconds,
        "Time taken to answer a query, by outcome"
    );
    describe_gauge!(RULES, "Rules in the live rule set, by action and kind");
    describe_counter!(
        RULESET_SWAPS_TOTAL,
        "Times the live rule set has been replaced"
    );
    describe_gauge!(
        RULESET_LAST_SWAP,
        "Unix time of the last rule set replacement"
    );
    describe_gauge!(LIST_RULES, "Rules in a list's last good content");
    describe_gauge!(
        LIST_SKIPPED_LINES,
        "Lines skipped as invalid in a list's last successful load"
    );
    describe_gauge!(
        LIST_LAST_SUCCESS,
        "Unix time of a list's last successful load or refresh"
    );
    describe_gauge!(
        LIST_LAST_REFRESH_SUCCESS,
        "1 if a list's last refresh attempt succeeded, 0 if it failed"
    );
}

/// The label for a query type: one of a fixed set of common types, and `other`
/// for everything else, so the number of series stays bounded.
pub fn type_label(record_type: RecordType) -> &'static str {
    match record_type {
        RecordType::A => "A",
        RecordType::AAAA => "AAAA",
        RecordType::CNAME => "CNAME",
        RecordType::HTTPS => "HTTPS",
        RecordType::MX => "MX",
        RecordType::NS => "NS",
        RecordType::PTR => "PTR",
        RecordType::SOA => "SOA",
        RecordType::SRV => "SRV",
        RecordType::SVCB => "SVCB",
        RecordType::TXT => "TXT",
        _ => "other",
    }
}

/// Every label `type_label` can return.
pub const TYPE_LABELS: &[&str] = &[
    "A", "AAAA", "CNAME", "HTTPS", "MX", "NS", "PTR", "SOA", "SRV", "SVCB", "TXT", "other",
];

/// A list URL safe to log or use as a label: credentials (`user:password@`)
/// are removed, and a query string, which often carries a token, is replaced
/// by `?…`.
pub fn display_url(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (format!("{scheme}://"), rest),
        None => (String::new(), url),
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let tail = match tail.find(['?', '#']) {
        Some(at) if tail[at..].starts_with('?') => format!("{}?…", &tail[..at]),
        Some(at) => tail[..at].to_owned(),
        None => tail.to_owned(),
    };
    format!("{scheme}{host}{tail}")
}

fn unix_seconds(time: SystemTime) -> f64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs_f64()
}

/// Count one answered query and how long it took.
pub fn record_query(outcome: &'static str, record_type: RecordType, elapsed: Duration) {
    counter!(QUERIES_TOTAL, "outcome" => outcome, "type" => type_label(record_type)).increment(1);
    histogram!(QUERY_DURATION, "outcome" => outcome).record(elapsed.as_secs_f64());
}

/// Record that the rule set was replaced by one with these rule counts.
pub fn record_ruleset_swap(counts: &RuleCounts) {
    for (action, kinds) in [("allow", &counts.allow), ("block", &counts.block)] {
        gauge!(RULES, "action" => action, "kind" => "domain").set(kinds.domain as f64);
        gauge!(RULES, "action" => action, "kind" => "wildcard").set(kinds.wildcard as f64);
        gauge!(RULES, "action" => action, "kind" => "regex").set(kinds.regex as f64);
    }
    counter!(RULESET_SWAPS_TOTAL).increment(1);
    gauge!(RULESET_LAST_SWAP).set(unix_seconds(SystemTime::now()));
}

/// Publish a list's state: its rules, skipped lines, last success and the
/// result of the last attempt. A list that hasn't been attempted yet publishes
/// nothing.
pub fn record_list(list: &ListState) {
    if list.last_result == LastResult::NotAttempted {
        return;
    }
    let action = match list.decl.action {
        Action::Allow => "allow",
        Action::Block => "block",
    };
    let list_type = list.decl.list_type.as_str();
    let url = display_url(&list.decl.url);
    gauge!(LIST_RULES, "action" => action, "type" => list_type, "url" => url.clone())
        .set(list.rules.len() as f64);
    gauge!(LIST_SKIPPED_LINES, "action" => action, "type" => list_type, "url" => url.clone())
        .set(list.skipped as f64);
    if let Some(success) = list.last_success {
        gauge!(LIST_LAST_SUCCESS, "action" => action, "type" => list_type, "url" => url.clone())
            .set(unix_seconds(success));
    }
    let succeeded = !matches!(list.last_result, LastResult::Failed(_));
    gauge!(LIST_LAST_REFRESH_SUCCESS, "action" => action, "type" => list_type, "url" => url)
        .set(if succeeded { 1.0 } else { 0.0 });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_types_keep_their_name_and_the_rest_are_other() {
        assert_eq!(type_label(RecordType::A), "A");
        assert_eq!(type_label(RecordType::AAAA), "AAAA");
        assert_eq!(type_label(RecordType::HTTPS), "HTTPS");
        assert_eq!(type_label(RecordType::TXT), "TXT");
        assert_eq!(type_label(RecordType::Unknown(65280)), "other");
        assert_eq!(type_label(RecordType::ANY), "other");
        assert_eq!(type_label(RecordType::AXFR), "other");
    }

    #[test]
    fn every_type_label_is_in_the_documented_set() {
        for code in 0..=u16::MAX {
            let label = type_label(RecordType::from(code));
            assert!(TYPE_LABELS.contains(&label), "{code}: {label}");
        }
    }

    #[test]
    fn credentials_and_query_strings_are_removed_from_list_urls() {
        for (url, shown) in [
            (
                "https://lists.example/hosts.txt",
                "https://lists.example/hosts.txt",
            ),
            (
                "https://user:secret@lists.example/hosts.txt",
                "https://lists.example/hosts.txt",
            ),
            (
                "https://token@lists.example:8443/a/b.txt",
                "https://lists.example:8443/a/b.txt",
            ),
            (
                "https://lists.example/hosts.txt?token=abc123",
                "https://lists.example/hosts.txt?…",
            ),
            (
                "https://u:p@lists.example/h.txt?key=v#frag",
                "https://lists.example/h.txt?…",
            ),
            (
                "https://lists.example/h.txt#frag",
                "https://lists.example/h.txt",
            ),
            ("https://lists.example?token=abc", "https://lists.example?…"),
            (
                "file:///etc/hickory-filter/block.txt",
                "file:///etc/hickory-filter/block.txt",
            ),
            ("https://a@b@lists.example/x", "https://lists.example/x"),
        ] {
            let result = display_url(url);
            assert_eq!(result, shown, "{url}");
            assert!(
                !result.contains("secret") && !result.contains("abc"),
                "{result}"
            );
        }
    }
}
