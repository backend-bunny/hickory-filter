//! A small HTTP server on loopback for the list-fetching tests.
#![allow(dead_code)]

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[derive(Clone, Debug)]
pub struct Req {
    pub path: String,
    pub headers: HashMap<String, String>,
}

pub enum Reply {
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

pub fn ok(body: &str) -> Reply {
    Reply::Full {
        status: 200,
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    }
}

pub fn status(code: u16) -> Reply {
    Reply::Full {
        status: code,
        headers: Vec::new(),
        body: Vec::new(),
    }
}

pub fn redirect(location: &str) -> Reply {
    Reply::Full {
        status: 302,
        headers: vec![("Location", location.to_owned())],
        body: Vec::new(),
    }
}

pub struct TestServer {
    pub addr: SocketAddr,
    requests: Arc<Mutex<Vec<Req>>>,
}

impl TestServer {
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub fn requests(&self) -> Vec<Req> {
        self.requests.lock().unwrap().clone()
    }
}

pub async fn serve<F>(handler: F) -> TestServer
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

/// The text of README.md.
pub fn readme() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).expect("README.md")
}

/// The rows of the table under `heading` in README.md, as trimmed cells. Only
/// rows whose first cell starts with a backtick count, so headers are skipped.
pub fn readme_table(heading: &str) -> Vec<Vec<String>> {
    let mut in_section = false;
    let mut rows = Vec::new();
    for line in readme().lines() {
        if line.trim_end() == heading {
            in_section = true;
            continue;
        }
        if in_section && line.starts_with('#') {
            break;
        }
        if in_section && line.starts_with("| `") {
            rows.push(
                line.trim()
                    .trim_matches('|')
                    .split('|')
                    .map(|cell| cell.trim().to_owned())
                    .collect(),
            );
        }
    }
    assert!(!rows.is_empty(), "no table under {heading} in README.md");
    rows
}

/// The words between backticks in a table cell.
pub fn backticked(cell: &str) -> Vec<String> {
    cell.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}
