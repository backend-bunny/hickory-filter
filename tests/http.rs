//! The HTTP endpoint: `/health`, `/ready` and `/metrics`.

use std::{net::SocketAddr, path::Path, time::Duration};

use hickory_filter::{
    config::Config,
    server::{StartError, start},
};
use reqwest::{Method, StatusCode};
use tokio::net::TcpListener;

mod common;
use common::{Reply, serve};

/// `top` is extra top-level config lines, `tables` the tables that follow.
fn config(top: &str, tables: &str) -> Config {
    Config::parse(
        Path::new("test.toml"),
        &format!("listen = [\"127.0.0.1:0\"]\nupstreams = [\"127.0.0.1:9\"]\n{top}\n{tables}\n"),
    )
    .expect("config")
}

/// A port nothing listens on right now.
async fn free_port() -> u16 {
    let probe = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    probe.local_addr().expect("addr").port()
}

struct Reply2 {
    status: StatusCode,
    content_type: Option<String>,
    body: String,
}

async fn request(method: Method, addr: SocketAddr, path: &str) -> Reply2 {
    let response = reqwest::Client::new()
        .request(method, format!("http://{addr}{path}"))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .expect("an HTTP answer");
    Reply2 {
        status: response.status(),
        content_type: response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
        body: response.text().await.expect("body"),
    }
}

async fn get(addr: SocketAddr, path: &str) -> Reply2 {
    request(Method::GET, addr, path).await
}

#[tokio::test]
async fn the_endpoint_serves_metrics_health_and_readiness() {
    let running = start(&config("http = \"127.0.0.1:0\"", ""))
        .await
        .expect("start");
    let addr = running.http_addr().expect("the endpoint is enabled");

    let health = get(addr, "/health").await;
    assert_eq!(health.status, StatusCode::OK);
    let ready = get(addr, "/ready").await;
    assert_eq!(ready.status, StatusCode::OK);
    assert!(running.is_ready());

    let metrics = get(addr, "/metrics").await;
    assert_eq!(metrics.status, StatusCode::OK);
    assert_eq!(
        metrics.content_type.as_deref(),
        Some("text/plain; version=0.0.4; charset=utf-8")
    );
    running.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn the_default_endpoint_is_port_8080() {
    // Not started here (the port may be taken); the default is the contract.
    let config = config("", "");
    assert_eq!(
        config.http,
        hickory_filter::config::Http::Listen("0.0.0.0:8080".parse().unwrap())
    );
}

#[tokio::test]
async fn health_is_ok_even_when_every_list_failed() {
    let dead = {
        let probe = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        probe.local_addr().expect("addr")
    };
    let tables = format!("[[filter.lists]]\nrule = \"block list domain http://{dead}/list.txt\"\n");
    let running = start(&config("http = \"127.0.0.1:0\"", &tables))
        .await
        .expect("an optional list may fail");
    let addr = running.http_addr().expect("endpoint");

    assert_eq!(get(addr, "/health").await.status, StatusCode::OK);
    // A failed list does not make the server unready either.
    assert_eq!(get(addr, "/ready").await.status, StatusCode::OK);
    running.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn ready_is_503_during_the_initial_load_and_200_after() {
    // A list server that takes a while to answer.
    let list_server = serve(|_| Reply::Stall(Duration::from_millis(1500))).await;
    let port = free_port().await;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let tables = format!(
        "[[filter.lists]]\nrule = \"block list domain {}\"\n",
        list_server.url("/list.txt")
    );
    let config = config(&format!("http = \"{addr}\""), &tables);
    let starting = tokio::spawn(async move { start(&config).await });

    // The endpoint is up before the lists are loaded.
    let mut health = None;
    for _ in 0..100 {
        if let Ok(response) = reqwest::get(format!("http://{addr}/health")).await {
            health = Some(response.status());
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        health,
        Some(StatusCode::OK),
        "/health answers while loading"
    );
    assert_eq!(
        get(addr, "/ready").await.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "not ready during the initial load"
    );

    let running = starting.await.expect("task").expect("start");
    assert_eq!(get(addr, "/ready").await.status, StatusCode::OK);
    running.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn the_endpoint_can_be_disabled() {
    let running = start(&config("http = false", "")).await.expect("start");
    assert_eq!(running.http_addr(), None);
    assert!(running.is_ready());
    running.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn other_paths_and_methods_are_refused() {
    let running = start(&config("http = \"127.0.0.1:0\"", ""))
        .await
        .expect("start");
    let addr = running.http_addr().expect("endpoint");

    assert_eq!(get(addr, "/nope").await.status, StatusCode::NOT_FOUND);
    let post = request(Method::POST, addr, "/health").await;
    assert_eq!(post.status, StatusCode::METHOD_NOT_ALLOWED);
    let head = request(Method::HEAD, addr, "/health").await;
    assert_eq!(head.status, StatusCode::OK);
    assert!(head.body.is_empty());
    running.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn an_http_bind_failure_names_the_address() {
    let taken = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = taken.local_addr().expect("addr");

    let Err(error) = start(&config(&format!("http = \"{addr}\""), "")).await else {
        panic!("the address is taken, startup must fail")
    };
    assert!(matches!(error, StartError::Bind(_)), "{error}");
    let text = error.to_string();
    assert!(text.contains(&addr.to_string()), "{text}");
    assert!(text.contains("http"), "{text}");
}

#[tokio::test]
async fn a_failed_startup_releases_the_http_port() {
    let port = free_port().await;
    let tables = "[[filter.lists]]\nrule = \"block list domain file:///nonexistent/block.txt\"\nrequired = true\n";
    let Err(error) = start(&config(&format!("http = \"127.0.0.1:{port}\""), tables)).await else {
        panic!("a required list is missing, startup must fail")
    };
    assert!(matches!(error, StartError::RequiredList(_)), "{error}");
    TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("the HTTP port is free again");
}

#[tokio::test]
async fn shutdown_stops_the_endpoint_and_reports_not_ready() {
    let running = start(&config("http = \"127.0.0.1:0\"", ""))
        .await
        .expect("start");
    let addr = running.http_addr().expect("endpoint");
    assert_eq!(get(addr, "/ready").await.status, StatusCode::OK);

    assert!(running.shutdown(Duration::from_secs(5)).await);
    let after = reqwest::Client::new()
        .get(format!("http://{addr}/health"))
        .timeout(Duration::from_secs(2))
        .send()
        .await;
    assert!(after.is_err(), "nothing answers after shutdown");
}
