//! The request-level handler: answer blocked names ourselves, forward the rest.
//!
//! `FilterHandler` implements hickory's `RequestHandler`. A query for a blocked
//! name is answered here, per the configured mode, without contacting any
//! upstream; every other query is delegated to an inner `Catalog` that
//! forwards it. Building the blocked response by hand keeps its shape (rcode,
//! flags, SOA) independent of how `Catalog` maps zone-handler errors, and the
//! filter runs before the forwarder's cache.

use std::{iter, sync::Arc};

use hickory_server::{
    net::runtime::Time,
    proto::op::{Header, HeaderCounts, MessageType, Metadata, OpCode, ResponseCode},
    server::{Request, RequestHandler, ResponseHandler, ResponseInfo},
    zone_handler::{Catalog, MessageResponseBuilder},
};

use crate::{blocked::blocked_answer, config::Response, ruleset::LiveRules};

/// Answers blocked names per `response`, and forwards everything else.
pub struct FilterHandler {
    inner: Catalog,
    rules: Arc<LiveRules>,
    response: Response,
}

impl FilterHandler {
    /// `rules` is the live rule set, replaced atomically by refreshes;
    /// `inner` is the catalog that forwards unblocked queries.
    pub fn new(inner: Catalog, rules: Arc<LiveRules>, response: Response) -> Self {
        Self {
            inner,
            rules,
            response,
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
        let is_query = request.metadata.message_type == MessageType::Query
            && request.metadata.op_code == OpCode::Query;
        if is_query
            && let Ok(info) = request.request_info()
            && self.rules.load().is_blocked(&info.query.name().to_string())
        {
            let blocked = blocked_answer(
                &self.response,
                info.query.original().name(),
                info.query.query_type(),
            );
            // The ID, the question and the RD flag are echoed from the request.
            let mut metadata = Metadata::response_from_request(info.metadata);
            metadata.response_code = blocked.response_code;
            metadata.authoritative = blocked.authoritative;
            metadata.recursion_available = true;

            let response = MessageResponseBuilder::from_message_request(request).build(
                metadata,
                blocked.answers.iter(),
                iter::empty(),
                blocked.soa.iter(),
                iter::empty(),
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
