//! Recursive mode, end to end, against fake root, TLD and authoritative
//! servers in a network namespace (see `common::netns`).

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Command,
};

use hickory_server::net::xfer::Protocol;
use hickory_server::proto::{
    op::{Message, ResponseCode},
    rr::{RData, RecordType},
};

mod common;
use common::{
    authority::{self, Hierarchy},
    dns::ask_udp,
    netns,
    process::{Process, http_get, http_status_line, spawn, spawn_command},
};

/// A server in recursive mode starting from the fake root. DNSSEC is off:
/// the fake zones are unsigned, and the built-in trust anchor would reject them.
struct Server {
    process: Process,
    addr: SocketAddr,
    http: SocketAddr,
    dir: PathBuf,
    _dir: Option<tempfile::TempDir>,
}

/// Write the hints and a recursive-mode configuration into `dir`; returns the
/// configuration file. `top` is extra top-level configuration, `recursion`
/// extra `[recursion]` settings and `tables` further tables, written after it.
fn write_config(dir: &Path, top: &str, recursion: &str, tables: &str) -> PathBuf {
    let hints = authority::write_hints(dir);
    let file = dir.join("config.toml");
    std::fs::write(
        &file,
        format!(
            "mode = \"recursive\"\nlisten = [\"127.0.0.1:0\"]\nhttp = \"127.0.0.1:0\"\n{top}\n\
             [recursion]\nroots = \"{}\"\ndnssec = \"off\"\n{recursion}\n{tables}\n",
            hints.display()
        ),
    )
    .expect("write config");
    file
}

/// The names of the files in `dir`, sorted.
fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read the directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

impl Server {
    /// A server with its own directory.
    fn start(top: &str, recursion: &str, tables: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut server = Self::start_in(dir.path(), top, recursion, tables);
        server._dir = Some(dir);
        server
    }

    /// A server whose files live in `dir`, which the caller keeps.
    fn start_in(dir: &Path, top: &str, recursion: &str, tables: &str) -> Self {
        let file = write_config(dir, top, recursion, tables);
        let process = spawn(&file);
        // The listeners are announced before the endpoint.
        let addr = process.listening_addr();
        let http = process.http_addr();
        Self {
            process,
            addr,
            http,
            dir: dir.to_owned(),
            _dir: None,
        }
    }

    /// The files in the server's directory.
    fn files(&self) -> Vec<String> {
        files(&self.dir)
    }

    /// The text of `/metrics`.
    fn metrics(&self) -> String {
        let response = http_get(self.http, "/metrics");
        response.split("\r\n\r\n").nth(1).unwrap_or("").to_owned()
    }

    fn status(&self, path: &str) -> String {
        http_status_line(self.http, path)
    }

