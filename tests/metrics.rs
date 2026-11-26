//! `/metrics` after real events: blocked, allowed, passed, cached and failed
//! queries, a changed and an unchanged refresh, and a failing list.
//!
//! The metrics recorder is process-global, so this file holds a single test
//! that records everything and compares scrapes before and after each step.

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use hickory_filter::{
    config::Config,
    metrics::TYPE_LABELS,
    server::{Running, start},
};
use hickory_server::{
    Server,
    net::runtime::Time,
    proto::{
        op::{Message, Metadata, Query, ResponseCode},
        rr::{Name, RData, Record, RecordType, rdata::A},
        serialize::binary::BinEncodable,
    },
    server::{Request, RequestHandler, ResponseHandler, ResponseInfo},
    zone_handler::MessageResponseBuilder,
};
use tokio::net::UdpSocket;

mod common;
use common::{Reply, serve};

/// Answers every query with one A record.
struct StubUpstream;

#[async_trait::async_trait]
impl RequestHandler for StubUpstream {
    async fn handle_request<R: ResponseHandler, T: Time>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        let info = request.request_info().expect("single query");
        let record = Record::from_rdata(
            Name::from(info.query.name().clone()),
            300,
            RData::A(A("192.0.2.1".parse().unwrap())),
        );
        let response = MessageResponseBuilder::from_message_request(request).build(
            Metadata::response_from_request(info.metadata),
            std::iter::once(&record),
            std::iter::empty(),
            std::iter::empty(),
            std::iter::empty(),
        );
        response_handle.send_response(response).await.expect("send")
    }
}

/// One scrape of `/metrics`, parsed.
struct Scrape {
    text: String,
    series: Vec<(String, BTreeMap<String, String>, f64)>,
}

impl Scrape {
    fn parse(text: String) -> Self {
        let mut series = Vec::new();
        for line in text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let (head, value) = line.rsplit_once(' ').expect("name and value");
            let value: f64 = value.parse().expect("a number");
            let (name, labels) = match head.split_once('{') {
                Some((name, rest)) => {
                    let inner = rest.strip_suffix('}').expect("closing brace");
                    let labels = inner
                        .split("\",")
                        .filter(|pair| !pair.is_empty())
                        .map(|pair| {
                            let (key, value) = pair.split_once("=\"").expect("key=\"value");
                            (key.to_owned(), value.trim_end_matches('"').to_owned())
                        })
                        .collect();
                    (name.to_owned(), labels)
                }
                None => (head.to_owned(), BTreeMap::new()),
            };
            series.push((name, labels, value));
        }
        Self { text, series }
    }

    /// The sum of every series of `name` whose labels include all of `labels`.
    fn value(&self, name: &str, labels: &[(&str, &str)]) -> f64 {
        self.series
            .iter()
            .filter(|(n, l, _)| {
                n == name
                    && labels
                        .iter()
                        .all(|(k, v)| l.get(*k).map(String::as_str) == Some(*v))
            })
            .map(|(_, _, value)| value)
            .sum()
    }

    fn has(&self, name: &str) -> bool {
        self.series.iter().any(|(n, _, _)| n == name)
    }
}

async fn scrape(http: SocketAddr) -> Scrape {
    let response = reqwest::get(format!("http://{http}/metrics"))
        .await
        .expect("scrape");
    assert_eq!(response.status(), 200);
    Scrape::parse(response.text().await.expect("body"))
}

/// Ask `name` for `record_type` over UDP; returns the answer and the client
/// address used.
async fn ask(server: SocketAddr, name: &str, record_type: RecordType) -> (Message, SocketAddr) {
    let mut message = Message::query();
    message.metadata.id = 7;
    message.metadata.recursion_desired = true;
    message.add_query(Query::query(
        Name::from_ascii(name).expect("name"),
        record_type,
    ));
    let client = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    let local = client.local_addr().expect("addr");
    client
        .send_to(&message.to_bytes().expect("encode"), server)
        .await
        .expect("send");
    let mut buf = [0u8; 512];
    let (len, _) = tokio::time::timeout(Duration::from_secs(20), client.recv_from(&mut buf))
        .await
        .expect("timed out waiting for an answer")
        .expect("recv");
    (Message::from_vec(&buf[..len]).expect("decode"), local)
}

