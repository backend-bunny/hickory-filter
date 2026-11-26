//! The request-level handler: answer blocked names ourselves, forward the rest.
//!
//! `FilterHandler` implements hickory's `RequestHandler`. A blocked query gets
//! NXDOMAIN with the authoritative-answer flag and an SOA record in the
//! authority section, built by hand so the response shape doesn't depend on how
//! `Catalog` maps zone-handler errors. Every other query is delegated to an
//! inner `Catalog`.

use hickory_server::{
    net::runtime::Time,
    proto::{
        op::{Header, HeaderCounts, MessageType, Metadata, ResponseCode},
        rr::{LowerName, Record},
    },
    server::{Request, RequestHandler, ResponseHandler, ResponseInfo},
    zone_handler::{Catalog, MessageResponseBuilder},
};

/// Answers one blocked name with NXDOMAIN + SOA, and forwards everything else.
pub struct FilterHandler {
    inner: Catalog,
    blocked: LowerName,
    soa: Record,
}

impl FilterHandler {
    /// `blocked` is matched exactly; `soa` goes into the authority section.
    pub fn new(inner: Catalog, blocked: LowerName, soa: Record) -> Self {
        Self {
            inner,
            blocked,
            soa,
        }
    }
}

#[async_trait::async_trait]
impl RequestHandler for FilterHandler {
    async fn handle_request<R: ResponseHandler, T: Time>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        if let Ok(info) = request.request_info()
            && info.query.name() == &self.blocked
        {
            let mut metadata = Metadata::response_from_request(info.metadata);
            metadata.response_code = ResponseCode::NXDomain;
            metadata.authoritative = true;

            let builder = MessageResponseBuilder::from_message_request(request);
            let response = builder.build(
                metadata,
                std::iter::empty(),
                std::iter::empty(),
                std::iter::once(&self.soa),
                std::iter::empty(),
            );
            // A failed send is the client's problem (it went away); the
            // protocol-level outcome is the same either way.
            return match response_handle.send_response(response).await {
                Ok(info) => info,
                Err(_) => serve_failed(request),
            };
        }
        self.inner
            .handle_request::<R, T>(request, response_handle)
            .await
    }
}

/// `ResponseInfo` for a response we could not send. hickory's own constructor
/// for this is crate-private, so build it from the public `Header` conversion.
fn serve_failed(request: &Request) -> ResponseInfo {
    let mut metadata = Metadata::new(
        request.metadata.id,
        MessageType::Response,
        request.metadata.op_code,
    );
    metadata.response_code = ResponseCode::ServFail;
    Header {
        metadata,
        counts: HeaderCounts::default(),
    }
    .into()
}
