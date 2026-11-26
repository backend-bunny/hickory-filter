//! Wire-level checks of the filter handler: a real hickory server on loopback,
//! queried over UDP with hand-encoded messages.
//!
//! The upstream is a second server that answers every query with
//! `192.0.2.1` and records every name it is asked, so a forwarded answer is
//! told apart from a blocked one and "the upstream received nothing" can be
//! checked.

use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};

use hickory_filter::{
    config::{Response, ResponseMode},
    filter_handler::FilterHandler,
    rules::{RuleLine, parse_rule_line},
    ruleset::{LiveRules, RuleSet},
};
use hickory_server::{
    Server,
    net::runtime::Time,
    proto::{
        op::{Message, Metadata, Query, ResponseCode},
        rr::{LowerName, Name, RData, Record, RecordType, rdata::A, rdata::AAAA},
        serialize::binary::BinEncodable,
    },
    resolver::config::NameServerConfig,
    server::{Request, RequestHandler, ResponseHandler, ResponseInfo},
    store::forwarder::{ForwardConfig, ForwardZoneHandler},
    zone_handler::{Catalog, MessageResponseBuilder},
};
use tokio::net::UdpSocket;

const UPSTREAM_ANSWER: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 1);

/// Answers every query with one A record and remembers the names asked.
struct StubUpstream {
    seen: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl RequestHandler for StubUpstream {
    async fn handle_request<R: ResponseHandler, T: Time>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        let info = request.request_info().expect("single query");
        self.seen
            .lock()
            .expect("lock")
            .push(info.query.name().to_string());
        let record = Record::from_rdata(
            Name::from(info.query.name().clone()),
            30,
            RData::A(A(UPSTREAM_ANSWER)),
        );
        let metadata = Metadata::response_from_request(info.metadata);
        let response = MessageResponseBuilder::from_message_request(request).build(
            metadata,
            std::iter::once(&record),
            std::iter::empty(),
            std::iter::empty(),
            std::iter::empty(),
        );
        response_handle.send_response(response).await.expect("send")
    }
}

async fn spawn_server<T: RequestHandler>(handler: T) -> (SocketAddr, Server<T>) {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    let addr = socket.local_addr().expect("addr");
    let mut server = Server::new(handler);
    server.register_socket(socket);
    (addr, server)
}

/// A filter in front of a forwarder pointing at a recording stub upstream.
struct Stack {
    addr: SocketAddr,
    /// Every name the upstream was asked, in order.
    seen: Arc<Mutex<Vec<String>>>,
    live: Arc<LiveRules>,
    _upstream: Server<StubUpstream>,
    _filter: Server<FilterHandler>,
}

fn rule_set(lines: &[&str]) -> RuleSet {
    let rules: Vec<_> = lines
        .iter()
        .map(|line| match parse_rule_line(line).expect(line) {
            RuleLine::Rule(rule) => rule,
            other => panic!("{line}: {other:?}"),
        })
        .collect();
    RuleSet::from_rules(&rules).expect("build")
}

async fn stack(rules: &[&str], response: Response) -> Stack {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (upstream_addr, upstream) = spawn_server(StubUpstream {
        seen: Arc::clone(&seen),
    })
    .await;

    let mut name_server = NameServerConfig::udp(upstream_addr.ip());
    name_server.connections[0].port = upstream_addr.port();
    let forwarder = ForwardZoneHandler::builder_tokio(ForwardConfig {
        name_servers: vec![name_server],
        options: None,
    })
    .build()
    .expect("forwarder");
    let mut catalog = Catalog::new();
    catalog.upsert(LowerName::new(&Name::root()), vec![Arc::new(forwarder)]);

    let live = Arc::new(LiveRules::new(rule_set(rules)));
    let handler = FilterHandler::new(catalog, Arc::clone(&live), response);
    let (addr, filter) = spawn_server(handler).await;
    Stack {
        addr,
        seen,
        live,
        _upstream: upstream,
        _filter: filter,
    }
}