/// Poll scrapes until `done` holds, for up to `limit`.
async fn wait_for(
    http: SocketAddr,
    limit: Duration,
    what: &str,
    done: impl Fn(&Scrape) -> bool,
) -> Scrape {
    let deadline = Instant::now() + limit;
    loop {
        let scrape = scrape(http).await;
        if done(&scrape) {
            return scrape;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// What the list server currently serves.
struct ListState {
    body: String,
    etag: String,
    down: bool,
}

fn config(upstream: SocketAddr, http: &str, tables: &str) -> Config {
    Config::parse(
        Path::new("test.toml"),
        &format!(
            "listen = [\"127.0.0.1:0\"]\nupstreams = [\"{upstream}\"]\nhttp = {http}\n{tables}\n"
        ),
    )
    .expect("config")
}

async fn shutdown(running: Running) {
    running.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn metrics_reflect_queries_refreshes_and_failures_without_identifying_anyone() {
    // An upstream, and a list server whose content and health the test drives.
    let upstream_socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    let upstream = upstream_socket.local_addr().expect("addr");
    let mut upstream_server = Server::new(StubUpstream);
    upstream_server.register_socket(upstream_socket);

    let list_state = Arc::new(Mutex::new(ListState {
        body: "listed.example\nbad name\n".to_owned(),
        etag: "\"v1\"".to_owned(),
        down: false,
    }));
    let shared = Arc::clone(&list_state);
    let list_server = serve(move |req| {
        let state = shared.lock().unwrap();
        if state.down {
            return Reply::Full {
                status: 503,
                headers: Vec::new(),
                body: Vec::new(),
            };
        }
        if req.headers.get("if-none-match") == Some(&state.etag) {
            return Reply::Full {
                status: 304,
                headers: Vec::new(),
                body: Vec::new(),
            };
        }
        Reply::Full {
            status: 200,
            headers: vec![("ETag", state.etag.clone())],
            body: state.body.clone().into_bytes(),
        }
    })
    .await;
    // The URL carries credentials, which must never reach a metric.
    let list_url = list_server.url("/list.txt");
    let credentialed = list_url.replacen("http://", "http://user:secret@", 1);
    let tables = format!(
        "[filter]\nrules = [\"block domain blocked.example\", \"allow domain allowed.example\"]\n\
         [[filter.lists]]\nrule = \"block list domain {credentialed}\"\nrefresh = \"1s\"\n"
    );

    let running = start(&config(upstream, "\"127.0.0.1:0\"", &tables))
        .await
        .expect("start");
    let dns = running.addrs()[0];
    let http = running.http_addr().expect("http endpoint");
    let mut clients: Vec<SocketAddr> = Vec::new();

    // --- queries of every outcome and several types ---
    let before = scrape(http).await;
    let mut query = async |name: &str, record_type| {
        let (answer, client) = ask(dns, name, record_type).await;
        clients.push(client);
        answer
    };
    query("blocked.example.", RecordType::A).await;
    query("blocked.example.", RecordType::AAAA).await;
    query("blocked.example.", RecordType::Unknown(65280)).await;
    query("blocked.example.", RecordType::Unknown(65281)).await;
    query("allowed.example.", RecordType::A).await;
    query("passed.example.", RecordType::A).await;
    query("passed.example.", RecordType::A).await; // from the response cache
    query("passed.example.", RecordType::MX).await;

    let after = scrape(http).await;
    let delta = |name: &str, labels: &[(&str, &str)]| {
        after.value(name, labels) - before.value(name, labels)
    };
    let queries = |outcome: &str, record_type: &str| {
        delta(
            "hickory_filter_queries_total",
            &[("outcome", outcome), ("type", record_type)],
        )
    };
    assert_eq!(queries("blocked", "A"), 1.0);
    assert_eq!(queries("blocked", "AAAA"), 1.0);
    assert_eq!(
        queries("blocked", "other"),
        2.0,
        "unusual types are all `other`"
    );
    assert_eq!(queries("allowed", "A"), 1.0);
    assert_eq!(queries("passed", "A"), 2.0);
    assert_eq!(queries("passed", "MX"), 1.0);
    for (outcome, count) in [("blocked", 4.0), ("allowed", 1.0), ("passed", 3.0)] {
        let name = "hickory_filter_query_duration_seconds_count";
        assert_eq!(delta(name, &[("outcome", outcome)]), count, "{outcome}");
    }
    assert!(after.has("hickory_filter_query_duration_seconds_bucket"));
    assert!(after.has("hickory_filter_query_duration_seconds_sum"));

    // Upstream and cache metrics come from the resolver library.
    assert!(
        delta("hickory_resolver_cache_hit_total", &[]) >= 1.0,
        "a cache hit"
    );
    assert!(
        delta("hickory_resolver_cache_miss_total", &[]) >= 1.0,
        "a cache miss"
    );
    assert!(
        delta("hickory_resolver_outgoing_queries_total", &[]) >= 1.0,
        "queries sent upstream"
    );

    // Labels stay bounded: only the fixed types and the four outcomes appear.
    for (name, labels, _) in &after.series {
        if name == "hickory_filter_queries_total" {
            assert!(TYPE_LABELS.contains(&labels["type"].as_str()), "{labels:?}");
            assert!(
                ["allowed", "blocked", "passed", "upstream_error"]
                    .contains(&labels["outcome"].as_str()),
                "{labels:?}"
            );
        }
    }

    // --- rule set and list gauges ---
    assert_eq!(
        after.value(
            "hickory_filter_rules",
            &[("action", "block"), ("kind", "domain")]
        ),
        2.0
    );
    assert_eq!(
        after.value(
            "hickory_filter_rules",
            &[("action", "allow"), ("kind", "domain")]
        ),
        1.0
    );
    assert!(after.value("hickory_filter_ruleset_swaps_total", &[]) >= 1.0);
    assert!(after.value("hickory_filter_ruleset_last_swap_timestamp_seconds", &[]) > 1.0e9);

    let list = [
        ("url", list_url.as_str()),
        ("action", "block"),
        ("type", "domain"),
    ];
    assert_eq!(after.value("hickory_filter_list_rules", &list), 1.0);
    assert_eq!(after.value("hickory_filter_list_skipped_lines", &list), 1.0);
    assert_eq!(
        after.value("hickory_filter_list_last_refresh_success", &list),
        1.0
    );
    let first_success = after.value("hickory_filter_list_last_success_timestamp_seconds", &list);
    assert!(first_success > 1.0e9);
    assert!(
        !after.text.contains("secret"),
        "credentials never reach a metric"
    );
    assert!(
        !after.text.contains("user:"),
        "credentials never reach a metric"
    );

    // --- a changed refresh swaps once; unchanged refreshes swap nothing ---
    let swaps_before = after.value("hickory_filter_ruleset_swaps_total", &[]);
    {
        let mut state = list_state.lock().unwrap();
        state.body = "listed.example\nmore.example\n".to_owned();
        state.etag = "\"v2\"".to_owned();
    }
    let changed = wait_for(http, Duration::from_secs(10), "the changed list", |s| {
        s.value("hickory_filter_list_rules", &list) == 2.0
    })
    .await;
    assert_eq!(
        changed.value("hickory_filter_ruleset_swaps_total", &[]),
        swaps_before + 1.0,
        "one replacement for the change"
    );
    assert_eq!(
        changed.value(
            "hickory_filter_rules",
            &[("action", "block"), ("kind", "domain")]
        ),
        3.0
    );
    // Further refreshes find the list unchanged (304) and replace nothing.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let unchanged = scrape(http).await;
    assert_eq!(
        unchanged.value("hickory_filter_ruleset_swaps_total", &[]),
        swaps_before + 1.0,
        "unchanged refreshes do not replace the rule set"
    );

    // --- a failing refresh shows as a failure and keeps the last success ---
    list_state.lock().unwrap().down = true;
    let failed = wait_for(http, Duration::from_secs(10), "the failed refresh", |s| {
        s.value("hickory_filter_list_last_refresh_success", &list) == 0.0
    })
    .await;
    let kept = failed.value("hickory_filter_list_last_success_timestamp_seconds", &list);
    assert!(kept >= first_success, "the last success is never lost");
    assert_eq!(
        failed.value("hickory_filter_list_rules", &list),
        2.0,
        "the rules are kept"
    );
    // More failed refreshes happen; none of them moves the last success.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let later = scrape(http).await;
    assert_eq!(
        later.value("hickory_filter_list_last_refresh_success", &list),
        0.0
    );
    assert_eq!(
        later.value("hickory_filter_list_last_success_timestamp_seconds", &list),
        kept,
        "failed refreshes leave the last success unchanged"
    );
    assert_eq!(
        later.value("hickory_filter_ruleset_swaps_total", &[]),
        unchanged.value("hickory_filter_ruleset_swaps_total", &[]),
        "and replace nothing"
    );

    // --- an unreachable upstream is counted as an upstream error ---
    let dead = {
        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
        socket.local_addr().expect("addr")
    };
    let failing = start(&config(dead, "false", ""))
        .await
        .expect("second server");
    let before_error = scrape(http).await;
    let (answer, client) = ask(failing.addrs()[0], "fail.example.", RecordType::A).await;
    clients.push(client);
    assert_eq!(answer.metadata.response_code, ResponseCode::ServFail);
    let after_error = scrape(http).await;
    assert_eq!(
        after_error.value(
            "hickory_filter_queries_total",
            &[("outcome", "upstream_error"), ("type", "A")]
        ) - before_error.value(
            "hickory_filter_queries_total",
            &[("outcome", "upstream_error"), ("type", "A")]
        ),
        1.0
    );

    assert!(
        after_error.value(
            "hickory_resolver_name_server_query_responses_total",
            &[("status", "timeout")]
        ) > before_error.value(
            "hickory_resolver_name_server_query_responses_total",
            &[("status", "timeout")]
        ),
        "an upstream that doesn't answer shows as timeouts"
    );

    // --- nobody is identified: no client address, no queried name, no client label ---
    let text = scrape(http).await.text;
    for client in &clients {
        assert!(
            !text.contains(&client.to_string()),
            "client {client} leaked"
        );
    }
    for name in [
        "blocked.example",
        "allowed.example",
        "passed.example",
        "fail.example",
    ] {
        assert!(!text.contains(name), "queried name {name} leaked");
    }
    assert!(!text.to_lowercase().contains("client"), "no client label");

    shutdown(failing).await;
    shutdown(running).await;
}
