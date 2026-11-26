//! The running server: listeners, lists, refresher and DNS service together.
//!
//! `start` binds every listen address (UDP and TCP), loads every list before
//! it begins serving, and starts the refresher. A bind failure or a `required`
//! list that can't be loaded stops startup (fail closed). `Running::shutdown`
//! stops accepting queries, lets the ones in flight finish for a bounded grace
//! period, and stops the refresher.

use std::{
    fmt, io,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use hickory_server::Server;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::{
    net::{TcpListener, UdpSocket},
    sync::watch,
    task::JoinHandle,
};

use crate::{
    cache::CachedLoader,
    config::{Config, Http},
    fetch::{FetchError, FetchOutcome, Fetcher},
    filter_handler::FilterHandler,
    forwarder::{ForwarderError, build_catalog},
    http::{self, Endpoint},
    lists::{RequiredListError, Source, Validators},
    metrics,
    refresh::{DefaultLoader, ListLoader, Refresher},
    rules::RuleError,
    ruleset::{LiveRules, RuleSet},
};

/// How long in-flight queries may take to finish after a shutdown request.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// How long an idle TCP connection stays open.
const TCP_IDLE_TIMEOUT: Duration = Duration::from_secs(5);
const TCP_RESPONSE_BUFFER: usize = 1024;

/// A listen address that could not be bound.
#[derive(Debug)]
pub struct BindError {
    pub address: SocketAddr,
    pub protocol: &'static str,
    pub source: io::Error,
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cannot listen on {} ({}): {}",
            self.address, self.protocol, self.source
        )
    }
}

impl std::error::Error for BindError {}

