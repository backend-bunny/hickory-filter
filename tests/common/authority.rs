//! Scripted fake name servers and a small delegation hierarchy for recursion
//! tests: a root, a TLD and an authoritative server, each on its own address
//! at port 53. They only work inside the network namespace of `netns`, where
//! binding port 53 and adding addresses to loopback is allowed.

use std::{
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use hickory_server::{
    Server,
    net::{runtime::Time, xfer::Protocol},
    proto::{
        op::{Metadata, ResponseCode},
        rr::{
            Name, RData, Record, RecordType,
            rdata::{A, NS, SOA},
        },
    },
    server::{Request, RequestHandler, ResponseHandler, ResponseInfo},
    zone_handler::MessageResponseBuilder,
};
use tokio::net::{TcpListener, UdpSocket};

/// Addresses of the hierarchy. They are outside every range the recursor
/// refuses as name servers (loopback, private, documentation and so on); this
/// one is the benchmarking range.
pub const ROOT: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 10);
pub const TLD: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 11);
pub const AUTHORITY: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 12);

/// The addresses the namespace must have on its loopback interface.
pub fn addresses() -> Vec<String> {
    [ROOT, TLD, AUTHORITY]
        .iter()
        .map(|a| a.to_string())
        .collect()
}

/// What a fake server answers.
#[derive(Default, Clone)]
pub struct Say {
    pub rcode: ResponseCode,
    pub authoritative: bool,
    pub answers: Vec<Record>,
    /// The authority section: the name servers of a referral.
    pub referral: Vec<Record>,
    /// The authority section of a negative answer.
    pub soa: Vec<Record>,
    pub additional: Vec<Record>,
}

/// One query a fake server received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    /// The name as it arrived, case preserved, fully qualified.
    pub name: String,
    pub record_type: RecordType,
    /// How it arrived: UDP, TCP or TLS.
    pub protocol: Protocol,
}

type Script = dyn Fn(&str, RecordType) -> Say + Send + Sync;

struct Handler {
    script: Arc<Script>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

#[async_trait::async_trait]
impl RequestHandler for Handler {
    async fn handle_request<R: ResponseHandler, T: Time>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        let info = request.request_info().expect("single query");
        let name = info.query.name().to_string();
        let record_type = info.query.query_type();
        self.seen.lock().unwrap().push(Seen {
            name: name.clone(),
            record_type,
            protocol: request.protocol(),
        });
        let say = (self.script)(&name.to_ascii_lowercase(), record_type);
        let mut metadata = Metadata::response_from_request(info.metadata);
        metadata.authoritative = say.authoritative;
        metadata.response_code = say.rcode;
        let response = MessageResponseBuilder::from_message_request(request).build(
            metadata,
            say.answers.iter(),
            say.referral.iter(),
            say.soa.iter(),
            say.additional.iter(),
        );
        response_handle.send_response(response).await.expect("send")
    }
}

/// A fake name server on `address`, UDP and TCP port 53.
pub struct FakeServer {
    pub address: Ipv4Addr,
    seen: Arc<Mutex<Vec<Seen>>>,
    _server: Server<Handler>,
}

impl FakeServer {
    pub async fn start(
        address: Ipv4Addr,
        script: impl Fn(&str, RecordType) -> Say + Send + Sync + 'static,
    ) -> Self {
        Self::start_with(address, false, script).await
    }

    /// Like `start`, and also accepting DNS-over-TLS on port 853 with a
    /// self-signed certificate.
    pub async fn start_tls(
        address: Ipv4Addr,
        script: impl Fn(&str, RecordType) -> Say + Send + Sync + 'static,
    ) -> Self {
        Self::start_with(address, true, script).await
    }

    async fn start_with(
        address: Ipv4Addr,
        tls: bool,
        script: impl Fn(&str, RecordType) -> Say + Send + Sync + 'static,
    ) -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let udp = UdpSocket::bind(SocketAddr::from((address, 53)))
            .await
            .unwrap_or_else(|e| panic!("bind {address}:53 (udp): {e}"));
        let tcp = TcpListener::bind(SocketAddr::from((address, 53)))
            .await
            .unwrap_or_else(|e| panic!("bind {address}:53 (tcp): {e}"));
        let mut server = Server::new(Handler {
            script: Arc::new(script),
            seen: Arc::clone(&seen),
        });
        server.register_socket(udp);
        server.register_listener(tcp, Duration::from_secs(5), 4096);
        if tls {
            let listener = TcpListener::bind(SocketAddr::from((address, 853)))
                .await
                .unwrap_or_else(|e| panic!("bind {address}:853 (tls): {e}"));
            server
                .register_tls_listener_with_tls_config(
                    listener,
                    Duration::from_secs(5),
                    tls_config(),
                )
                .expect("register the TLS listener");
        }
        Self {
            address,
            seen,
            _server: server,
        }
    }

    /// Every query received, in order.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

/// A server certificate for `ns.example.test`, generated now and signed by
/// itself: the recursor's opportunistic encryption doesn't verify it.
fn tls_config() -> Arc<rustls::ServerConfig> {
    let certified = rcgen::generate_simple_self_signed(vec!["ns.example.test".to_owned()])
        .expect("certificate");
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
        certified.signing_key.serialize_der(),
    ));
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_no_client_auth()
        .with_single_cert(vec![certified.cert.der().clone()], key)
        .expect("server certificate");
    config.alpn_protocols = vec![b"dot".to_vec()];
    Arc::new(config)
}

