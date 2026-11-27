//! A signed delegation hierarchy for DNSSEC tests: real authoritative zones
//! (root, `test.`, `example.test.`) held in memory, signed with generated keys
//! and chained with DS records, served by the library's own catalog on the
//! addresses of `authority`. The root key, written as a trust anchor file, is
//! what the server under test validates against.
//!
//! How it works (the findings of the spike):
//! - Each zone is an `InMemoryZoneHandler` with a SOA, its records and the
//!   delegation to its child (NS, glue and, when the child is signed, DS).
//!   `add_zone_signing_key_mut` plus `secure_zone_mut` sign it, NSEC included,
//!   with one key per zone (flags 257); the library's catalog serves referrals,
//!   DNSKEY and negative proofs, so nothing is scripted.
//! - A DS record comes from `DS::from_key`; the trust anchor file is one
//!   `. 3600 IN DNSKEY <flags> 3 <algorithm> <key>` line, which is how a
//!   `DNSKEY` displays.
//! - A zone signed with a key its parent's DS doesn't name stands for a tampered
//!   answer: the chain of trust breaks, and the server answers SERVFAIL.
//! - A zone with no signatures and no DS in its signed parent is an insecure
//!   delegation: answered normally.

use std::{
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use hickory_server::dnssec::NxProofKind;
use hickory_server::{
    Server,
    net::runtime::Time,
    proto::{
        dnssec::{
            Algorithm, DigestType, DnssecSigner, SigningKey,
            crypto::EcdsaSigningKey,
            rdata::{DNSKEY, DS},
        },
        rr::{LowerName, Name, RData, Record, RecordType, rdata::NS},
    },
    server::{Request, RequestHandler, ResponseHandler, ResponseInfo},
    store::in_memory::InMemoryZoneHandler,
    zone_handler::{AxfrPolicy, Catalog, ZoneType},
};
use tokio::net::{TcpListener, UdpSocket};

use super::authority::{self, Seen, a, name, soa};

/// A zone's signing key and the DNSKEY that goes with it.
pub struct ZoneKey {
    pkcs8: Vec<u8>,
    pub dnskey: DNSKEY,
}

impl ZoneKey {
    pub fn generate() -> Self {
        let pkcs8 =
            EcdsaSigningKey::generate_pkcs8(Algorithm::ECDSAP256SHA256).expect("generate a key");
        let key =
            EcdsaSigningKey::from_pkcs8(&pkcs8, Algorithm::ECDSAP256SHA256).expect("load the key");
        let public = key.to_public_key().expect("public key");
        Self {
            pkcs8: pkcs8.secret_pkcs8_der().to_vec(),
            dnskey: DNSKEY::from_key(&public),
        }
    }

    fn signer(&self, zone: &str) -> DnssecSigner {
        let pkcs8 = rustls::pki_types::PrivatePkcs8KeyDer::from(self.pkcs8.clone());
        let key =
            EcdsaSigningKey::from_pkcs8(&pkcs8, Algorithm::ECDSAP256SHA256).expect("load the key");
        DnssecSigner::new(
            self.dnskey.clone(),
            Box::new(key),
            name(zone),
            Duration::from_secs(365 * 24 * 3600),
        )
    }

    /// The DS record for this key, as published in the parent of `zone`.
    pub fn ds(&self, zone: &str) -> Record {
        let ds = DS::from_key(self.dnskey.public_key(), &name(zone), DigestType::SHA256)
            .expect("a DS record");
        Record::from_rdata(
            name(zone),
            300,
            RData::DNSSEC(hickory_server::proto::dnssec::rdata::DNSSECRData::DS(ds)),
        )
    }
}

/// How the `example.test.` zone is published.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Example {
    /// Signed, with a DS record in `test.` for its key.
    Signed,
    /// Not signed, and `test.` says there is no DS record for it.
    Unsigned,
    /// Signed with a key other than the one the DS record in `test.` names, so
    /// its signatures can't be trusted.
    WrongKey,
}

struct Recording {
    catalog: Catalog,
    seen: Arc<Mutex<Vec<Seen>>>,
}

#[async_trait::async_trait]
impl RequestHandler for Recording {
    async fn handle_request<R: ResponseHandler, T: Time>(
        &self,
        request: &Request,
        response_handle: R,
    ) -> ResponseInfo {
        if let Ok(info) = request.request_info() {
            self.seen.lock().unwrap().push(Seen {
                name: info.query.name().to_string(),
                record_type: info.query.query_type(),
                protocol: request.protocol(),
            });
        }
        self.catalog
            .handle_request::<R, T>(request, response_handle)
            .await
    }
}

