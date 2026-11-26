//! DNS helpers for tests: a stub resolver that records what it is asked, and
//! UDP and TCP clients.

use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};

use hickory_server::{
    Server,
    net::runtime::Time,
    proto::{
        op::{Message, Metadata, Query},
        rr::{Name, RData, Record, RecordType, rdata::A},
        serialize::binary::BinEncodable,
    },
    server::{Request, RequestHandler, ResponseHandler, ResponseInfo},
    zone_handler::MessageResponseBuilder,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UdpSocket},
};

/// Answers every query with one A record and remembers the names asked.
struct StubHandler {
    answer: Ipv4Addr,
    asked: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl RequestHandler for StubHandler {
    async fn handle_request<R: ResponseHandler, T: Time>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        let info = request.request_info().expect("single query");
        self.asked
            .lock()
            .unwrap()
            .push(info.query.name().to_string());
        let record = Record::from_rdata(
            Name::from(info.query.name().clone()),
            30,
            RData::A(A(self.answer)),
        );
        let response = MessageResponseBuilder::from_message_request(request).build(
            Metadata::response_from_request(info.metadata),
            std::iter::once(&record),
            std::iter::empty(),
            std::iter::empty(),
            std::iter::empty(),
        );
        response_handle.send_response(response).await.expect("send")
    }
}

/// A stub DNS resolver on loopback (UDP).
pub struct StubDns {
    pub addr: SocketAddr,
    /// Every name it was asked, in order, as fully qualified names.
    pub asked: Arc<Mutex<Vec<String>>>,
    _server: Server<StubHandler>,
}

impl StubDns {
    pub async fn start(answer: Ipv4Addr) -> Self {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
        let addr = socket.local_addr().expect("addr");
        let mut server = Server::new(StubHandler {
            answer,
            asked: Arc::clone(&asked),
        });
        server.register_socket(socket);
        Self {
            addr,
            asked,
            _server: server,
        }
    }

    pub fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

fn query_bytes(name: &str, record_type: RecordType) -> Vec<u8> {
    let mut message = Message::query();
    message.metadata.id = 0x5151;
    message.metadata.recursion_desired = true;
    message.add_query(Query::query(
        Name::from_ascii(name).expect("name"),
        record_type,
    ));
    message.to_bytes().expect("encode")
}

/// Ask over UDP, waiting up to 20 seconds (a dead upstream takes about four).
pub async fn ask_udp(server: SocketAddr, name: &str, record_type: RecordType) -> Message {
    let client = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    client
        .send_to(&query_bytes(name, record_type), server)
        .await
        .expect("send");
    let mut buf = [0u8; 1024];
    let (len, _) = tokio::time::timeout(Duration::from_secs(20), client.recv_from(&mut buf))
        .await
        .expect("timed out waiting for a UDP answer")
        .expect("recv");
    Message::from_vec(&buf[..len]).expect("decode")
}

/// Ask over TCP (a two-byte length prefix, as DNS over TCP does).
pub async fn ask_tcp(server: SocketAddr, name: &str, record_type: RecordType) -> Message {
    let mut stream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(server))
        .await
        .expect("timed out connecting")
        .expect("connect");
    let query = query_bytes(name, record_type);
    stream
        .write_all(&(query.len() as u16).to_be_bytes())
        .await
        .expect("write length");
    stream.write_all(&query).await.expect("write");
    let mut length = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(20), stream.read_exact(&mut length))
        .await
        .expect("timed out waiting for a TCP answer")
        .expect("read length");
    let mut body = vec![0u8; u16::from_be_bytes(length) as usize];
    stream.read_exact(&mut body).await.expect("read");
    Message::from_vec(&body).expect("decode")
}
