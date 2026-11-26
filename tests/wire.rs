//! Wire-level checks of the request-level handler design: a real hickory
//! server on loopback, queried over UDP with hand-encoded messages.
//!
//! The upstream is a second server that answers every query with
//! `192.0.2.1`, so a forwarded answer is told apart from a blocked one.

use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use hickory_filter::filter_handler::FilterHandler;
use hickory_server::{
    Server,
    net::runtime::Time,
    proto::{
        op::{Message, Metadata, Query, ResponseCode},
        rr::{
            LowerName, Name, RData, Record, RecordType,
            rdata::{A, SOA},
        },
        serialize::binary::BinEncodable,
    },
    resolver::config::NameServerConfig,
    server::{Request, RequestHandler, ResponseHandler, ResponseInfo},
    store::forwarder::{ForwardConfig, ForwardZoneHandler},
    zone_handler::{Catalog, MessageResponseBuilder},
};
use tokio::net::UdpSocket;

const UPSTREAM_ANSWER: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 1);

/// Answers every query with one A record.
struct StubUpstream;

#[async_trait::async_trait]
impl RequestHandler for StubUpstream {
    async fn handle_request<R: ResponseHandler, T: Time>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        let info = request.request_info().expect("single query");
        let record = Record::from_rdata(
            Name::from(info.query.name().clone()),
            30,
            RData::A(A(UPSTREAM_ANSWER)),
        );
        let metadata = Metadata::response_from_request(info.metadata);
        let response = MessageResponseBuilder::from_message_request(request).build(
            metadata,
            std::iter::once(&record),
            std::iter::empty(),
            std::iter::empty(),
            std::iter::empty(),
        );
        response_handle.send_response(response).await.expect("send")
    }
}

async fn spawn_server<T: RequestHandler>(handler: T) -> (SocketAddr, Server<T>) {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    let addr = socket.local_addr().expect("addr");
    let mut server = Server::new(handler);
    server.register_socket(socket);
    (addr, server)
}

async fn query(server: SocketAddr, name: &str) -> Message {
    query_with_rd(server, name, true).await
}

async fn query_with_rd(server: SocketAddr, name: &str, recursion_desired: bool) -> Message {
    let mut message = Message::query();
    message.metadata.id = 0x1234;
    message.metadata.recursion_desired = recursion_desired;
    message.add_query(Query::query(
        Name::from_ascii(name).expect("name"),
        RecordType::A,
    ));
    let client = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    client
        .send_to(&message.to_bytes().expect("encode"), server)
        .await
        .expect("send");
    let mut buf = [0u8; 512];
    let (len, _) = tokio::time::timeout(Duration::from_secs(5), client.recv_from(&mut buf))
        .await
        .expect("timed out waiting for a response")
        .expect("recv");
    Message::from_vec(&buf[..len]).expect("decode")
}

#[tokio::test]
async fn blocked_name_gets_nxdomain_with_soa_and_aa_everything_else_is_forwarded() {
    // Upstream.
    let (upstream_addr, _upstream) = spawn_server(StubUpstream).await;

    // Forwarder pointing at the stub, on the stub's port.
    let mut name_server = NameServerConfig::udp(upstream_addr.ip());
    name_server.connections[0].port = upstream_addr.port();
    let forwarder = ForwardZoneHandler::builder_tokio(ForwardConfig {
        name_servers: vec![name_server],
        options: None,
    })
    .build()
    .expect("forwarder");
    let mut catalog = Catalog::new();
    catalog.upsert(LowerName::new(&Name::root()), vec![Arc::new(forwarder)]);

    // The filter in front of the catalog.
    let soa_owner = Name::from_ascii("blocked.test.").expect("name");
    let soa = Record::from_rdata(
        soa_owner.clone(),
        60,
        RData::SOA(SOA::new(
            soa_owner.clone(),
            Name::from_ascii("hostmaster.blocked.test.").expect("name"),
            1,
            3600,
            600,
            86400,
            60,
        )),
    );
    let handler = FilterHandler::new(catalog, LowerName::new(&soa_owner), soa);
    let (addr, _server) = spawn_server(handler).await;

    // Blocked: NXDOMAIN, AA set, SOA in the authority section, no answers.
    let blocked = query(addr, "blocked.test.").await;
    assert_eq!(blocked.metadata.id, 0x1234);
    assert_eq!(blocked.metadata.response_code, ResponseCode::NXDomain);
    assert!(blocked.metadata.authoritative, "AA must be set");
    assert!(blocked.answers.is_empty());
    assert_eq!(blocked.authorities.len(), 1);
    assert_eq!(blocked.authorities[0].record_type(), RecordType::SOA);

    // Not blocked: forwarded, answered by the stub upstream.
    let forwarded = query(addr, "ok.test.").await;
    assert_eq!(forwarded.metadata.response_code, ResponseCode::NoError);
    assert_eq!(forwarded.answers.len(), 1);
    assert_eq!(
        forwarded.answers[0].data,
        RData::A(A(UPSTREAM_ANSWER)),
        "the forwarded answer comes from the upstream"
    );

    // A blocked name is answered whether or not the client asked for
    // recursion; the filter doesn't depend on the RD flag.
    let blocked_no_rd = query_with_rd(addr, "blocked.test.", false).await;
    assert_eq!(blocked_no_rd.metadata.response_code, ResponseCode::NXDomain);

    // Findings recorded in design.md D1: the inner Catalog REFUSES a query
    // that does not set RD, because the forwarder only serves recursive
    // queries. That is existing forwarding behaviour, kept as is.
    let forwarded_no_rd = query_with_rd(addr, "ok.test.", false).await;
    assert_eq!(
        forwarded_no_rd.metadata.response_code,
        ResponseCode::Refused
    );
}
