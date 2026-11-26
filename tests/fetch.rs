//! Fetching remote lists, against a small HTTP server on loopback.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use hickory_filter::{
    config::Config,
    fetch::{FetchError, FetchOutcome, Fetcher, load_remote, refresh_remote},
    lists::{LastResult, ListSet, Validators},
    rules::Matcher,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[derive(Clone, Debug)]
struct Req {
    path: String,
    headers: HashMap<String, String>,
}

enum Reply {
    Full {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    },
    /// Wait this long, then answer 200 with an empty body.
    Stall(Duration),
    /// A 200 with no length that never ends.
    Endless,
}

fn ok(body: &str) -> Reply {
    Reply::Full {
        status: 200,
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    }
}

fn status(code: u16) -> Reply {
    Reply::Full {
        status: code,
        headers: Vec::new(),
        body: Vec::new(),
    }
}

fn redirect(location: &str) -> Reply {
    Reply::Full {
        status: 302,
        headers: vec![("Location", location.to_owned())],
        body: Vec::new(),
    }
}

struct TestServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<Req>>>,
}

impl TestServer {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn requests(&self) -> Vec<Req> {
        self.requests.lock().unwrap().clone()
    }
}

async fn serve<F>(handler: F) -> TestServer
where
    F: Fn(&Req) -> Reply + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(handler);
    let seen = Arc::clone(&requests);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let handler = Arc::clone(&handler);
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                let _ = handle(stream, handler, seen).await;
            });
        }
    });
    TestServer { addr, requests }
}

async fn handle(
    mut stream: TcpStream,
    handler: Arc<dyn Fn(&Req) -> Reply + Send + Sync>,
    seen: Arc<Mutex<Vec<Req>>>,
) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let mut lines = head.split("\r\n");
    let path = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let req = Req { path, headers };
    seen.lock().unwrap().push(req.clone());

    match handler(&req) {
        Reply::Full {
            status,
            headers,
            body,
        } => {
            let mut response = format!("HTTP/1.1 {status} X\r\nConnection: close\r\n");
            if status != 304 {
                response.push_str(&format!("Content-Length: {}\r\n", body.len()));
            }
            for (name, value) in headers {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("\r\n");
            stream.write_all(response.as_bytes()).await?;
            stream.write_all(&body).await?;
            stream.shutdown().await
        }
        Reply::Stall(delay) => {
            tokio::time::sleep(delay).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                .await
        }
        Reply::Endless => {
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                .await?;
            // Until the client gives up and the write fails.
            loop {
                stream.write_all(&[b'a'; 8192]).await?;
            }
        }
    }
}

fn fetcher() -> Fetcher {
    Fetcher::new(Duration::from_secs(5), 1 << 20).expect("fetcher")
}

fn no_validators() -> Validators {
    Validators::default()
}

