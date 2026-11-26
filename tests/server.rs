//! The server started in process: listeners, list loading before serving,
//! bind failures and graceful shutdown.

use std::{
    fs,
    net::SocketAddr,
    path::Path,
    time::{Duration, Instant},
};

use hickory_filter::{
    cache,
    config::Config,
    fetch::Fetcher,
    server::{ConfiguredLoader, StartError, start},
};
use hickory_server::proto::{
    op::{Message, Query, ResponseCode},
    rr::{Name, RData, RecordType, rdata::A},
    serialize::binary::BinEncodable,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
};

mod common;
use common::{ok, serve};

/// A config listening on `listen` with an upstream nothing answers (these
/// tests only use names the filter answers itself).
fn config(listen: &str, rest: &str) -> Config {
    Config::parse(
        Path::new("test.toml"),
        &format!("listen = [{listen}]\nupstreams = [\"127.0.0.1:9\"]\n{rest}\n"),
    )
    .expect("config")
}

fn query_bytes(name: &str) -> Vec<u8> {
    let mut message = Message::query();
    message.metadata.id = 0x2a2a;
    message.metadata.recursion_desired = true;
    message.add_query(Query::query(
        Name::from_ascii(name).expect("name"),
        RecordType::A,
    ));
    message.to_bytes().expect("encode")
}

async fn ask_udp(server: SocketAddr, name: &str) -> Message {
    let client = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    client
        .send_to(&query_bytes(name), server)
        .await
        .expect("send");
    let mut buf = [0u8; 512];
    let (len, _) = tokio::time::timeout(Duration::from_secs(5), client.recv_from(&mut buf))
        .await
        .expect("timed out waiting for a UDP answer")
        .expect("recv");
    Message::from_vec(&buf[..len]).expect("decode")
}

async fn ask_tcp(server: SocketAddr, name: &str) -> Message {
    let mut stream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(server))
        .await
        .expect("timed out connecting")
        .expect("connect");
    let query = query_bytes(name);
    stream
        .write_all(&(query.len() as u16).to_be_bytes())
        .await
        .expect("write length");
    stream.write_all(&query).await.expect("write");
    let mut length = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut length))
        .await
        .expect("timed out waiting for a TCP answer")
        .expect("read length");
    let mut body = vec![0u8; u16::from_be_bytes(length) as usize];
    stream.read_exact(&mut body).await.expect("read");
    Message::from_vec(&body).expect("decode")
}

fn is_null_answer(message: &Message) -> bool {
    message.metadata.response_code == ResponseCode::NoError
        && message.answers.len() == 1
        && message.answers[0].data == RData::A(A("0.0.0.0".parse().unwrap()))
}

const BLOCK: &str = "[filter]\nrules = [\"block domain ads.example\"]\n";