pub fn name(text: &str) -> Name {
    Name::from_ascii(text).unwrap_or_else(|e| panic!("name {text:?}: {e}"))
}

pub fn a(owner: &str, address: Ipv4Addr) -> Record {
    Record::from_rdata(name(owner), 300, RData::A(A(address)))
}

pub fn ns(zone: &str, server: &str) -> Record {
    Record::from_rdata(name(zone), 300, RData::NS(NS(name(server))))
}

pub fn soa(zone: &str) -> Record {
    Record::from_rdata(
        name(zone),
        300,
        RData::SOA(SOA::new(
            name(&format!("ns.{zone}")),
            name(&format!("hostmaster.{zone}")),
            1,
            3600,
            600,
            86400,
            300,
        )),
    )
}

/// NXDOMAIN with the zone's SOA.
pub fn nxdomain(zone: &str) -> Say {
    Say {
        rcode: ResponseCode::NXDomain,
        authoritative: true,
        soa: vec![soa(zone)],
        ..Say::default()
    }
}

/// NOERROR with no data, with the zone's SOA.
pub fn nodata(zone: &str) -> Say {
    Say {
        authoritative: true,
        soa: vec![soa(zone)],
        ..Say::default()
    }
}

/// A delegation to `server`, whose address is given as glue.
pub fn referral(zone: &str, server: &str, glue: Ipv4Addr) -> Say {
    Say {
        referral: vec![ns(zone, server)],
        additional: vec![a(server, glue)],
        ..Say::default()
    }
}

fn is_at_or_below(name: &str, zone: &str) -> bool {
    name == zone || name.ends_with(&format!(".{zone}"))
}

/// The address records the authoritative server serves for `example.test.`.
pub const WWW: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 80);
pub const ADS: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 66);

/// A root server, a `test.` server and an `example.test.` server.
///
/// The root delegates `test.` to `ns.test.`, which delegates `example.test.`
/// to `ns.example.test.`, which serves `www.example.test.` (192.0.2.80) and
/// `ads.example.test.` (192.0.2.66).
pub struct Hierarchy {
    pub root: FakeServer,
    pub tld: FakeServer,
    pub authority: FakeServer,
}

impl Hierarchy {
    pub async fn start() -> Self {
        Self::start_with(false).await
    }

    /// Like `start`, with an authoritative server that also accepts
    /// DNS-over-TLS.
    pub async fn start_tls_authority() -> Self {
        Self::start_with(true).await
    }

    async fn start_with(tls_authority: bool) -> Self {
        let root = FakeServer::start(ROOT, |name, record_type| {
            if name == "." && record_type == RecordType::NS {
                return Say {
                    authoritative: true,
                    answers: vec![ns(".", "a.root.")],
                    additional: vec![a("a.root.", ROOT)],
                    ..Say::default()
                };
            }
            if is_at_or_below(name, "test.") {
                return referral("test.", "ns.test.", TLD);
            }
            nxdomain(".")
        })
        .await;
        let tld = FakeServer::start(TLD, |name, record_type| {
            if is_at_or_below(name, "example.test.") {
                return referral("example.test.", "ns.example.test.", AUTHORITY);
            }
            match (name, record_type) {
                ("test.", RecordType::NS) => Say {
                    authoritative: true,
                    answers: vec![ns("test.", "ns.test.")],
                    additional: vec![a("ns.test.", TLD)],
                    ..Say::default()
                },
                ("ns.test.", RecordType::A) => Say {
                    authoritative: true,
                    answers: vec![a("ns.test.", TLD)],
                    ..Say::default()
                },
                ("test." | "ns.test.", _) => nodata("test."),
                _ => nxdomain("test."),
            }
        })
        .await;
        let authority = FakeServer::start_with(AUTHORITY, tls_authority, |name, record_type| {
            let owner = name.to_owned();
            match (name, record_type) {
                ("example.test.", RecordType::NS) => Say {
                    authoritative: true,
                    answers: vec![ns("example.test.", "ns.example.test.")],
                    additional: vec![a("ns.example.test.", AUTHORITY)],
                    ..Say::default()
                },
                ("ns.example.test.", RecordType::A) => Say {
                    authoritative: true,
                    answers: vec![a(&owner, AUTHORITY)],
                    ..Say::default()
                },
                ("www.example.test.", RecordType::A) => Say {
                    authoritative: true,
                    answers: vec![a(&owner, WWW)],
                    ..Say::default()
                },
                ("ads.example.test.", RecordType::A) => Say {
                    authoritative: true,
                    answers: vec![a(&owner, ADS)],
                    ..Say::default()
                },
                (
                    "example.test." | "ns.example.test." | "www.example.test."
                    | "ads.example.test.",
                    _,
                ) => nodata("example.test."),
                _ => nxdomain("example.test."),
            }
        })
        .await;
        Self {
            root,
            tld,
            authority,
        }
    }

    /// The names of all queries the three servers received, by server.
    pub fn seen(&self) -> [Vec<Seen>; 3] {
        [self.root.seen(), self.tld.seen(), self.authority.seen()]
    }

    pub fn queries_received(&self) -> usize {
        self.seen().iter().map(Vec::len).sum()
    }
}

/// Write a root hints file naming the fake root server into `dir`.
pub fn write_hints(dir: &Path) -> PathBuf {
    let file = dir.join("root.hints");
    std::fs::write(
        &file,
        format!(". 3600000 IN NS a.root.\na.root. 3600000 IN A {ROOT}\n"),
    )
    .expect("write root hints");
    file
}
