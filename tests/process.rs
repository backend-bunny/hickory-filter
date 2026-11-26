//! The real binary: it serves, exits 0 on SIGTERM and SIGINT within the grace
//! period, and fails startup with the address when it can't bind.

use std::{
    net::{SocketAddr, TcpListener, UdpSocket},
    path::Path,
    time::Duration,
};

mod common;
use common::process::{http_status_line, spawn};

use hickory_server::proto::{
    op::{Message, Query},
    rr::{Name, RData, RecordType, rdata::A},
    serialize::binary::BinEncodable,
};

fn write_config(dir: &Path, listen: &str) -> std::path::PathBuf {
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        format!(
            "listen = [\"{listen}\"]\nupstreams = [\"127.0.0.1:9\"]\nhttp = \"127.0.0.1:0\"\n[filter]\nrules = [\"block domain ads.example\"]\n"
        ),
    )
    .expect("write config");
    path
}

fn blocked_answer(server: SocketAddr) -> bool {
    let mut message = Message::query();
    message.metadata.id = 1;
    message.metadata.recursion_desired = true;
    message.add_query(Query::query(
        Name::from_ascii("ads.example.").expect("name"),
        RecordType::A,
    ));
    let client = UdpSocket::bind("127.0.0.1:0").expect("bind");
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    client
        .send_to(&message.to_bytes().expect("encode"), server)
        .expect("send");
    let mut buf = [0u8; 512];
    let (len, _) = client.recv_from(&mut buf).expect("an answer");
    let answer = Message::from_vec(&buf[..len]).expect("decode");
    answer.answers.len() == 1 && answer.answers[0].data == RData::A(A("0.0.0.0".parse().unwrap()))
}

fn exits_zero_on(signal: &str) {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = write_config(dir.path(), "127.0.0.1:0");
    let mut process = spawn(&config);
    let addr = process.listening_addr();
    assert!(blocked_answer(addr), "the binary serves");
    let http = process.http_addr();
    assert!(http_status_line(http, "/health").contains("200"));
    assert!(http_status_line(http, "/ready").contains("200"));
    assert!(http_status_line(http, "/metrics").contains("200"));

    process.signal(signal);
    let (status, took) = process.wait_exit(Duration::from_secs(15));
    assert_eq!(status.code(), Some(0), "{status:?}");
    assert!(
        took < Duration::from_secs(5),
        "within the grace period: {took:?}"
    );
}

#[test]
fn json_logs_are_one_object_per_line_and_the_query_log_is_off() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        "listen = [\"127.0.0.1:0\"]\nupstreams = [\"127.0.0.1:9\"]\nhttp = \"127.0.0.1:0\"\n\
         [filter]\nrules = [\"block domain ads.example\"]\n[log]\nformat = \"json\"\n",
    )
    .expect("write config");
    let mut process = spawn(&config);

    let line = process.wait_for_line("listening on");
    let object: serde_json::Value =
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{line}: {e}"));
    assert_eq!(object["level"], "INFO");
    assert!(
        object["target"]
            .as_str()
            .is_some_and(|t| t.starts_with("hickory_filter"))
    );
    assert!(
        object["message"]
            .as_str()
            .is_some_and(|m| m.contains("listening on")),
        "{object}"
    );

    // A query is answered and, with the query log off, not logged at info.
    let addr: SocketAddr = object["message"]
        .as_str()
        .and_then(|m| m.split("listening on ").nth(1))
        .and_then(|a| a.split(',').next())
        .and_then(|a| a.trim().parse().ok())
        .expect("the listening address");
    assert!(blocked_answer(addr));
    process.signal("TERM");
    let (status, _) = process.wait_exit(Duration::from_secs(15));
    assert_eq!(status.code(), Some(0));
    while let Ok(line) = process.lines.try_recv() {
        assert!(!line.contains("hickory_filter::query"), "{line}");
        assert!(!line.contains("ads.example"), "{line}");
    }
}

#[test]
fn sigterm_while_idle_exits_zero_within_the_grace_period() {
    exits_zero_on("TERM");
}

#[test]
fn sigint_while_idle_exits_zero_within_the_grace_period() {
    exits_zero_on("INT");
}

#[test]
fn a_bind_failure_exits_nonzero_and_names_the_address() {
    let taken = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = taken.local_addr().expect("addr");
    let dir = tempfile::tempdir().expect("tempdir");
    let config = write_config(dir.path(), &addr.to_string());

    let mut process = spawn(&config);
    let line = process.wait_for_line("cannot listen on");
    assert!(line.contains(&addr.to_string()), "{line}");
    let (status, _) = process.wait_exit(Duration::from_secs(15));
    assert_eq!(status.code(), Some(1), "{status:?}");
}

#[test]
fn a_config_error_exits_nonzero_before_binding_anything() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("bad.toml");
    std::fs::write(
        &config,
        "upstreams = [\"192.0.2.53\"]\nlisten_adress = []\n",
    )
    .expect("write");

    let mut process = spawn(&config);
    // The error names the file; the key is on a later line of the same error.
    let line = process.wait_for_line("bad.toml");
    assert!(line.starts_with("error:"), "{line}");
    process.wait_for_line("listen_adress");
    let (status, _) = process.wait_exit(Duration::from_secs(15));
    assert_eq!(status.code(), Some(1), "{status:?}");
}