#[tokio::test]
async fn answers_over_udp_and_tcp_on_the_same_address() {
    let running = start(&config("\"127.0.0.1:0\"", BLOCK))
        .await
        .expect("start");
    let [addr] = running.addrs() else {
        panic!("{:?}", running.addrs())
    };
    assert_ne!(addr.port(), 0, "a configured port 0 is resolved");

    assert!(is_null_answer(&ask_udp(*addr, "ads.example.").await));
    assert!(is_null_answer(&ask_tcp(*addr, "ads.example.").await));
    running.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn several_listen_addresses_each_answer() {
    let running = start(&config("\"127.0.0.1:0\", \"127.0.0.2:0\"", BLOCK))
        .await
        .expect("start");
    assert_eq!(running.addrs().len(), 2);
    for addr in running.addrs() {
        assert!(
            is_null_answer(&ask_udp(*addr, "ads.example.").await),
            "{addr}"
        );
        assert!(
            is_null_answer(&ask_tcp(*addr, "ads.example.").await),
            "{addr}"
        );
    }
    running.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn ipv4_and_ipv6_any_addresses_can_be_bound_together() {
    // The default listens on both; a port both can share.
    let port = {
        let probe = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        probe.local_addr().expect("addr").port()
    };
    let listen = format!("\"0.0.0.0:{port}\", \"[::]:{port}\"");
    let running = start(&config(&listen, BLOCK)).await.expect("start");

    // Hosts without IPv6 skip the second address instead of failing.
    let v4: SocketAddr = format!("0.0.0.0:{port}").parse().unwrap();
    assert!(running.addrs().contains(&v4));
    assert_eq!(
        running.addrs().len() + running.skipped().len(),
        2,
        "bound {:?}, skipped {:?}",
        running.addrs(),
        running.skipped()
    );
    let local: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    assert!(is_null_answer(&ask_udp(local, "ads.example.").await));
    running.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn a_tcp_bind_failure_names_the_address() {
    let taken = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = taken.local_addr().expect("addr");

    let Err(error) = start(&config(&format!("\"{addr}\""), "")).await else {
        panic!("the address is taken, startup must fail")
    };
    assert!(matches!(error, StartError::Bind(_)), "{error}");
    let text = error.to_string();
    assert!(text.contains(&addr.to_string()), "{text}");
    assert!(text.contains("tcp"), "{text}");
}

#[tokio::test]
async fn a_udp_bind_failure_names_the_address() {
    let taken = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    let addr = taken.local_addr().expect("addr");

    let Err(error) = start(&config(&format!("\"{addr}\""), "")).await else {
        panic!("the address is taken, startup must fail")
    };
    let text = error.to_string();
    assert!(text.contains(&addr.to_string()), "{text}");
    assert!(text.contains("udp"), "{text}");
}

#[tokio::test]
async fn lists_are_loaded_before_the_server_answers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let list = dir.path().join("block.txt");
    fs::write(&list, "from-list.example\n").expect("write");
    let rest = format!(
        "[[filter.lists]]\nrule = \"block list domain file://{}\"\n",
        list.display()
    );
    let running = start(&config("\"127.0.0.1:0\"", &rest))
        .await
        .expect("start");

    let answer = ask_udp(running.addrs()[0], "from-list.example.").await;
    assert!(is_null_answer(&answer), "{answer:?}");
    running.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn a_required_list_that_cannot_load_stops_startup_and_frees_the_port() {
    let port = {
        let probe = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        probe.local_addr().expect("addr").port()
    };
    let rest = "[[filter.lists]]\nrule = \"block list domain file:///nonexistent/hickory-filter/block.txt\"\nrequired = true\n";
    let Err(error) = start(&config(&format!("\"127.0.0.1:{port}\""), rest)).await else {
        panic!("a required list is missing, startup must fail")
    };
    assert!(matches!(error, StartError::RequiredList(_)), "{error}");
    assert!(
        error
            .to_string()
            .contains("/nonexistent/hickory-filter/block.txt")
    );

    // Nothing is left bound.
    TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("the port is free again");
}

#[tokio::test]
async fn an_optional_list_that_cannot_load_does_not_stop_startup() {
    let rest = format!(
        "{BLOCK}[[filter.lists]]\nrule = \"block list domain file:///nonexistent/block.txt\"\n"
    );
    let running = start(&config("\"127.0.0.1:0\"", &rest))
        .await
        .expect("start");
    assert!(is_null_answer(
        &ask_udp(running.addrs()[0], "ads.example.").await
    ));
    running.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn the_cache_is_used_only_when_cache_dir_is_set() {
    let fetcher = || Fetcher::new(Duration::from_secs(5), 1 << 20).expect("fetcher");
    let without = config("\"127.0.0.1:0\"", "");
    assert!(!ConfiguredLoader::new(&without, fetcher()).is_cached());
    let with = config(
        "\"127.0.0.1:0\"",
        "[filter]\ncache_dir = \"/var/cache/hickory-filter\"\n",
    );
    assert!(ConfiguredLoader::new(&with, fetcher()).is_cached());
}

#[tokio::test]
async fn a_remote_list_is_cached_when_a_cache_directory_is_configured() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache_dir = dir.path().join("cache");
    let list_server = serve(|_| ok("cached.example\n")).await;
    let url = list_server.url("/list.txt");
    let rest = format!(
        "[filter]\ncache_dir = \"{}\"\n[[filter.lists]]\nrule = \"block list domain {url}\"\n",
        cache_dir.display()
    );
    let running = start(&config("\"127.0.0.1:0\"", &rest))
        .await
        .expect("start");

    assert!(is_null_answer(
        &ask_udp(running.addrs()[0], "cached.example.").await
    ));
    assert_eq!(
        cache::read(&cache_dir, &url).expect("cached").body,
        "cached.example\n"
    );
    running.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn shutdown_returns_within_the_grace_period_and_stops_accepting() {
    let running = start(&config("\"127.0.0.1:0\"", BLOCK))
        .await
        .expect("start");
    let addr = running.addrs()[0];
    assert!(is_null_answer(&ask_tcp(addr, "ads.example.").await));

    let started = Instant::now();
    let finished = running.shutdown(Duration::from_secs(5)).await;
    assert!(finished, "an idle server finishes within the grace period");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );

    // Nothing accepts connections any more.
    let connect = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(addr)).await;
    assert!(matches!(connect, Ok(Err(_))), "{connect:?}");
}
