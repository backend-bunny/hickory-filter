//! The real binary: it serves, exits 0 on SIGTERM and SIGINT within the grace
//! period, and fails startup with the address when it can't bind.

use std::{
    io::{BufRead, BufReader},
    net::{SocketAddr, TcpListener, UdpSocket},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use hickory_server::proto::{
    op::{Message, Query},
    rr::{Name, RData, RecordType, rdata::A},
    serialize::binary::BinEncodable,
};

/// A running binary and the lines it has written to stderr.
struct Process {
    child: Child,
    lines: mpsc::Receiver<String>,
}

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

fn spawn(config: &Path) -> Process {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hickory-filter"))
        .arg("--config")
        .arg(config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hickory-filter");
    let stderr = child.stderr.take().expect("stderr");
    let (send, lines) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if send.send(line).is_err() {
                return;
            }
        }
    });
    Process { child, lines }
}

impl Process {
    /// Wait for a stderr line containing `needle`.
    fn wait_for_line(&self, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) if line.contains(needle) => return line,
                Ok(_) => {}
                Err(_) => panic!("no stderr line containing {needle:?}"),
            }
        }
    }

    /// The address from the `listening on` line.
    fn listening_addr(&self) -> SocketAddr {
        let line = self.wait_for_line("listening on");
        let text = line.split("listening on ").nth(1).expect("addresses");
        text.split(',')
            .next()
            .expect("an address")
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("{text:?}: {e}"))
    }

    /// The address from the `http endpoint on` line.
    fn http_addr(&self) -> SocketAddr {
        let line = self.wait_for_line("http endpoint on");
        let text = line.split("http endpoint on ").nth(1).expect("an address");
        text.split_whitespace()
            .next()
            .expect("an address")
            .parse()
            .unwrap_or_else(|e| panic!("{text:?}: {e}"))
    }

    fn signal(&self, name: &str) {
        let status = Command::new("kill")
            .arg(format!("-{name}"))
            .arg(self.child.id().to_string())
            .status()
            .expect("run kill");
        assert!(status.success(), "kill -{name} failed");
    }

    /// Wait for the process to exit; kill it and fail after `limit`.
    fn wait_exit(&mut self, limit: Duration) -> (std::process::ExitStatus, Duration) {
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return (status, started.elapsed());
            }
            if started.elapsed() > limit {
                let _ = self.child.kill();
                panic!("the process did not exit within {limit:?}");
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The status line of a plain `GET` to the HTTP endpoint.
fn http_status_line(addr: SocketAddr, path: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n"
    )
    .expect("write");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    response.lines().next().unwrap_or("").to_owned()
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