/// Why the server could not start.
#[derive(Debug)]
pub enum StartError {
    Bind(BindError),
    /// A `required` list could not be loaded.
    RequiredList(RequiredListError),
    Rules(RuleError),
    Fetcher(FetchError),
    Forwarder(ForwarderError),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bind(e) => e.fmt(f),
            Self::RequiredList(e) => e.fmt(f),
            Self::Rules(e) => e.fmt(f),
            Self::Fetcher(e) => write!(f, "cannot build the list fetcher: {e}"),
            Self::Forwarder(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for StartError {}

impl From<BindError> for StartError {
    fn from(e: BindError) -> Self {
        Self::Bind(e)
    }
}

impl From<RequiredListError> for StartError {
    fn from(e: RequiredListError) -> Self {
        Self::RequiredList(e)
    }
}

impl From<RuleError> for StartError {
    fn from(e: RuleError) -> Self {
        Self::Rules(e)
    }
}

impl From<FetchError> for StartError {
    fn from(e: FetchError) -> Self {
        Self::Fetcher(e)
    }
}

impl From<ForwarderError> for StartError {
    fn from(e: ForwarderError) -> Self {
        Self::Forwarder(e)
    }
}

/// Bound UDP and TCP sockets, one pair per usable listen address.
pub struct Listeners {
    udp: Vec<UdpSocket>,
    tcp: Vec<TcpListener>,
    /// The addresses actually bound (a configured port of 0 is resolved).
    pub addrs: Vec<SocketAddr>,
    /// IPv6 addresses skipped because this host has no IPv6, with why.
    pub skipped: Vec<(SocketAddr, String)>,
}

fn bind_socket(addr: SocketAddr, kind: Type, protocol: Protocol) -> io::Result<Socket> {
    let socket = Socket::new(Domain::for_address(addr), kind, Some(protocol))?;
    if addr.is_ipv6() {
        // So `[::]:1053` and `0.0.0.0:1053` can both be bound.
        socket.set_only_v6(true)?;
    }
    socket.set_nonblocking(true)?;
    Ok(socket)
}

fn bind_tcp(addr: SocketAddr) -> io::Result<TcpListener> {
    let socket = bind_socket(addr, Type::STREAM, Protocol::TCP)?;
    // Lets a restarted server rebind at once; it does not allow two listeners.
    socket.set_reuse_address(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    TcpListener::from_std(socket.into())
}

fn bind_udp(addr: SocketAddr) -> io::Result<UdpSocket> {
    let socket = bind_socket(addr, Type::DGRAM, Protocol::UDP)?;
    socket.bind(&addr.into())?;
    UdpSocket::from_std(socket.into())
}

/// Is this the error of binding an IPv6 address on a host without IPv6?
fn ipv6_unavailable(addr: SocketAddr, error: &io::Error) -> bool {
    // EAFNOSUPPORT (97) and EADDRNOTAVAIL (99) on Linux.
    addr.is_ipv6() && matches!(error.raw_os_error(), Some(97 | 99))
}

/// Bind UDP and TCP on every address. A failure names the address and the
/// protocol. The one tolerated failure is an IPv6 address on a host without
/// IPv6, so the default of listening on all addresses also works there; at
/// least one address must bind.
pub fn bind(addresses: &[SocketAddr]) -> Result<Listeners, BindError> {
    let mut listeners = Listeners {
        udp: Vec::new(),
        tcp: Vec::new(),
        addrs: Vec::new(),
        skipped: Vec::new(),
    };
    let mut first_skipped: Option<BindError> = None;
    for &address in addresses {
        let tcp = match bind_tcp(address) {
            Ok(tcp) => tcp,
            Err(source) if ipv6_unavailable(address, &source) => {
                listeners.skipped.push((address, source.to_string()));
                first_skipped.get_or_insert(BindError {
                    address,
                    protocol: "tcp",
                    source,
                });
                continue;
            }
            Err(source) => {
                return Err(BindError {
                    address,
                    protocol: "tcp",
                    source,
                });
            }
        };
        // UDP on the same port as TCP, which also resolves a configured port 0.
        let bound = tcp.local_addr().map_err(|source| BindError {
            address,
            protocol: "tcp",
            source,
        })?;
        let udp = bind_udp(bound).map_err(|source| BindError {
            address,
            protocol: "udp",
            source,
        })?;
        listeners.tcp.push(tcp);
        listeners.udp.push(udp);
        listeners.addrs.push(bound);
    }
    match (listeners.addrs.is_empty(), first_skipped) {
        (true, Some(error)) => Err(error),
        _ => Ok(listeners),
    }
}

/// The list loader the configuration asks for: with a cache directory, remote
/// lists are cached; without one nothing is ever written.
pub enum ConfiguredLoader {
    Plain(DefaultLoader),
    Cached(CachedLoader<DefaultLoader>),
}

impl ConfiguredLoader {
    pub fn new(config: &Config, fetcher: Fetcher) -> Self {
        let loader = DefaultLoader::new(fetcher);
        match &config.filter.cache_dir {
            Some(dir) => Self::Cached(CachedLoader::new(loader, dir.clone())),
            None => Self::Plain(loader),
        }
    }

    pub fn is_cached(&self) -> bool {
        matches!(self, Self::Cached(_))
    }
}

impl ListLoader for ConfiguredLoader {
    async fn load(&self, source: &Source, validators: &Validators) -> Result<FetchOutcome, String> {
        match self {
            Self::Plain(loader) => loader.load(source, validators).await,
            Self::Cached(loader) => loader.load(source, validators).await,
        }
    }
}

/// A started server.
pub struct Running {
    server: Server<FilterHandler>,
    refresher: JoinHandle<()>,
    http: Option<HttpTask>,
    ready: Arc<AtomicBool>,
    addrs: Vec<SocketAddr>,
    skipped: Vec<(SocketAddr, String)>,
    live: Arc<LiveRules>,
}

impl Running {
    /// The addresses being served (UDP and TCP on each).
    pub fn addrs(&self) -> &[SocketAddr] {
        &self.addrs
    }

    /// IPv6 listen addresses skipped because the host has no IPv6.
    pub fn skipped(&self) -> &[(SocketAddr, String)] {
        &self.skipped
    }

    /// The live rule set.
    pub fn live(&self) -> &Arc<LiveRules> {
        &self.live
    }

    /// The HTTP endpoint's address, when it is enabled.
    pub fn http_addr(&self) -> Option<SocketAddr> {
        self.http.as_ref().map(|http| http.addr)
    }

    /// Whether `/ready` answers 200.
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    /// Stop accepting queries, give the ones in flight up to `grace` to finish,
    /// and stop the refresher and the HTTP endpoint. Returns whether everything
    /// finished in time.
    pub async fn shutdown(mut self, grace: Duration) -> bool {
        self.ready.store(false, Ordering::Release);
        self.server.shutdown_token().cancel();
        let http = self.http.take();
        let finished = tokio::time::timeout(grace, async {
            if let Some(http) = http {
                http.stop().await;
            }
            let _ = self.server.block_until_done().await;
            let _ = (&mut self.refresher).await;
        })
        .await
        .is_ok();
        if !finished {
            self.refresher.abort();
        }
        finished
    }
}

/// The running HTTP endpoint.
struct HttpTask {
    handle: JoinHandle<()>,
    stop: watch::Sender<bool>,
    addr: SocketAddr,
}

impl HttpTask {
    async fn stop(self) {
        let _ = self.stop.send(true);
        let _ = self.handle.await;
    }
}

/// Bind and start the HTTP endpoint, if it is enabled. It starts before the
/// lists are loaded, so `/health` answers and `/ready` says 503 meanwhile.
async fn start_http(
    config: &Config,
    ready: &Arc<AtomicBool>,
) -> Result<Option<HttpTask>, BindError> {
    let Http::Listen(address) = config.http else {
        return Ok(None);
    };
    let bind_error = |source| BindError {
        address,
        protocol: "http",
        source,
    };
    let listener = TcpListener::bind(address).await.map_err(bind_error)?;
    let addr = listener.local_addr().map_err(bind_error)?;
    let (stop, mut stopped) = watch::channel(false);
    let endpoint = Arc::new(Endpoint::new(Arc::clone(ready), Arc::new(metrics::render)));
    let handle = tokio::spawn(http::serve(listener, endpoint, async move {
        let _ = stopped.changed().await;
    }));
    Ok(Some(HttpTask { handle, stop, addr }))
}

/// Load every list and start answering DNS on the bound sockets.
async fn serve_dns(
    config: &Config,
    listeners: Listeners,
) -> Result<
    (
        Server<FilterHandler>,
        JoinHandle<()>,
        Vec<SocketAddr>,
        Vec<(SocketAddr, String)>,
        Arc<LiveRules>,
    ),
    StartError,
> {
    let live = Arc::new(LiveRules::new(RuleSet::default()));
    let fetcher = Fetcher::from_config(config)?;
    let loader = Arc::new(ConfiguredLoader::new(config, fetcher));
    let mut refresher = Refresher::new(loader, config, Arc::clone(&live))?;
    refresher.initial_load().await?;

    let catalog = build_catalog(config)?;
    let handler = FilterHandler::new(catalog, Arc::clone(&live), config.filter.response);
    let mut server = Server::new(handler);
    for socket in listeners.udp {
        server.register_socket(socket);
    }
    for listener in listeners.tcp {
        server.register_listener(listener, TCP_IDLE_TIMEOUT, TCP_RESPONSE_BUFFER);
    }

    let token = server.shutdown_token().clone();
    let refresher = tokio::spawn(refresher.run(async move { token.cancelled().await }));
    Ok((server, refresher, listeners.addrs, listeners.skipped, live))
}

/// Start serving: bind, load every list, then answer queries and keep the
/// lists fresh. Fails closed: a bind error or a `required` list that can't be
/// loaded stops startup. The HTTP endpoint comes up first and reports ready
/// once the lists are loaded and the DNS listeners are serving.
pub async fn start(config: &Config) -> Result<Running, StartError> {
    // Record metrics from the start, including the initial list load.
    metrics::install();
    // Bind first so a bad address fails before any list is fetched.
    let listeners = bind(&config.listen)?;
    let ready = Arc::new(AtomicBool::new(false));
    let http = start_http(config, &ready).await?;

    match serve_dns(config, listeners).await {
        Ok((server, refresher, addrs, skipped, live)) => {
            ready.store(true, Ordering::Release);
            Ok(Running {
                server,
                refresher,
                http,
                ready,
                addrs,
                skipped,
                live,
            })
        }
        Err(error) => {
            if let Some(http) = http {
                http.stop().await;
            }
            Err(error)
        }
    }
}
