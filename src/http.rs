//! The HTTP endpoint: `/health`, `/ready` and `/metrics`.
//!
//! It runs on its own listener, apart from the DNS sockets, and can be turned
//! off. `/health` answers 200 for as long as the process is up, whatever state
//! the lists are in. `/ready` answers 503 until the server has bound its
//! listeners and finished its first list load, and 200 after; a later refresh
//! failure never makes it fail. `/metrics` renders whatever the metrics
//! recorder holds, in the Prometheus text format.

use std::{
    convert::Infallible,
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use http_body_util::Full;
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header,
    service::service_fn,
};
use hyper_util::{rt::TokioIo, server::graceful::GracefulShutdown};
use tokio::net::TcpListener;

/// The Prometheus text exposition content type.
const METRICS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// How long open HTTP connections get to finish after a shutdown request.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// Renders the current metrics as Prometheus text.
pub type RenderMetrics = Arc<dyn Fn() -> String + Send + Sync>;

/// What the endpoint reports on.
pub struct Endpoint {
    ready: Arc<AtomicBool>,
    render_metrics: RenderMetrics,
}

impl Endpoint {
    pub fn new(ready: Arc<AtomicBool>, render_metrics: RenderMetrics) -> Self {
        Self {
            ready,
            render_metrics,
        }
    }
}

fn respond(status: StatusCode, content_type: &str, body: String) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(body)));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_str(content_type).expect("a valid content type"),
    );
    response
}

fn text(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
    respond(status, "text/plain; charset=utf-8", format!("{body}\n"))
}

async fn route(
    endpoint: Arc<Endpoint>,
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        let mut response = text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        response
            .headers_mut()
            .insert(header::ALLOW, header::HeaderValue::from_static("GET, HEAD"));
        return Ok(response);
    }
    Ok(match request.uri().path() {
        "/health" => text(StatusCode::OK, "ok"),
        "/ready" if endpoint.ready.load(Ordering::Acquire) => text(StatusCode::OK, "ready"),
        "/ready" => text(StatusCode::SERVICE_UNAVAILABLE, "not ready"),
        "/metrics" => respond(
            StatusCode::OK,
            METRICS_CONTENT_TYPE,
            (endpoint.render_metrics)(),
        ),
        _ => text(StatusCode::NOT_FOUND, "not found"),
    })
}

/// Serve the endpoint on `listener` until `shutdown` completes, then give open
/// connections a moment to finish.
pub async fn serve(
    listener: TcpListener,
    endpoint: Arc<Endpoint>,
    shutdown: impl Future<Output = ()>,
) {
    let http = hyper::server::conn::http1::Builder::new();
    let graceful = GracefulShutdown::new();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let endpoint = Arc::clone(&endpoint);
                    let connection = http.serve_connection(
                        TokioIo::new(stream),
                        service_fn(move |request| route(Arc::clone(&endpoint), request)),
                    );
                    let connection = graceful.watch(connection);
                    tokio::spawn(async move {
                        let _ = connection.await;
                    });
                }
                // A transient accept error (out of descriptors, say) must not
                // end the endpoint or spin.
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            },
            () = &mut shutdown => break,
        }
    }
    tokio::select! {
        () = graceful.shutdown() => {}
        () = tokio::time::sleep(SHUTDOWN_GRACE) => {}
    }
}