#[tokio::test]
async fn a_200_returns_the_body_and_the_validators() {
    let server = serve(|_| Reply::Full {
        status: 200,
        headers: vec![
            ("ETag", "\"v1\"".to_owned()),
            ("Last-Modified", "Wed, 21 Oct 2026 07:28:00 GMT".to_owned()),
        ],
        body: b"ads.example\n".to_vec(),
    })
    .await;

    let outcome = fetcher()
        .fetch(&server.url("/list.txt"), &no_validators())
        .await
        .expect("fetch");
    assert_eq!(
        outcome,
        FetchOutcome::Fetched {
            body: "ads.example\n".to_owned(),
            validators: Validators {
                etag: Some("\"v1\"".to_owned()),
                last_modified: Some("Wed, 21 Oct 2026 07:28:00 GMT".to_owned()),
            }
        }
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/list.txt");
    assert!(!requests[0].headers.contains_key("if-none-match"));
    assert!(!requests[0].headers.contains_key("if-modified-since"));
}

#[tokio::test]
async fn validators_are_sent_and_a_304_is_not_modified() {
    let server = serve(|req| {
        if req.headers.get("if-none-match").map(String::as_str) == Some("\"v1\"") {
            status(304)
        } else {
            ok("changed.example\n")
        }
    })
    .await;
    let validators = Validators {
        etag: Some("\"v1\"".to_owned()),
        last_modified: Some("Wed, 21 Oct 2026 07:28:00 GMT".to_owned()),
    };

    let outcome = fetcher()
        .fetch(&server.url("/list.txt"), &validators)
        .await
        .expect("fetch");
    assert_eq!(outcome, FetchOutcome::NotModified);
    let request = &server.requests()[0];
    assert_eq!(request.headers["if-none-match"], "\"v1\"");
    assert_eq!(
        request.headers["if-modified-since"],
        "Wed, 21 Oct 2026 07:28:00 GMT"
    );
}

#[tokio::test]
async fn a_503_is_a_status_error() {
    let server = serve(|_| status(503)).await;
    let error = fetcher()
        .fetch(&server.url("/list.txt"), &no_validators())
        .await
        .expect_err("503");
    assert_eq!(error, FetchError::Status(503));
}

#[tokio::test]
async fn a_slow_server_times_out() {
    let server = serve(|_| Reply::Stall(Duration::from_secs(5))).await;
    let fetcher = Fetcher::new(Duration::from_millis(200), 1 << 20).expect("fetcher");
    let error = fetcher
        .fetch(&server.url("/list.txt"), &no_validators())
        .await
        .expect_err("timeout");
    assert_eq!(error, FetchError::Timeout);
}

#[tokio::test]
async fn an_oversized_body_fails_by_content_length() {
    let server = serve(|_| ok(&"a".repeat(500))).await;
    let fetcher = Fetcher::new(Duration::from_secs(5), 100).expect("fetcher");
    let error = fetcher
        .fetch(&server.url("/list.txt"), &no_validators())
        .await
        .expect_err("too large");
    assert_eq!(error, FetchError::TooLarge { limit: 100 });
}

#[tokio::test]
async fn a_body_that_grows_past_the_limit_is_abandoned() {
    let server = serve(|_| Reply::Endless).await;
    let fetcher = Fetcher::new(Duration::from_secs(5), 100_000).expect("fetcher");
    let started = std::time::Instant::now();
    let error = fetcher
        .fetch(&server.url("/list.txt"), &no_validators())
        .await
        .expect_err("too large");
    assert_eq!(error, FetchError::TooLarge { limit: 100_000 });
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "it must not read forever"
    );
}

#[tokio::test]
async fn an_http_redirect_to_http_is_followed() {
    let server = serve(|req| match req.path.as_str() {
        "/old" => redirect("/new"),
        "/new" => ok("moved.example\n"),
        _ => status(404),
    })
    .await;
    let outcome = fetcher()
        .fetch(&server.url("/old"), &no_validators())
        .await
        .expect("fetch");
    assert!(matches!(outcome, FetchOutcome::Fetched { body, .. } if body == "moved.example\n"));
}

#[tokio::test]
async fn a_redirect_to_another_scheme_is_rejected() {
    let server = serve(|_| redirect("ftp://lists.example/a.txt")).await;
    let error = fetcher()
        .fetch(&server.url("/old"), &no_validators())
        .await
        .expect_err("rejected");
    assert!(
        matches!(&error, FetchError::Request(reason) if reason.contains("not allowed")),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_redirect_loop_stops() {
    let server = serve(|_| redirect("/again")).await;
    let error = fetcher()
        .fetch(&server.url("/again"), &no_validators())
        .await
        .expect_err("loop");
    assert!(matches!(error, FetchError::Request(_)), "{error:?}");
}

// --- lists: the last good copy and the bookkeeping ---

fn list_set(url: &str, extra: &str) -> ListSet {
    let config = Config::parse(
        Path::new("test.toml"),
        &format!(
            "upstreams = [\"192.0.2.53\"]\n[[filter.lists]]\nrule = \"block list domain {url}\"\n{extra}\n"
        ),
    )
    .expect("config");
    ListSet::from_config(&config).expect("lists")
}

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_000)
}

fn domains(set: &ListSet) -> Vec<String> {
    set.rules()
        .map(|rule| match &rule.matcher {
            Matcher::Domain(name) => name.clone(),
            other => panic!("{other:?}"),
        })
        .collect()
}

#[tokio::test]
async fn a_remote_list_loads_with_skipped_lines_counted_and_validators_kept() {
    let server = serve(|_| Reply::Full {
        status: 200,
        headers: vec![("ETag", "\"v1\"".to_owned())],
        body: b"# header\nads.example\nbad name\ntrack.example\n".to_vec(),
    })
    .await;
    let mut set = list_set(&server.url("/list.txt"), "");

    load_remote(&fetcher(), &mut set, now()).await;

    let list = &set.lists()[0];
    assert_eq!(list.last_result, LastResult::Loaded);
    assert_eq!(list.skipped, 1);
    assert_eq!(list.validators.etag.as_deref(), Some("\"v1\""));
    assert_eq!(domains(&set), vec!["ads.example", "track.example"]);
}

