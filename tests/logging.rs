//! Logs: the query log, its client privacy modes, and list refresh lines.
//!
//! Each test installs its own subscriber writing to a shared buffer for the
//! duration of the test. The tests run on a single-threaded runtime, so the
//! server's tasks log through that same subscriber.

use std::{
    io,
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use hickory_filter::{
    config::Config,
    logging::{QUERY_FIELDS, QUERY_TARGET, subscriber},
    server::{Running, start},
};
use hickory_server::proto::{
    op::{Message, Query},
    rr::{Name, RecordType},
    serialize::binary::BinEncodable,
};
use serde_json::Value;
use tokio::net::{TcpListener, UdpSocket};
use tracing_subscriber::fmt::MakeWriter;

mod common;
use common::{ok, serve};

/// A writer that collects everything logged.
#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Buffer {
    type Writer = Buffer;

    fn make_writer(&'a self) -> Buffer {
        self.clone()
    }
}

impl Buffer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }

    fn lines(&self) -> Vec<String> {
        self.text().lines().map(str::to_owned).collect()
    }

    /// The lines of a JSON log, parsed.
    fn json(&self) -> Vec<Value> {
        self.lines()
            .iter()
            .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{line}: {e}")))
            .collect()
    }

    /// The JSON lines carrying the query-log target.
    fn query_lines(&self) -> Vec<Value> {
        self.json()
            .into_iter()
            .filter(|line| line["target"] == QUERY_TARGET)
            .collect()
    }
}

/// A config with the endpoint off, an upstream nothing answers (these tests
/// only query names the filter answers itself), the given `[log]` lines and
/// any extra tables.
fn config(log: &str, tables: &str) -> Config {
    Config::parse(
        Path::new("test.toml"),
        &format!(
            "listen = [\"127.0.0.1:0\"]\nupstreams = [\"127.0.0.1:9\"]\nhttp = false\n\
             [filter]\nrules = [\"block domain ads.example\"]\n{tables}\n[log]\n{log}\n"
        ),
    )
    .expect("config")
}

/// Install a subscriber for this test, as `main` would from the config.
fn capture(config: &Config) -> (Buffer, tracing::subscriber::DefaultGuard) {
    let buffer = Buffer::default();
    let guard = tracing::subscriber::set_default(subscriber(&config.log, buffer.clone()));
    (buffer, guard)
}

async fn ask(server: SocketAddr, name: &str) {
    let mut message = Message::query();
    message.metadata.id = 9;
    message.metadata.recursion_desired = true;
    message.add_query(Query::query(
        Name::from_ascii(name).expect("name"),
        RecordType::A,
    ));
    let client = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    client
        .send_to(&message.to_bytes().expect("encode"), server)
        .await
        .expect("send");
    let mut buf = [0u8; 512];
    tokio::time::timeout(Duration::from_secs(5), client.recv_from(&mut buf))
        .await
        .expect("timed out waiting for an answer")
        .expect("recv");
}

async fn shutdown(running: Running) {
    running.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn with_the_query_log_off_no_query_line_is_logged_at_info() {
    let config = config("format = \"json\"", "");
    let (buffer, _guard) = capture(&config);
    let running = start(&config).await.expect("start");
    ask(running.addrs()[0], "ads.example.").await;
    shutdown(running).await;

    // Logging works (the rule set line is there) but no query is logged.
    assert!(!buffer.lines().is_empty(), "something was logged");
    assert!(buffer.query_lines().is_empty());
    assert!(!buffer.text().contains("ads.example"));
}

#[tokio::test]
async fn debug_level_logs_queries_even_without_the_query_log() {
    let config = config("level = \"debug\"\nformat = \"json\"", "");
    let (buffer, _guard) = capture(&config);
    let running = start(&config).await.expect("start");
    ask(running.addrs()[0], "ads.example.").await;
    shutdown(running).await;

    let lines = buffer.query_lines();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["level"], "DEBUG");
}

#[tokio::test]
async fn a_query_line_has_every_field_at_the_top_level() {
    let config = config("format = \"json\"\nquery_log = true", "");
    let (buffer, _guard) = capture(&config);
    let running = start(&config).await.expect("start");
    ask(running.addrs()[0], "ADS.example.").await;
    shutdown(running).await;

    let lines = buffer.query_lines();
    assert_eq!(lines.len(), 1, "one line for one query: {lines:?}");
    let line = &lines[0];
    for field in QUERY_FIELDS {
        assert!(line.get(*field).is_some(), "missing {field}: {line}");
    }
    assert_eq!(line["level"], "INFO");
    assert_eq!(line["client"], "127.0.0.1");
    assert_eq!(line["name"], "ads.example", "lower case, no trailing dot");
    assert_eq!(line["type"], "A");
    assert_eq!(line["outcome"], "blocked");
    assert_eq!(line["rcode"], "NOERROR");
    assert!(
        line["duration_ms"].as_f64().is_some_and(|ms| ms >= 0.0),
        "{line}"
    );
}