async fn ask(server: SocketAddr, name: &str, qtype: RecordType, rd: bool) -> Message {
    let mut message = Message::query();
    message.metadata.id = 0x1234;
    message.metadata.recursion_desired = rd;
    message.add_query(Query::query(Name::from_ascii(name).expect("name"), qtype));
    let client = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    client
        .send_to(&message.to_bytes().expect("encode"), server)
        .await
        .expect("send");
    let mut buf = [0u8; 512];
    let (len, _) = tokio::time::timeout(Duration::from_secs(5), client.recv_from(&mut buf))
        .await
        .expect("timed out waiting for a response")
        .expect("recv");
    Message::from_vec(&buf[..len]).expect("decode")
}

async fn query(server: SocketAddr, name: &str, qtype: RecordType) -> Message {
    ask(server, name, qtype, true).await
}

fn response(mode: ResponseMode) -> Response {
    Response {
        mode,
        ..Response::default()
    }
}

const BLOCK: &[&str] = &["block domain ads.example"];

#[tokio::test]
async fn null_mode_answers_with_the_unspecified_address_and_echoes_the_query() {
    let s = stack(BLOCK, Response::default()).await;

    let a = query(s.addr, "ads.example.", RecordType::A).await;
    assert_eq!(a.metadata.id, 0x1234, "the ID is echoed");
    assert!(a.metadata.recursion_desired, "RD is echoed");
    assert_eq!(a.metadata.response_code, ResponseCode::NoError);
    assert_eq!(a.queries.len(), 1, "the question is echoed");
    assert_eq!(
        a.queries[0].name(),
        &Name::from_ascii("ads.example.").unwrap()
    );
    assert_eq!(a.queries[0].query_type(), RecordType::A);
    assert_eq!(a.answers.len(), 1);
    assert_eq!(a.answers[0].data, RData::A(A(Ipv4Addr::UNSPECIFIED)));
    assert_eq!(a.answers[0].ttl, 3600);

    let aaaa = query(s.addr, "ads.example.", RecordType::AAAA).await;
    assert_eq!(aaaa.metadata.response_code, ResponseCode::NoError);
    assert_eq!(aaaa.answers.len(), 1);
    assert_eq!(
        aaaa.answers[0].data,
        RData::AAAA(AAAA(std::net::Ipv6Addr::UNSPECIFIED))
    );

    let mx = query(s.addr, "ads.example.", RecordType::MX).await;
    assert_eq!(mx.metadata.response_code, ResponseCode::NoError);
    assert!(mx.answers.is_empty(), "no A record for an MX query");

    assert!(
        s.seen.lock().unwrap().is_empty(),
        "the upstream saw nothing"
    );
}

#[tokio::test]
async fn a_blocked_query_without_rd_is_answered_and_a_forwarded_one_is_refused() {
    let s = stack(BLOCK, Response::default()).await;

    let blocked = ask(s.addr, "ads.example.", RecordType::A, false).await;
    assert_eq!(blocked.metadata.response_code, ResponseCode::NoError);
    assert!(!blocked.metadata.recursion_desired, "RD (unset) is echoed");
    assert_eq!(blocked.answers.len(), 1);

    // Existing forwarding behaviour (design D1): the catalog serves only
    // recursive queries.
    let forwarded = ask(s.addr, "ok.test.", RecordType::A, false).await;
    assert_eq!(forwarded.metadata.response_code, ResponseCode::Refused);
}