#[tokio::test]
async fn a_server_that_goes_down_keeps_the_last_good_rules() {
    let healthy = Arc::new(Mutex::new(true));
    let flag = Arc::clone(&healthy);
    let server = serve(move |_| {
        if *flag.lock().unwrap() {
            ok("ads.example\n")
        } else {
            status(503)
        }
    })
    .await;
    let mut set = list_set(&server.url("/list.txt"), "");
    let fetcher = fetcher();
    load_remote(&fetcher, &mut set, now()).await;

    *healthy.lock().unwrap() = false;
    let later = now() + Duration::from_secs(3600);
    load_remote(&fetcher, &mut set, later).await;

    let list = &set.lists()[0];
    match &list.last_result {
        LastResult::Failed(reason) => {
            assert!(reason.contains("503"), "{reason}");
            assert!(reason.contains(&server.url("/list.txt")), "{reason}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(list.last_success, Some(now()), "the last success is kept");
    assert_eq!(
        domains(&set),
        vec!["ads.example"],
        "ads.example stays blocked"
    );
}

#[tokio::test]
async fn a_304_keeps_the_rules_without_reparsing_and_counts_as_a_success() {
    let server = serve(|req| {
        if req.headers.contains_key("if-none-match") {
            status(304)
        } else {
            Reply::Full {
                status: 200,
                headers: vec![("ETag", "\"v1\"".to_owned())],
                body: b"ads.example\n".to_vec(),
            }
        }
    })
    .await;
    let mut set = list_set(&server.url("/list.txt"), "");
    let fetcher = fetcher();
    load_remote(&fetcher, &mut set, now()).await;

    let later = now() + Duration::from_secs(3600);
    load_remote(&fetcher, &mut set, later).await;

    let list = &set.lists()[0];
    assert_eq!(list.last_result, LastResult::NotModified);
    assert_eq!(list.last_success, Some(later));
    assert_eq!(domains(&set), vec!["ads.example"]);
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn a_changed_list_replaces_the_rules_whole() {
    let version = Arc::new(Mutex::new(1));
    let current = Arc::clone(&version);
    let server = serve(move |_| {
        if *current.lock().unwrap() == 1 {
            ok("old.example\n")
        } else {
            ok("new.example\nmore.example\n")
        }
    })
    .await;
    let mut set = list_set(&server.url("/list.txt"), "");
    let fetcher = fetcher();
    load_remote(&fetcher, &mut set, now()).await;
    *version.lock().unwrap() = 2;
    load_remote(&fetcher, &mut set, now() + Duration::from_secs(60)).await;
    assert_eq!(domains(&set), vec!["new.example", "more.example"]);
}

#[tokio::test]
async fn a_304_with_no_previous_copy_is_a_failure() {
    let server = serve(|_| status(304)).await;
    let mut set = list_set(&server.url("/list.txt"), "");
    load_remote(&fetcher(), &mut set, now()).await;

    let list = &set.lists()[0];
    assert!(!list.has_loaded());
    assert!(matches!(&list.last_result, LastResult::Failed(r) if r.contains("no previous copy")));
}

/// A URL on a port nothing listens on.
async fn dead_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    format!("http://{addr}/list.txt")
}

#[tokio::test]
async fn an_unreachable_optional_list_starts_and_a_required_one_fails_startup() {
    let url = dead_url().await;

    let mut optional = list_set(&url, "");
    load_remote(&fetcher(), &mut optional, now()).await;
    assert!(optional.check_required().is_ok());
    assert!(matches!(
        optional.lists()[0].last_result,
        LastResult::Failed(_)
    ));

    let mut required = list_set(&url, "required = true");
    load_remote(&fetcher(), &mut required, now()).await;
    let error = required.check_required().expect_err("startup must fail");
    assert!(error.to_string().contains(&url), "{error}");
}

#[tokio::test]
async fn refreshing_a_file_list_through_the_fetcher_does_nothing() {
    let mut set = list_set("file:///nonexistent/list.txt", "");
    refresh_remote(&fetcher(), &mut set.lists_mut()[0], now()).await;
    assert_eq!(set.lists()[0].last_result, LastResult::NotAttempted);
}