#[tokio::test]
async fn the_query_log_shows_even_at_a_quiet_level() {
    let config = config("level = \"warn\"\nformat = \"json\"\nquery_log = true", "");
    let (buffer, _guard) = capture(&config);
    let running = start(&config).await.expect("start");
    ask(running.addrs()[0], "ads.example.").await;
    shutdown(running).await;

    assert_eq!(buffer.query_lines().len(), 1);
    // Other info lines (the rule set replacement) stay quiet at `warn`.
    assert_eq!(buffer.json().len(), 1, "{:?}", buffer.lines());
}

#[tokio::test]
async fn a_masked_client_is_logged_without_its_host_part() {
    let config = config(
        "format = \"json\"\nquery_log = true\nquery_log_client = \"masked\"",
        "",
    );
    let (buffer, _guard) = capture(&config);
    let running = start(&config).await.expect("start");
    ask(running.addrs()[0], "ads.example.").await;
    shutdown(running).await;

    let lines = buffer.query_lines();
    assert_eq!(lines[0]["client"], "127.0.0.0");
}

#[tokio::test]
async fn an_omitted_client_has_no_client_field() {
    let config = config(
        "format = \"json\"\nquery_log = true\nquery_log_client = \"omit\"",
        "",
    );
    let (buffer, _guard) = capture(&config);
    let running = start(&config).await.expect("start");
    ask(running.addrs()[0], "ads.example.").await;
    shutdown(running).await;

    let lines = buffer.query_lines();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].get("client").is_none(), "{}", lines[0]);
    assert_eq!(lines[0]["name"], "ads.example");
}

#[tokio::test]
async fn query_lines_are_selected_by_target_apart_from_other_lines() {
    let list_server = serve(|_| ok("listed.example\n")).await;
    let tables = format!(
        "[[filter.lists]]\nrule = \"block list domain {}\"\n",
        list_server.url("/list.txt")
    );
    let config = config("format = \"json\"\nquery_log = true", &tables);
    let (buffer, _guard) = capture(&config);
    let running = start(&config).await.expect("start");
    ask(running.addrs()[0], "listed.example.").await;
    shutdown(running).await;

    let all = buffer.json();
    let queries = buffer.query_lines();
    assert_eq!(queries.len(), 1);
    assert!(
        all.len() > queries.len(),
        "list and rule set lines are logged too: {all:?}"
    );
    assert!(queries[0].get("client").is_some() && queries[0].get("name").is_some());
    for line in all.iter().filter(|line| line["target"] != QUERY_TARGET) {
        assert!(line.get("name").is_none(), "{line}");
    }
}

#[tokio::test]
async fn list_loads_are_logged_with_list_result_rules_and_skipped_lines() {
    let list_server = serve(|_| ok("listed.example\nbad name\nother.example\n")).await;
    let url = list_server.url("/list.txt");
    let tables = format!("[[filter.lists]]\nrule = \"block list domain {url}\"\n");
    let config = config("format = \"json\"", &tables);
    let (buffer, _guard) = capture(&config);
    let running = start(&config).await.expect("start");
    shutdown(running).await;

    let loaded = buffer
        .json()
        .into_iter()
        .find(|line| line["result"] == "loaded")
        .expect("a list load line");
    assert_eq!(loaded["level"], "INFO");
    assert_eq!(loaded["list"], url);
    assert_eq!(loaded["rules"], 2);
    assert_eq!(loaded["skipped"], 1);
}

#[tokio::test]
async fn a_failed_list_refresh_is_a_warning_naming_the_url_without_credentials() {
    // A URL carrying credentials, on a port nothing listens on.
    let dead = {
        let probe = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        probe.local_addr().expect("addr")
    };
    let credentialed = format!("http://user:secret@{dead}/list.txt");
    let tables = format!("[[filter.lists]]\nrule = \"block list domain {credentialed}\"\n");
    let config = config("format = \"json\"", &tables);
    let (buffer, _guard) = capture(&config);
    let running = start(&config).await.expect("an optional list may fail");
    shutdown(running).await;

    let warning = buffer
        .json()
        .into_iter()
        .find(|line| line["level"] == "WARN" && line["result"] == "failed")
        .expect("a warning for the failed list");
    assert_eq!(warning["list"], format!("http://{dead}/list.txt"));
    assert!(
        warning["reason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty()),
        "{warning}"
    );
    let text = buffer.text();
    assert!(
        !text.contains("secret"),
        "credentials never reach the log: {text}"
    );
    assert!(
        !text.contains("user:"),
        "credentials never reach the log: {text}"
    );
}

#[tokio::test]
async fn text_format_logs_a_readable_query_line() {
    let config = config("query_log = true", "");
    let (buffer, _guard) = capture(&config);
    let running = start(&config).await.expect("start");
    ask(running.addrs()[0], "ads.example.").await;
    shutdown(running).await;

    let line = buffer
        .lines()
        .into_iter()
        .find(|line| line.contains(QUERY_TARGET))
        .expect("a query line");
    for part in ["INFO", "127.0.0.1", "ads.example", "blocked", "NOERROR"] {
        assert!(line.contains(part), "{part}: {line}");
    }
}
