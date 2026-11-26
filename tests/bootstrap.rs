//! List host names resolve only through the bootstrap resolver.
//!
//! A stub DNS server on loopback answers every name with `127.0.0.1` and
//! records what it was asked, and a local HTTP server stands in for the list
//! host. The list URL uses a name that exists nowhere else, so the fetch can
//! only work through the stub.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use hickory_filter::{
    bootstrap::BootstrapResolver,
    config::Resolver,
    fetch::{FetchError, FetchOutcome, Fetcher},
    lists::Validators,
};
use hickory_server::{
    Server,
    net::runtime::Time,
    proto::{
        op::Metadata,
        rr::{Name, RData, Record, rdata::A},
    },
    server::{Request, RequestHandler, ResponseHandler, ResponseInfo},
    zone_handler::MessageResponseBuilder,
};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, UdpSocket},
};

mod common;
use common::{ok, serve};

/// Answers every query with `127.0.0.1` and records the names asked.
struct StubResolver {
    asked: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl RequestHandler for StubResolver {
    async fn handle_request<R: ResponseHandler, T: Time>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        let info = request.request_info().expect("single query");
        self.asked
            .lock()
            .unwrap()
            .push(info.query.name().to_string());
        let record = Record::from_rdata(
            Name::from(info.query.name().clone()),
            30,
            RData::A(A("127.0.0.1".parse().unwrap())),
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

struct Stub {
    addr: SocketAddr,
    asked: Arc<Mutex<Vec<String>>>,
    _server: Server<StubResolver>,
}

async fn stub_resolver() -> Stub {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    let addr = socket.local_addr().expect("addr");
    let mut server = Server::new(StubResolver {
        asked: Arc::clone(&asked),
    });
    server.register_socket(socket);
    Stub {
        addr,
        asked,
        _server: server,
    }
}

fn bootstrap(text: &str) -> Resolver {
    text.parse().expect("resolver")
}

fn fetcher_with_bootstrap(resolver: &Resolver) -> Fetcher {
    let config = hickory_filter::config::Config::parse(
        std::path::Path::new("test.toml"),
        &format!(
            "upstreams = [\"192.0.2.53\"]\n[filter]\nbootstrap = \"{}\"\n",
            match &resolver.tls_name {
                Some(name) => format!("tls://{}#{name}", resolver.addr),
                None => resolver.addr.to_string(),
            }
        ),
    )
    .expect("config");
    Fetcher::from_config(&config).expect("fetcher")
}

#[tokio::test]
async fn a_list_host_resolves_through_the_bootstrap_resolver_and_only_through_it() {
    let stub = stub_resolver().await;
    let list_server = serve(|_| ok("ads.example\n")).await;
    // `lists.example` resolves nowhere but the stub.
    let url = format!("http://lists.example:{}/list.txt", list_server.addr.port());

    let fetcher = fetcher_with_bootstrap(&bootstrap(&stub.addr.to_string()));
    let outcome = fetcher
        .fetch(&url, &Validators::default())
        .await
        .expect("fetch through the bootstrap resolver");

    assert!(matches!(outcome, FetchOutcome::Fetched { body, .. } if body == "ads.example\n"));
    let asked = stub.asked.lock().unwrap().clone();
    assert!(
        !asked.is_empty() && asked.iter().all(|name| name == "lists.example."),
        "the stub is asked for the list host, and only for it: {asked:?}"
    );
    assert_eq!(list_server.requests().len(), 1);
}

#[tokio::test]
async fn without_a_bootstrap_resolver_the_system_resolver_is_used() {
    let list_server = serve(|_| ok("ads.example\n")).await;
    // `.example` is reserved and never resolves: the system resolver can't
    // find this host, which shows the stub above was what made it work.
    let url = format!("http://lists.example:{}/list.txt", list_server.addr.port());
    let fetcher = Fetcher::new(Duration::from_secs(5), 1 << 20).expect("fetcher");

    let error = fetcher
        .fetch(&url, &Validators::default())
        .await
        .expect_err("the system resolver cannot resolve lists.example");
    assert!(matches!(error, FetchError::Request(_)), "{error:?}");
    assert!(list_server.requests().is_empty());
}

#[tokio::test]
async fn a_bootstrap_resolver_that_answers_nothing_fails_the_fetch() {
    // A port nothing listens on: every lookup fails, so the fetch fails and
    // nothing falls back to another resolver.
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    let dead = socket.local_addr().expect("addr");
    drop(socket);
    let list_server = serve(|_| ok("ads.example\n")).await;
    let url = format!("http://lists.example:{}/list.txt", list_server.addr.port());

    // A short fetch timeout keeps the test quick; the resolver's own retries
    // would otherwise run for several seconds.
    let resolver = BootstrapResolver::new(&bootstrap(&dead.to_string())).expect("resolver");
    let fetcher = Fetcher::with_resolver(Duration::from_secs(2), 1 << 20, Arc::new(resolver))
        .expect("fetcher");
    let error = fetcher
        .fetch(&url, &Validators::default())
        .await
        .expect_err("no resolver, no fetch");
    assert!(
        matches!(error, FetchError::Request(_) | FetchError::Timeout),
        "{error:?}"
    );
    assert!(list_server.requests().is_empty());
}

#[tokio::test]
async fn dns_over_tls_opens_a_tls_connection_not_plain_dns() {
    // Not a DNS server: it only records how the resolver opens the connection.
    // (The configured server name reaches certificate verification, which is
    // covered by the name-server mapping tests and checked against a real
    // resolver; the client sends no SNI, so it is not visible here.)
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let hello = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).await.expect("read");
        buf.truncate(n);
        buf
    });

    let resolver = bootstrap(&format!("tls://{addr}#dns.example"));
    let fetcher = fetcher_with_bootstrap(&resolver);
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        fetcher.fetch("http://lists.example/list.txt", &Validators::default()),
    )
    .await
    .expect("the fetch must not hang");
    assert!(result.is_err(), "the handshake cannot complete");

    let bytes = hello.await.expect("listener task");
    // A TLS handshake record (type 0x16, version 3.x) carrying a ClientHello
    // (handshake type 0x01), not a plain DNS query.
    assert_eq!(bytes.first(), Some(&0x16), "{bytes:02x?}");
    assert_eq!(bytes.get(1), Some(&0x03), "{bytes:02x?}");
    assert_eq!(bytes.get(5), Some(&0x01), "{bytes:02x?}");
}
