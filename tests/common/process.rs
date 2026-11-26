//! Driving the real binary from a test: spawn it, read its stderr lines, send
//! it signals, and talk to its HTTP endpoint.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

/// A running binary and the lines it has written to stderr.
pub struct Process {
    pub child: Child,
    pub lines: mpsc::Receiver<String>,
}

pub fn spawn(config: &Path) -> Process {
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
    pub fn wait_for_line(&self, needle: &str) -> String {
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
    pub fn listening_addr(&self) -> SocketAddr {
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
    pub fn http_addr(&self) -> SocketAddr {
        let line = self.wait_for_line("http endpoint on");
        let text = line.split("http endpoint on ").nth(1).expect("an address");
        text.split_whitespace()
            .next()
            .expect("an address")
            .parse()
            .unwrap_or_else(|e| panic!("{text:?}: {e}"))
    }

    pub fn signal(&self, name: &str) {
        let status = Command::new("kill")
            .arg(format!("-{name}"))
            .arg(self.child.id().to_string())
            .status()
            .expect("run kill");
        assert!(status.success(), "kill -{name} failed");
    }

    /// Wait for the process to exit; kill it and fail after `limit`.
    pub fn wait_exit(&mut self, limit: Duration) -> (ExitStatus, Duration) {
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

/// A plain `GET` to the HTTP endpoint: the whole response text.
pub fn http_get(addr: SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).expect("connect");
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
    response
}

/// The status line of a plain `GET` to the HTTP endpoint.
pub fn http_status_line(addr: SocketAddr, path: &str) -> String {
    http_get(addr, path).lines().next().unwrap_or("").to_owned()
}