#[tokio::test]
async fn address_mode_answers_with_the_configured_addresses() {
    let config = Response {
        mode: ResponseMode::Address,
        ipv4: Some("192.0.2.80".parse().unwrap()),
        ipv6: Some("2001:db8::80".parse().unwrap()),
        ..Response::default()
    };
    let s = stack(BLOCK, config).await;

    let a = query(s.addr, "ads.example.", RecordType::A).await;
    assert_eq!(
        a.answers[0].data,
        RData::A(A("192.0.2.80".parse().unwrap()))
    );
    let aaaa = query(s.addr, "ads.example.", RecordType::AAAA).await;
    assert_eq!(
        aaaa.answers[0].data,
        RData::AAAA(AAAA("2001:db8::80".parse().unwrap()))
    );
    assert!(s.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn address_mode_with_a_missing_family_answers_noerror_with_no_records() {
    let config = Response {
        mode: ResponseMode::Address,
        ipv4: Some("192.0.2.80".parse().unwrap()),
        ..Response::default()
    };
    let s = stack(BLOCK, config).await;

    let aaaa = query(s.addr, "ads.example.", RecordType::AAAA).await;
    assert_eq!(aaaa.metadata.response_code, ResponseCode::NoError);
    assert!(aaaa.answers.is_empty());
    assert!(s.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn nodata_mode_answers_noerror_with_no_records() {
    let s = stack(BLOCK, response(ResponseMode::Nodata)).await;
    let a = query(s.addr, "ads.example.", RecordType::A).await;
    assert_eq!(a.metadata.response_code, ResponseCode::NoError);
    assert!(a.answers.is_empty());
    assert!(s.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn nxdomain_mode_answers_with_aa_and_an_soa_for_the_queried_name() {
    let s = stack(BLOCK, response(ResponseMode::Nxdomain)).await;
    let a = query(s.addr, "ads.example.", RecordType::A).await;
    assert_eq!(a.metadata.response_code, ResponseCode::NXDomain);
    assert!(a.metadata.authoritative, "AA must be set");
    assert!(a.answers.is_empty());
    assert_eq!(a.authorities.len(), 1);
    assert_eq!(a.authorities[0].record_type(), RecordType::SOA);
    assert_eq!(
        a.authorities[0].name,
        Name::from_ascii("ads.example.").unwrap()
    );
    assert_eq!(a.authorities[0].ttl, 3600);
    assert!(s.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_custom_ttl_is_used_for_the_blocked_answer() {
    let config = Response {
        ttl: 60,
        ..Response::default()
    };
    let s = stack(BLOCK, config).await;
    let a = query(s.addr, "ads.example.", RecordType::A).await;
    assert_eq!(a.answers[0].ttl, 60);
}

#[tokio::test]
async fn unblocked_names_are_forwarded_and_only_they_reach_the_upstream() {
    let s = stack(BLOCK, Response::default()).await;

    let blocked = query(s.addr, "ads.example.", RecordType::A).await;
    assert_eq!(blocked.answers[0].data, RData::A(A(Ipv4Addr::UNSPECIFIED)));

    let forwarded = query(s.addr, "ok.test.", RecordType::A).await;
    assert_eq!(forwarded.metadata.response_code, ResponseCode::NoError);
    assert_eq!(forwarded.answers.len(), 1);
    assert_eq!(
        forwarded.answers[0].data,
        RData::A(A(UPSTREAM_ANSWER)),
        "the forwarded answer comes from the upstream"
    );

    let seen = s.seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec!["ok.test.".to_owned()],
        "blocked names are never forwarded"
    );
}

#[tokio::test]
async fn allow_beats_block_and_names_match_case_insensitively() {
    let rules = [
        "block wildcard *.ads.example",
        "allow domain ok.ads.example",
    ];
    let s = stack(&rules, Response::default()).await;

    let blocked = query(s.addr, "BAD.Ads.Example.", RecordType::A).await;
    assert_eq!(blocked.answers[0].data, RData::A(A(Ipv4Addr::UNSPECIFIED)));

    let allowed = query(s.addr, "ok.ads.example.", RecordType::A).await;
    assert_eq!(allowed.answers[0].data, RData::A(A(UPSTREAM_ANSWER)));
    assert_eq!(
        s.seen.lock().unwrap().clone(),
        vec!["ok.ads.example.".to_owned()]
    );
}

#[tokio::test]
async fn a_refresh_takes_effect_for_the_next_query() {
    let s = stack(BLOCK, Response::default()).await;
    let before = query(s.addr, "news.example.", RecordType::A).await;
    assert_eq!(before.answers[0].data, RData::A(A(UPSTREAM_ANSWER)));

    s.live.swap(rule_set(&["block domain news.example"]));

    let after = query(s.addr, "news.example.", RecordType::A).await;
    assert_eq!(after.answers[0].data, RData::A(A(Ipv4Addr::UNSPECIFIED)));
    // And the old rule is gone with the old set.
    let ads = query(s.addr, "ads.example.", RecordType::A).await;
    assert_eq!(ads.answers[0].data, RData::A(A(UPSTREAM_ANSWER)));
}
