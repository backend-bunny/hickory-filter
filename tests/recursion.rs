//! Recursive mode, end to end, against fake root, TLD and authoritative
//! servers in a network namespace (see `common::netns`).

use std::{net::SocketAddr, path::PathBuf};

use hickory_server::proto::{
    op::{Message, ResponseCode},
    rr::{RData, RecordType},
};

mod common;
use common::{
    authority::{self, Hierarchy},
    dns::ask_udp,
    netns,
    process::{Process, http_get, spawn},
};

/// A server in recursive mode starting from the fake root. DNSSEC is off:
/// the fake zones are unsigned, and the built-in trust anchor would reject them.
struct Server {
    process: Process,
    addr: SocketAddr,
    http: SocketAddr,
    _dir: tempfile::TempDir,
}

impl Server {
    /// `top` is extra top-level configuration, `recursion` extra `[recursion]`
    /// settings and `tables` further tables, written after `[recursion]`.
    fn start(top: &str, recursion: &str, tables: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let hints = authority::write_hints(dir.path());
        let file: PathBuf = dir.path().join("config.toml");
        std::fs::write(
            &file,
            format!(
                "mode = \"recursive\"\nlisten = [\"127.0.0.1:0\"]\nhttp = \"127.0.0.1:0\"\n{top}\n\
                 [recursion]\nroots = \"{}\"\ndnssec = \"off\"\n{recursion}\n{tables}\n",
                hints.display()
            ),
        )
        .expect("write config");
        let process = spawn(&file);
        // The listeners are announced before the endpoint.
        let addr = process.listening_addr();
        let http = process.http_addr();
        Self {
            process,
            addr,
            http,
            _dir: dir,
        }
    }

    /// The text of `/metrics`.
    fn metrics(&self) -> String {
        let response = http_get(self.http, "/metrics");
        response.split("\r\n\r\n").nth(1).unwrap_or("").to_owned()
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

    let blocked = server.ask("ads.example.test.").await;
    assert_eq!(blocked.metadata.response_code, ResponseCode::NoError);
    assert_eq!(
        a_records(&blocked),
        vec![std::net::Ipv4Addr::UNSPECIFIED],
        "the null block answer"
    );
    assert_eq!(hierarchy.queries_received(), 0, "no name server was asked");

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