    /// Wait until `/ready` answers 200.
    async fn wait_ready(&self) {
        for _ in 0..300 {
            if self.status("/ready").contains(" 200") {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("/ready never answered 200: {}", self.status("/ready"));
    }

    /// Wait until the sum of `name`'s samples that satisfy `keep` reaches `at_least`.
    async fn wait_for_metric(&self, name: &str, keep: impl Fn(&str) -> bool, at_least: f64) {
        for _ in 0..100 {
            if metric(&self.metrics(), name, &keep) >= at_least {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("{name} never reached {at_least}:\n{}", self.metrics());
    }

    async fn ask(&self, name: &str) -> Message {
        let _ = &self.process;
        ask_udp(self.addr, name, RecordType::A).await
    }
}

/// The sum of the samples of `name` whose line satisfies `keep`.
fn metric(text: &str, name: &str, keep: impl Fn(&str) -> bool) -> f64 {
    text.lines()
        .filter(|line| line.starts_with(name) && keep(line))
        .filter_map(|line| line.rsplit(' ').next()?.parse::<f64>().ok())
        .sum()
}

fn a_records(message: &Message) -> Vec<std::net::Ipv4Addr> {
    message
        .answers
        .iter()
        .filter_map(|r| match &r.data {
            RData::A(a) => Some(a.0),
            _ => None,
        })
        .collect()
}

/// The harness on its own: the fake servers form a working delegation chain,
/// walked by hand without any resolver of ours.
#[tokio::test]
async fn the_fake_hierarchy_delegates_from_the_root_to_the_authoritative_server() {
    let Some(_ns) = netns::enter(
        "the_fake_hierarchy_delegates_from_the_root_to_the_authoritative_server",
        &authority::addresses(),
    ) else {
        return;
    };
    let hierarchy = Hierarchy::start().await;
    let ask = |server: std::net::Ipv4Addr| {
        let addr = std::net::SocketAddr::from((server, 53));
        async move { ask_udp(addr, "www.example.test.", RecordType::A).await }
    };

    // The root refers to the `test.` server, the `test.` server to the
    // `example.test.` server, which answers.
    let from_root: Message = ask(authority::ROOT).await;
    assert!(from_root.answers.is_empty());
    assert!(
        from_root
            .additionals
            .iter()
            .any(|r| matches!(&r.data, RData::A(a) if a.0 == authority::TLD)),
        "the root's referral carries the TLD server's address: {from_root:?}"
    );
    let from_tld = ask(authority::TLD).await;
    assert!(
        from_tld
            .additionals
            .iter()
            .any(|r| matches!(&r.data, RData::A(a) if a.0 == authority::AUTHORITY)),
        "the TLD's referral carries the authoritative server's address: {from_tld:?}"
    );
    let answer = ask(authority::AUTHORITY).await;
    assert_eq!(answer.metadata.response_code, ResponseCode::NoError);
    assert!(answer.metadata.authoritative);
    assert!(
        answer
            .answers
            .iter()
            .any(|r| matches!(&r.data, RData::A(a) if a.0 == authority::WWW)),
        "{answer:?}"
    );

    // A name that doesn't exist is a name error from the server that owns it.
    let missing = ask_udp(
        std::net::SocketAddr::from((authority::AUTHORITY, 53)),
        "missing.example.test.",
        RecordType::A,
    )
    .await;
    assert_eq!(missing.metadata.response_code, ResponseCode::NXDomain);

    assert_eq!(hierarchy.root.seen().len(), 1);
    assert_eq!(hierarchy.tld.seen().len(), 1);
    assert_eq!(hierarchy.authority.seen().len(), 2);
}

/// A name is answered by walking the delegations from the root to the
/// authoritative server, with no resolver involved: the three fake servers are
/// the only name servers that exist in the namespace.
#[tokio::test]
async fn a_name_is_resolved_through_the_delegation_chain() {
    let Some(_ns) = netns::enter(
        "a_name_is_resolved_through_the_delegation_chain",
        &authority::addresses(),
    ) else {
        return;
    };
    let hierarchy = Hierarchy::start().await;
    let server = Server::start("", "", "");

    let answer = server.ask("www.example.test.").await;
    assert_eq!(
        answer.metadata.response_code,
        ResponseCode::NoError,
        "{answer:?}"
    );
    assert_eq!(a_records(&answer), vec![authority::WWW]);

    assert!(!hierarchy.root.seen().is_empty(), "the root was asked");
    assert!(!hierarchy.tld.seen().is_empty(), "the TLD server was asked");
    assert!(
        hierarchy
            .authority
            .seen()
            .iter()
            .any(|q| q.name.eq_ignore_ascii_case("www.example.test.")
                && q.record_type == RecordType::A),
        "the authoritative server was asked for the name: {:?}",
        hierarchy.authority.seen()
    );
}

/// An answer is cached: asking again within its TTL sends nothing.
#[tokio::test]
async fn a_resolved_name_is_cached() {
    let Some(_ns) = netns::enter("a_resolved_name_is_cached", &authority::addresses()) else {
        return;
    };
    let hierarchy = Hierarchy::start().await;
    let server = Server::start("", "", "");

    assert_eq!(
        a_records(&server.ask("www.example.test.").await),
        vec![authority::WWW]
    );
    let after_first = hierarchy.queries_received();
    assert!(after_first > 0);

    assert_eq!(
        a_records(&server.ask("www.example.test.").await),
        vec![authority::WWW]
    );
    assert_eq!(
        hierarchy.queries_received(),
        after_first,
        "the second query was answered from the cache"
    );
}

/// With `cache_size = 0` there is no response cache, so every query reaches
/// the authoritative server again.
#[tokio::test]
async fn a_cache_size_of_zero_disables_the_response_cache() {
    let Some(_ns) = netns::enter(
        "a_cache_size_of_zero_disables_the_response_cache",
        &authority::addresses(),
    ) else {
        return;
    };
    let hierarchy = Hierarchy::start().await;
    let server = Server::start("cache_size = 0", "", "");

    assert_eq!(
        a_records(&server.ask("www.example.test.").await),
        vec![authority::WWW]
    );
    let asked = |h: &Hierarchy| {
        h.authority
            .seen()
            .iter()
            .filter(|q| {
                q.name.eq_ignore_ascii_case("www.example.test.") && q.record_type == RecordType::A
            })
            .count()
    };
    let after_first = asked(&hierarchy);
    assert_eq!(
        a_records(&server.ask("www.example.test.").await),
        vec![authority::WWW]
    );
    assert!(
        asked(&hierarchy) > after_first,
        "the second query reached the authoritative server"
    );
}

/// Filtering is in front of the recursion: a blocked name is answered by the
/// server itself and no name server hears about it.
#[tokio::test]
async fn a_blocked_name_sends_no_query_to_any_name_server() {
    let Some(_ns) = netns::enter(
        "a_blocked_name_sends_no_query_to_any_name_server",
        &authority::addresses(),
    ) else {
        return;
    };
    let hierarchy = Hierarchy::start().await;
    let server = Server::start(
        "",
        "",
        "[filter]\nrules = [\"block domain ads.example.test\"]\n",
    );
    // Startup sends one query to the root to check it can be reached; wait for
    // it, so what is counted next is the client's.
    server.wait_ready().await;
    let before = hierarchy.queries_received();

    let blocked = server.ask("ads.example.test.").await;
    assert_eq!(blocked.metadata.response_code, ResponseCode::NoError);
    assert_eq!(
        a_records(&blocked),
        vec![std::net::Ipv4Addr::UNSPECIFIED],
        "the null block answer"
    );
    assert_eq!(
        hierarchy.queries_received(),
        before,
        "the blocked query sent nothing to any name server"
    );

    // An unblocked name next to it still resolves.
    assert_eq!(
        a_records(&server.ask("www.example.test.").await),
        vec![authority::WWW]
    );
}

/// An allow rule beats a block rule in recursive mode as well.
#[tokio::test]
async fn allow_beats_block_in_recursive_mode() {
    let Some(_ns) = netns::enter(
        "allow_beats_block_in_recursive_mode",
        &authority::addresses(),
    ) else {
        return;
    };
    let _hierarchy = Hierarchy::start().await;
    let server = Server::start(
        "",
        "",
        "[filter]\nrules = [\"block wildcard *.example.test\", \"allow domain www.example.test\"]\n",
    );
    assert_eq!(
        a_records(&server.ask("www.example.test.").await),
        vec![authority::WWW]
    );
    let blocked = server.ask("ads.example.test.").await;
    assert_eq!(a_records(&blocked), vec![std::net::Ipv4Addr::UNSPECIFIED]);
}

/// When the name servers don't answer the client gets SERVFAIL: no blocked-style
/// answer, and nothing is asked of anyone outside the delegation chain.
#[tokio::test]
async fn unreachable_name_servers_give_servfail_and_are_counted() {
    let Some(_ns) = netns::enter(
        "unreachable_name_servers_give_servfail_and_are_counted",
        &authority::addresses(),
    ) else {
        return;
    };
    // Only the root exists; it delegates `test.` to a server that isn't there.
    let root = authority::FakeServer::start(authority::ROOT, |name, _| {
        if name == "test." || name.ends_with(".test.") {
            authority::referral("test.", "ns.test.", authority::TLD)
        } else {
            authority::nxdomain(".")
        }
    })
    .await;
    let server = Server::start("", "", "");

    let answer = server.ask("www.example.test.").await;
    assert_eq!(
        answer.metadata.response_code,
        ResponseCode::ServFail,
        "{answer:?}"
    );
    assert!(
        answer.answers.is_empty(),
        "no answer of any kind: {answer:?}"
    );
    assert!(!root.seen().is_empty(), "the root was asked");

    // The failure is counted: as a timed-out response from a name server and
    // as a query that ended in an upstream error.
    let metrics = server.metrics();
    assert!(
        metric(
            &metrics,
            "hickory_resolver_name_server_query_responses_total",
            |l| { !l.contains("status=\"success\"") }
        ) >= 1.0,
        "a failed name server response is counted:\n{metrics}"
    );
    assert_eq!(
        metric(&metrics, "hickory_filter_queries_total", |l| l
            .contains("outcome=\"upstream_error\"")),
        1.0,
        "{metrics}"
    );
}

/// A delegation to a private address is never followed, so a delegation can't
/// make the server probe its own network. A fake server listens on that
/// address to prove nothing reaches it.
#[tokio::test]
async fn a_delegation_to_a_private_address_is_not_contacted() {
    let mut addresses = authority::addresses();
    addresses.push("10.0.0.1".to_owned());
    let Some(_ns) = netns::enter(
        "a_delegation_to_a_private_address_is_not_contacted",
        &addresses,
    ) else {
        return;
    };
    let private = std::net::Ipv4Addr::new(10, 0, 0, 1);
    let spy = authority::FakeServer::start(private, |_, _| authority::nxdomain(".")).await;
    let root = authority::FakeServer::start(authority::ROOT, move |name, _| {
        if name == "test." || name.ends_with(".test.") {
            authority::referral("test.", "ns.test.", private)
        } else {
            authority::nxdomain(".")
        }
    })
    .await;
    let server = Server::start("", "", "");

    let answer = server.ask("www.example.test.").await;
    assert_eq!(
        answer.metadata.response_code,
        ResponseCode::ServFail,
        "{answer:?}"
    );
    assert!(
        !root.seen().is_empty(),
        "the root was asked and referred to the private address"
    );
    assert!(
        spy.seen().is_empty(),
        "nothing was sent to the private address: {:?}",
        spy.seen()
    );
}

/// Root servers that never answer keep `/ready` at 503, while the server is
/// alive and `/health` says so: a network that blocks DNS shows up as a pod
/// that never becomes ready.
#[tokio::test]
async fn unreachable_roots_keep_the_server_not_ready() {
    let Some(_ns) = netns::enter(
        "unreachable_roots_keep_the_server_not_ready",
        &authority::addresses(),
    ) else {
        return;
    };
    // Nothing listens at the root's address.
    let server = Server::start("", "", "");
    for _ in 0..3 {
        assert!(
            server.status("/health").contains(" 200"),
            "{}",
            server.status("/health")
        );
        assert!(
            server.status("/ready").contains(" 503"),
            "{}",
            server.status("/ready")
        );
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

/// `/ready` turns 200 when a root server answers.
#[tokio::test]
async fn the_server_is_ready_once_a_root_server_answers() {
    let Some(_ns) = netns::enter(
        "the_server_is_ready_once_a_root_server_answers",
        &authority::addresses(),
    ) else {
        return;
    };
    let _hierarchy = Hierarchy::start().await;
    let server = Server::start("", "", "");
    server.wait_ready().await;
    assert!(server.status("/ready").contains(" 200"));
}

/// Once ready, a later resolution failure doesn't flip `/ready` back.
#[tokio::test]
async fn a_later_resolution_failure_leaves_the_server_ready() {
    let Some(_ns) = netns::enter(
        "a_later_resolution_failure_leaves_the_server_ready",
        &authority::addresses(),
    ) else {
        return;
    };
    let hierarchy = Hierarchy::start().await;
    let server = Server::start("", "", "");
    server.wait_ready().await;

    // The whole hierarchy goes away; a name that was never asked for fails.
    drop(hierarchy);
    let answer = server.ask("ads.example.test.").await;
    assert_eq!(
        answer.metadata.response_code,
        ResponseCode::ServFail,
        "{answer:?}"
    );
    assert!(
        server.status("/ready").contains(" 200"),
        "{}",
        server.status("/ready")
    );
}

/// One scraped sample: the metric's name, its label keys and its value.
fn samples(text: &str) -> Vec<(String, Vec<String>, f64)> {
    text.lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .filter_map(|line| {
            let (series, value) = line.rsplit_once(' ')?;
            let (name, labels) = match series.split_once('{') {
                Some((name, rest)) => (
                    name,
                    rest.trim_end_matches('}')
                        .split(',')
                        .filter_map(|pair| pair.split_once('=').map(|(k, _)| k.to_owned()))
                        .collect(),
                ),
                None => (series, Vec::new()),
            };
            Some((name.to_owned(), labels, value.parse().ok()?))
        })
        .collect()
}

/// In recursive mode the response cache and the name servers contacted are
/// visible in the metrics, and the README documents exactly the series it names.
#[tokio::test]
async fn recursive_mode_metrics_show_the_cache_and_the_name_servers() {
    let Some(_ns) = netns::enter(
        "recursive_mode_metrics_show_the_cache_and_the_name_servers",
        &authority::addresses(),
    ) else {
        return;
    };
    let _hierarchy = Hierarchy::start().await;
    let server = Server::start("", "", "");
    assert_eq!(
        a_records(&server.ask("www.example.test.").await),
        vec![authority::WWW]
    );
    let first = server.metrics();
    assert_eq!(
        a_records(&server.ask("www.example.test.").await),
        vec![authority::WWW]
    );
    let second = server.metrics();

    // Name servers were contacted over plain UDP and none timed out (a negative
    // answer, such as no data, is a normal response).
    assert!(
        metric(&first, "hickory_resolver_outgoing_queries_total", |l| l
            .contains("protocol=\"udp\""))
            >= 3.0,
        "{first}"
    );
    assert!(
        metric(
            &first,
            "hickory_resolver_name_server_query_responses_total",
            |l| l.contains("status=\"success\"")
        ) >= 3.0
    );
    assert_eq!(
        metric(
            &first,
            "hickory_resolver_name_server_query_responses_total",
            |l| l.contains("status=\"timeout\"")
        ),
        0.0,
        "no name server timed out:\n{first}"
    );

    // The second query is a cache hit and sends nothing to a name server.
    assert_eq!(
        metric(&second, "hickory_recursor_cache_hit_total", |_| true)
            - metric(&first, "hickory_recursor_cache_hit_total", |_| true),
        1.0
    );
    assert_eq!(
        metric(&second, "hickory_recursor_outgoing_queries_total", |_| true),
        metric(&first, "hickory_recursor_outgoing_queries_total", |_| true)
    );

    // Every documented recursive-mode series is exported with the documented
    // labels and type.
    let exported = samples(&second);
    for row in common::readme_table("#### Recursive mode metrics") {
        let name = common::backticked(&row[0])[0].clone();
        let kind = row[1].clone();
        let labels = common::backticked(&row[2]);
        assert!(
            second.contains(&format!("# TYPE {name} {kind}")),
            "{name} is not exported as a {kind}:\n{second}"
        );
        let series: Vec<_> = exported.iter().filter(|(n, _, _)| *n == name).collect();
        assert!(
            !series.is_empty(),
            "documented metric {name} is not exported"
        );
        for label in &labels {
            assert!(
                series.iter().any(|(_, l, _)| l.contains(label)),
                "label {label} never appears on {name}"
            );
        }
        for (_, l, _) in &series {
            for key in l {
                assert!(
                    labels.contains(key),
                    "{name} has the undocumented label {key}"
                );
            }
        }
    }
}

/// With QNAME minimization (the default) each name server sees only as much of
/// the name as it needs: the root never the full name, the TLD server at most
/// the second-level name.
#[tokio::test]
async fn each_name_server_sees_only_the_labels_it_needs() {
    let Some(_ns) = netns::enter(
        "each_name_server_sees_only_the_labels_it_needs",
        &authority::addresses(),
    ) else {
        return;
    };
    let hierarchy = Hierarchy::start().await;
    let server = Server::start("", "", "");
    assert_eq!(
        a_records(&server.ask("www.example.test.").await),
        vec![authority::WWW]
    );

    let labels = |name: &str| name.trim_end_matches('.').split('.').count();
    for query in hierarchy.root.seen() {
        assert!(labels(&query.name) <= 1, "the root saw {:?}", query);
    }
    for query in hierarchy.tld.seen() {
        assert!(labels(&query.name) <= 2, "the TLD server saw {:?}", query);
    }
    assert!(
        hierarchy
            .authority
            .seen()
            .iter()
            .any(|q| q.name.eq_ignore_ascii_case("www.example.test.")
                && q.record_type == RecordType::A),
        "only the authoritative server is asked for the full name"
    );
}

const TLS: &str = "protocol=\"tls\"";

/// A name server that accepts DNS-over-TLS (with a certificate nobody
/// verifies) is probed and then asked over TLS, and the metrics show it.
#[tokio::test]
async fn a_name_server_that_accepts_tls_is_asked_over_tls() {
    let Some(_ns) = netns::enter(
        "a_name_server_that_accepts_tls_is_asked_over_tls",
        &authority::addresses(),
    ) else {
        return;
    };
    let hierarchy = Hierarchy::start_tls_authority().await;
    let server = Server::start("", "", "");

    // The first query goes out in the clear while the server is probed.
    assert_eq!(
        a_records(&server.ask("www.example.test.").await),
        vec![authority::WWW]
    );
    server
        .wait_for_metric(
            "hickory_resolver_probe_successes_total",
            |l| l.contains(TLS),
            1.0,
        )
        .await;

    // After the probe, queries to it use TLS.
    assert_eq!(
        a_records(&server.ask("ads.example.test.").await),
        vec![authority::ADS]
    );
    let over_tls: Vec<_> = hierarchy
        .authority
        .seen()
        .into_iter()
        .filter(|q| q.protocol == Protocol::Tls)
        .collect();
    assert!(
        over_tls
            .iter()
            .any(|q| q.name.eq_ignore_ascii_case("ads.example.test.")),
        "the authoritative server was asked over TLS: {:?}",
        hierarchy.authority.seen()
    );
    let metrics = server.metrics();
    assert!(
        metric(&metrics, "hickory_resolver_outgoing_queries_total", |l| l
            .contains(TLS))
            >= 1.0,
        "the tls transport is counted:\n{metrics}"
    );
    // Without a state file the server writes nothing at all.
    assert_eq!(server.files(), ["config.toml", "root.hints"]);
}

/// A name server that doesn't accept TLS is still answered, in the clear; the
/// probes that failed are counted.
#[tokio::test]
async fn a_name_server_that_does_not_accept_tls_is_answered_in_the_clear() {
    let Some(_ns) = netns::enter(
        "a_name_server_that_does_not_accept_tls_is_answered_in_the_clear",
        &authority::addresses(),
    ) else {
        return;
    };
    let hierarchy = Hierarchy::start().await;
    let server = Server::start("", "", "");

    assert_eq!(
        a_records(&server.ask("www.example.test.").await),
        vec![authority::WWW]
    );
    server
        .wait_for_metric(
            "hickory_resolver_probe_attempts_total",
            |l| l.contains(TLS),
            1.0,
        )
        .await;
    assert_eq!(
        a_records(&server.ask("ads.example.test.").await),
        vec![authority::ADS]
    );

    for query in hierarchy.seen().concat() {
        assert_ne!(
            query.protocol,
            Protocol::Tls,
            "no TLS to a plain server: {query:?}"
        );
    }
    let metrics = server.metrics();
    assert_eq!(
        metric(&metrics, "hickory_resolver_probe_successes_total", |l| l
            .contains(TLS)),
        0.0,
        "{metrics}"
    );
    assert_eq!(
        metric(&metrics, "hickory_resolver_outgoing_queries_total", |l| l
            .contains(TLS)),
        0.0
    );
}

/// With `opportunistic_encryption = false` nothing is probed and nothing goes
/// over TLS, even to a server that would accept it.
#[tokio::test]
async fn with_encryption_off_no_tls_is_attempted() {
    let Some(_ns) = netns::enter(
        "with_encryption_off_no_tls_is_attempted",
        &authority::addresses(),
    ) else {
        return;
    };
    let hierarchy = Hierarchy::start_tls_authority().await;
    let server = Server::start("", "opportunistic_encryption = false", "");

    assert_eq!(
        a_records(&server.ask("www.example.test.").await),
        vec![authority::WWW]
    );
    assert_eq!(
        a_records(&server.ask("ads.example.test.").await),
        vec![authority::ADS]
    );
    // Give a probe, if one were going to happen, time to show.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let metrics = server.metrics();
    assert_eq!(
        metric(&metrics, "hickory_resolver_probe_attempts_total", |_| true),
        0.0,
        "{metrics}"
    );
    assert_eq!(
        metric(&metrics, "hickory_resolver_outgoing_queries_total", |l| l
            .contains(TLS)),
        0.0
    );
    assert!(
        hierarchy
            .authority
            .seen()
            .iter()
            .all(|q| q.protocol != Protocol::Tls)
    );
}

/// What was learned about a name server's TLS support is saved to the state
/// file and used after a restart, without probing again.
#[tokio::test]
async fn a_restart_reuses_the_saved_tls_state() {
    let Some(_ns) = netns::enter(
        "a_restart_reuses_the_saved_tls_state",
        &authority::addresses(),
    ) else {
        return;
    };
    let hierarchy = Hierarchy::start_tls_authority().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("tls-state.toml");
    let recursion = format!(
        "state_file = \"{}\"\nstate_save_interval = \"1s\"\n",
        state.display()
    );

    // First run: the authoritative server is probed and found to accept TLS,
    // and the state is saved with that knowledge.
    let first = Server::start_in(dir.path(), "", &recursion, "");
    assert_eq!(
        a_records(&first.ask("www.example.test.").await),
        vec![authority::WWW]
    );
    first
        .wait_for_metric(
            "hickory_resolver_probe_successes_total",
            |l| l.contains(TLS),
            1.0,
        )
        .await;
    let address = authority::AUTHORITY.to_string();
    for _ in 0..100 {
        if std::fs::read_to_string(&state).is_ok_and(|text| text.contains(&address)) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let saved = std::fs::read_to_string(&state).expect("the state file was written");
    assert!(
        saved.contains(&address),
        "the state names the server:\n{saved}"
    );
    drop(first);

    // Second run, same state file: its first contact with the authoritative
    // server is already over TLS.
    let before = hierarchy.authority.seen().len();
    let second = Server::start_in(dir.path(), "", &recursion, "");
    assert_eq!(
        a_records(&second.ask("ads.example.test.").await),
        vec![authority::ADS]
    );
    let after: Vec<_> = hierarchy.authority.seen().split_off(before);
    assert!(
        !after.is_empty(),
        "the second run asked the authoritative server"
    );
    assert!(
        after.iter().all(|q| q.protocol == Protocol::Tls),
        "every query after the restart went over TLS: {after:?}"
    );
}

/// Recursive mode in the restricted container (an unprivileged user, no
/// capabilities, a read-only root filesystem) serves queries and writes no
/// file, because nothing is configured to be written.
#[tokio::test]
async fn recursive_mode_runs_in_the_restricted_container_and_writes_no_file() {
    let Some(_ns) = netns::enter(
        "recursive_mode_runs_in_the_restricted_container_and_writes_no_file",
        &authority::addresses(),
    ) else {
        return;
    };
    let probe = Command::new("bwrap")
        .args(["--unshare-user", "--ro-bind", "/", "/", "true"])
        .output();
    if !probe.as_ref().is_ok_and(|o| o.status.success()) {
        assert!(
            std::env::var_os("HICKORY_REQUIRE_SANDBOX").is_none(),
            "HICKORY_REQUIRE_SANDBOX is set but there is no usable sandbox: {probe:?}"
        );
        eprintln!("skipping: bwrap with a user namespace isn't available here");
        return;
    }
    let _hierarchy = Hierarchy::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let config = write_config(dir.path(), "", "", "");

    let mut command = Command::new("bwrap");
    command
        .args([
            "--unshare-user",
            "--uid",
            "65532",
            "--gid",
            "65532",
            "--cap-drop",
            "ALL",
        ])
        .args([
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--chdir",
            "/",
            "--die-with-parent",
            "--",
        ])
        .arg(env!("CARGO_BIN_EXE_hickory-filter"))
        .arg("--config")
        .arg(&config);
    let process = spawn_command(command);
    let addr = process.listening_addr();
    let http = process.http_addr();

    assert_eq!(
        a_records(&ask_udp(addr, "www.example.test.", RecordType::A).await),
        vec![authority::WWW]
    );
    assert!(http_status_line(http, "/health").contains(" 200"));
    for _ in 0..100 {
        if http_status_line(http, "/ready").contains(" 200") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(http_status_line(http, "/ready").contains(" 200"));
    assert_eq!(files(dir.path()), ["config.toml", "root.hints"]);
}