/// An authoritative server for one signed (or unsigned) zone.
pub struct ZoneServer {
    seen: Arc<Mutex<Vec<Seen>>>,
    _server: Server<Recording>,
}

impl ZoneServer {
    async fn start(address: Ipv4Addr, zone: &str, handler: InMemoryZoneHandler) -> Self {
        let mut catalog = Catalog::new();
        catalog.upsert(LowerName::new(&name(zone)), vec![Arc::new(handler)]);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut server = Server::new(Recording {
            catalog,
            seen: Arc::clone(&seen),
        });
        server.register_socket(
            UdpSocket::bind((address, 53))
                .await
                .unwrap_or_else(|e| panic!("bind {address}:53: {e}")),
        );
        server.register_listener(
            TcpListener::bind((address, 53))
                .await
                .unwrap_or_else(|e| panic!("bind {address}:53: {e}")),
            Duration::from_secs(5),
            4096,
        );
        Self {
            seen,
            _server: server,
        }
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

/// A zone with `records` (a SOA is added), signed with `key` when given.
fn zone(origin: &str, records: Vec<Record>, key: Option<&ZoneKey>) -> InMemoryZoneHandler {
    let mut handler = InMemoryZoneHandler::empty(
        name(origin),
        ZoneType::Primary,
        AxfrPolicy::Deny,
        Some(NxProofKind::Nsec),
    );
    handler.upsert_mut(soa(origin), 1);
    for record in records {
        handler.upsert_mut(record, 1);
    }
    if let Some(key) = key {
        handler
            .add_zone_signing_key_mut(key.signer(origin))
            .expect("add the signing key");
        handler.secure_zone_mut().expect("sign the zone");
    }
    handler
}

pub struct SignedHierarchy {
    pub root: ZoneServer,
    pub tld: ZoneServer,
    pub authority: ZoneServer,
    pub root_key: ZoneKey,
}

impl SignedHierarchy {
    /// `www.example.test.` is 192.0.2.80 and `ads.example.test.` 192.0.2.66.
    pub async fn start(example: Example) -> Self {
        let root_key = ZoneKey::generate();
        let tld_key = ZoneKey::generate();
        let example_key = ZoneKey::generate();
        // The key the example zone is signed with when it isn't the one named
        // by its DS record.
        let other_key = ZoneKey::generate();

        let root = zone(
            ".",
            vec![
                ns(".", "a.root."),
                a("a.root.", authority::ROOT),
                ns("test.", "ns.test."),
                a("ns.test.", authority::TLD),
                tld_key.ds("test."),
            ],
            Some(&root_key),
        );
        let mut tld_records = vec![
            ns("test.", "ns.test."),
            a("ns.test.", authority::TLD),
            ns("example.test.", "ns.example.test."),
            a("ns.example.test.", authority::AUTHORITY),
        ];
        if example != Example::Unsigned {
            tld_records.push(example_key.ds("example.test."));
        }
        let tld = zone("test.", tld_records, Some(&tld_key));
        let example_records = vec![
            ns("example.test.", "ns.example.test."),
            a("ns.example.test.", authority::AUTHORITY),
            a("www.example.test.", authority::WWW),
            a("ads.example.test.", authority::ADS),
        ];
        let example_zone = match example {
            Example::Signed => zone("example.test.", example_records, Some(&example_key)),
            Example::Unsigned => zone("example.test.", example_records, None),
            Example::WrongKey => zone("example.test.", example_records, Some(&other_key)),
        };

        Self {
            root: ZoneServer::start(authority::ROOT, ".", root).await,
            tld: ZoneServer::start(authority::TLD, "test.", tld).await,
            authority: ZoneServer::start(authority::AUTHORITY, "example.test.", example_zone).await,
            root_key,
        }
    }

    /// Write the root key as a trust anchor file into `dir`.
    pub fn write_anchor(&self, dir: &Path) -> PathBuf {
        let file = dir.join("root.anchor");
        std::fs::write(
            &file,
            format!(". 3600 IN DNSKEY {}\n", self.root_key.dnskey),
        )
        .expect("write the anchor");
        file
    }
}

fn ns(zone: &str, server: &str) -> Record {
    Record::from_rdata(name(zone), 300, RData::NS(NS(name(server))))
}

#[allow(dead_code)]
fn _types(_: RecordType, _: Name) {}
