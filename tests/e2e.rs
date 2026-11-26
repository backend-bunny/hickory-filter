//! End to end: the real binary, started from the shipped example configuration
//! with only its addresses pointed at local stand-ins, driven over the wire.
//!
//! Each test is one scenario per capability of the design: forwarding, filter
//! rules, list sources and refresh, blocked responses and observability.

use std::{
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use hickory_filter::cache;
use hickory_server::proto::{
    op::{Message, ResponseCode},
    rr::{
        RData, RecordType,
        rdata::{A, AAAA},
    },
};
use tokio::net::UdpSocket;

mod common;
use common::{
    Reply, TestServer,
    dns::{StubDns, ask_tcp, ask_udp},
    process::{Process, http_get, spawn},
    serve,
};

/// What the list server serves.
struct ListState {
    body: String,
    etag: String,
    down: bool,
}

/// Everything around the binary: the stand-ins and the process.
struct Env {
    upstream: StubDns,
    bootstrap: StubDns,
    list_server: TestServer,
    list_state: Arc<Mutex<ListState>>,
    list_url: String,
    cache_dir: PathBuf,
    process: Process,
    dns: SocketAddr,
    http: SocketAddr,
    _dir: tempfile::TempDir,
}

/// The example configuration with its addresses replaced by local ones. Every
/// replaced line must still be in the example, so a changed example fails here.
fn render_example(
    dir: &Path,
    upstream: SocketAddr,
    bootstrap: SocketAddr,
    list_port: u16,
    cache_dir: &Path,
    mode: &str,
) -> PathBuf {
    let mut config = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/hickory-filter.toml"
    ))
    .expect("the example config");
    for (from, to) in [
        (
            r#"listen = ["0.0.0.0:1053", "[::]:1053"]"#,
            r#"listen = ["127.0.0.1:0"]"#.to_owned(),
        ),
        (
            r#"upstreams = ["tls://192.0.2.53#dns.example", "192.0.2.54"]"#,
            format!(r#"upstreams = ["{upstream}"]"#),
        ),
        (
            r#"http = "0.0.0.0:8080""#,
            r#"http = "127.0.0.1:0""#.to_owned(),
        ),
        (
            r#"bootstrap = "tls://192.0.2.53#dns.example""#,
            format!(r#"bootstrap = "{bootstrap}""#),
        ),
        (
            "https://lists.example/hosts.txt",
            format!("http://lists.example:{list_port}/hosts.txt"),
        ),
        (r#"refresh = "6h""#, r#"refresh = "1s""#.to_owned()),
        (
            r#"# cache_dir = "/var/cache/hickory-filter""#,
            format!(r#"cache_dir = "{}""#, cache_dir.display()),
        ),
        (r#"mode = "null""#, format!(r#"mode = "{mode}""#)),
    ] {
        assert!(
            config.contains(from),
            "the example config no longer has {from:?}"
        );
        config = config.replacen(from, &to, 1);
    }
    let path = dir.join("config.toml");
    std::fs::write(&path, config).expect("write config");
    path
}

impl Env {
    /// Start the stand-ins and the binary. `upstream_alive: false` points the
    /// configured upstream at a port nothing listens on.
    async fn start(mode: &str, upstream_alive: bool) -> Env {
        let upstream = StubDns::start(Ipv4Addr::new(192, 0, 2, 1)).await;
        // Resolves every name, including the list host, to loopback.
        let bootstrap = StubDns::start(Ipv4Addr::LOCALHOST).await;
        let list_state = Arc::new(Mutex::new(ListState {
            body: "0.0.0.0 listed.example\n".to_owned(),
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
        let list_url = format!("http://lists.example:{}/hosts.txt", list_server.addr.port());

        let dir = tempfile::tempdir().expect("tempdir");
        let cache_dir = dir.path().join("cache");
        let upstream_addr = if upstream_alive {
            upstream.addr
        } else {
            let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
            socket.local_addr().expect("addr")
        };
        let config = render_example(
            dir.path(),
            upstream_addr,
            bootstrap.addr,
            list_server.addr.port(),
            &cache_dir,
            mode,
        );
        let process = spawn(&config);
        let (dns, http) =
            tokio::task::block_in_place(|| (process.listening_addr(), process.http_addr()));
        Env {
            upstream,
            bootstrap,
            list_server,
            list_state,
            list_url,
            cache_dir,
            process,
            dns,
            http,
            _dir: dir,
        }
    }

    async fn metrics(&self) -> String {
        reqwest::get(format!("http://{}/metrics", self.http))
            .await
            .expect("scrape")
            .text()
            .await
            .expect("body")
    }
}

fn answers_a(message: &Message, address: Ipv4Addr) -> bool {
    message.metadata.response_code == ResponseCode::NoError
        && message.answers.len() == 1
        && message.answers[0].data == RData::A(A(address))
}

const NULL_A: Ipv4Addr = Ipv4Addr::UNSPECIFIED;
const UPSTREAM_A: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 1);

/// Ask until `name` answers `address`, for up to `limit`.
async fn wait_for_answer(env: &Env, name: &str, address: Ipv4Addr, limit: Duration) {
    let deadline = Instant::now() + limit;
    loop {
        let answer = ask_udp(env.dns, name, RecordType::A).await;
        if answers_a(&answer, address) {
            return;
        }
        assert!(Instant::now() < deadline, "{name} never answered {address}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_example_configuration_serves_every_capability_over_the_wire() {
    let mut env = Env::start("null", true).await;

    // --- observability: ready and healthy once the lists are loaded ---
    let ready = reqwest::get(format!("http://{}/ready", env.http))
        .await
        .expect("ready");
    assert_eq!(ready.status(), 200);
    let health = reqwest::get(format!("http://{}/health", env.http))
        .await
        .expect("health");
    assert_eq!(health.status(), 200);

    // --- blocklist sources: the list from the example is loaded, and its host
    //     was resolved only through the bootstrap resolver ---
    let listed = ask_udp(env.dns, "listed.example.", RecordType::A).await;
    assert!(answers_a(&listed, NULL_A), "{listed:?}");
    assert!(
        env.bootstrap
            .asked()
            .iter()
            .any(|name| name == "lists.example."),
        "the list host went through the bootstrap resolver: {:?}",
        env.bootstrap.asked()
    );
    assert!(
        !env.upstream
            .asked()
            .iter()
            .any(|name| name == "lists.example."),
        "and never through the upstream"
    );
    let host = format!("lists.example:{}", env.list_server.addr.port());
    assert!(
        env.list_server
            .requests()
            .iter()
            .any(|request| request.headers.get("host") == Some(&host)),
        "the list server was reached under the name the bootstrap resolver handled"
    );

    // --- filter rules: wildcard, case, and allow beating block ---
    for name in [
        "x.ads.example.",
        "ads.example.",
        "ADS.Example.",
        "deep.x.ads.example.",
    ] {
        let answer = ask_udp(env.dns, name, RecordType::A).await;
        assert!(answers_a(&answer, NULL_A), "{name} is blocked: {answer:?}");
    }
    let allowed = ask_udp(env.dns, "ok.ads.example.", RecordType::A).await;
    assert!(
        answers_a(&allowed, UPSTREAM_A),
        "allow beats the wildcard: {allowed:?}"
    );
    assert!(
        env.upstream
            .asked()
            .iter()
            .any(|name| name == "ok.ads.example.")
    );

    // --- forwarding: unblocked names reach the upstream, over UDP and TCP ---
    let news = ask_udp(env.dns, "news.example.", RecordType::A).await;
    assert!(answers_a(&news, UPSTREAM_A), "{news:?}");
    let over_tcp = ask_tcp(env.dns, "other.example.", RecordType::A).await;
    assert!(answers_a(&over_tcp, UPSTREAM_A), "{over_tcp:?}");
    assert!(
        !env.upstream
            .asked()
            .iter()
            .any(|name| name.contains("ads.example") && name != "ok.ads.example."),
        "blocked names never reach the upstream: {:?}",
        env.upstream.asked()
    );

    // --- blocked responses (null mode from the example) ---
    let aaaa = ask_udp(env.dns, "x.ads.example.", RecordType::AAAA).await;
    assert_eq!(aaaa.answers.len(), 1);
    assert_eq!(
        aaaa.answers[0].data,
        RData::AAAA(AAAA(std::net::Ipv6Addr::UNSPECIFIED))
    );
    let mx = ask_udp(env.dns, "x.ads.example.", RecordType::MX).await;
    assert_eq!(mx.metadata.response_code, ResponseCode::NoError);
    assert!(mx.answers.is_empty());

    // --- list refresh: new content takes effect without a restart ---
    assert!(!answers_a(
        &ask_udp(env.dns, "newly.example.", RecordType::A).await,
        NULL_A
    ));
    {
        let mut state = env.list_state.lock().unwrap();
        state.body = "0.0.0.0 listed.example\n0.0.0.0 newly.example\n".to_owned();
        state.etag = "\"v2\"".to_owned();
    }
    wait_for_answer(&env, "newly.example.", NULL_A, Duration::from_secs(15)).await;

    // --- the cache: the fetched list is on disk with its validators ---
    let cached = cache::read(&env.cache_dir, &env.list_url).expect("the list is cached");
    assert!(cached.body.contains("newly.example"));
    assert_eq!(cached.validators.etag.as_deref(), Some("\"v2\""));

    // --- last good copy: a failing list server doesn't unblock anything ---
    env.list_state.lock().unwrap().down = true;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    for name in ["listed.example.", "newly.example."] {
        let answer = ask_udp(env.dns, name, RecordType::A).await;
        assert!(
            answers_a(&answer, NULL_A),
            "{name} stays blocked: {answer:?}"
        );
    }

    // --- observability: the metrics saw all of it ---
    let metrics = env.metrics().await;
    for expected in [
        r#"hickory_filter_queries_total{outcome="blocked""#,
        r#"hickory_filter_queries_total{outcome="allowed""#,
        r#"hickory_filter_queries_total{outcome="passed""#,
        "hickory_filter_ruleset_swaps_total",
    ] {
        assert!(
            metrics.contains(expected),
            "missing {expected} in:\n{metrics}"
        );
    }
    let failed_list = format!(
        r#"hickory_filter_list_last_refresh_success{{action="block",type="hosts",url="{}"}} 0"#,
        env.list_url
    );
    assert!(
        metrics.contains(&failed_list),
        "the failed refresh shows:\n{metrics}"
    );
    assert!(
        !metrics.to_lowercase().contains("client"),
        "no client labels"
    );
    let root = http_get(env.http, "/nope");
    assert!(root.starts_with("HTTP/1.1 404"), "{root}");

    // --- lifecycle: SIGTERM exits 0 within the grace period ---
    env.process.signal("TERM");
    let (status, took) =
        tokio::task::block_in_place(|| env.process.wait_exit(Duration::from_secs(15)));
    assert_eq!(status.code(), Some(0), "{status:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nxdomain_mode_answers_blocked_names_with_aa_and_an_soa() {
    let env = Env::start("nxdomain", true).await;

    let answer = ask_udp(env.dns, "x.ads.example.", RecordType::A).await;
    assert_eq!(answer.metadata.response_code, ResponseCode::NXDomain);
    assert!(answer.metadata.authoritative, "AA must be set");
    assert!(answer.answers.is_empty());
    assert_eq!(answer.authorities.len(), 1);
    assert_eq!(answer.authorities[0].record_type(), RecordType::SOA);

    // Unblocked names are unaffected by the mode.
    let news = ask_udp(env.dns, "news.example.", RecordType::A).await;
    assert!(answers_a(&news, UPSTREAM_A), "{news:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dead_upstreams_give_servfail_while_blocked_names_still_answer() {
    let env = Env::start("null", false).await;

    let failed = ask_udp(env.dns, "news.example.", RecordType::A).await;
    assert_eq!(failed.metadata.response_code, ResponseCode::ServFail);
    assert!(
        failed.answers.is_empty(),
        "never a blocked-style answer: {failed:?}"
    );

    let blocked = ask_udp(env.dns, "x.ads.example.", RecordType::A).await;
    assert!(answers_a(&blocked, NULL_A), "{blocked:?}");
    let metrics = env.metrics().await;
    assert!(
        metrics.contains(r#"hickory_filter_queries_total{outcome="upstream_error""#),
        "{metrics}"
    );
}
