//! The request-level handler: answer blocked names ourselves, forward the rest.
//!
//! `FilterHandler` implements hickory's `RequestHandler`. A query for a blocked
//! name is answered here, per the configured mode, without contacting any
//! upstream; every other query is delegated to an inner `Catalog` that
//! forwards it. Building the blocked response by hand keeps its shape (rcode,
//! flags, SOA) independent of how `Catalog` maps zone-handler errors, and the
//! filter runs before the forwarder's cache.

use std::{iter, sync::Arc, time::Instant};

use hickory_server::{
    net::runtime::Time,
    proto::op::{Header, HeaderCounts, MessageType, Metadata, OpCode, ResponseCode},
    server::{Request, RequestHandler, ResponseHandler, ResponseInfo},
    zone_handler::{Catalog, MessageResponseBuilder},
};

use crate::{
    blocked::blocked_answer,
    config::Response,
    logging::{QueryEvent, QueryLog},
    metrics,
    ruleset::{LiveRules, Verdict},
};

/// Answers blocked names per `response`, and forwards everything else.
pub struct FilterHandler {
    inner: Catalog,
    rules: Arc<LiveRules>,
    response: Response,
    query_log: QueryLog,
}

impl FilterHandler {
    /// `rules` is the live rule set, replaced atomically by refreshes;
    /// `inner` is the catalog that forwards unblocked queries.
    pub fn new(inner: Catalog, rules: Arc<LiveRules>, response: Response) -> Self {
        Self {
            inner,
            rules,
            response,
            query_log: QueryLog::default(),
        }
    }

    /// Log queries as `query_log` says (by default they are only logged at the
    /// `debug` level).
    pub fn with_query_log(mut self, query_log: QueryLog) -> Self {
        self.query_log = query_log;
        self
    }
}

#[async_trait::async_trait]
impl RequestHandler for FilterHandler {
    async fn handle_request<R: ResponseHandler, T: Time>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        let started = Instant::now();
        let is_query = request.metadata.message_type == MessageType::Query
            && request.metadata.op_code == OpCode::Query;
        let question = if is_query {
            request.request_info().ok().map(|info| {
                let verdict = self.rules.load().verdict(&info.query.name().to_string());
                (info, verdict)
            })
        } else {
            None
        };

        // A blocked name is answered here, per the configured mode.
        if let Some((info, Verdict::Block)) = &question {
            let record_type = info.query.query_type();
            let blocked = blocked_answer(&self.response, info.query.original().name(), record_type);
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
            let sent = match response_handle.send_response(response).await {
                Ok(info) => info,
                Err(_) => serve_failed(request),
            };
            let elapsed = started.elapsed();
            metrics::record_query("blocked", record_type, elapsed);
            self.query_log.record(&QueryEvent {
                client: request.src().ip(),
                name: info.query.name(),
                record_type,
                outcome: "blocked",
                rcode: blocked.response_code,
                elapsed,
            });
            return sent;
        }

        // Everything else is forwarded. An allow rule only changes how the
        // query is counted, and an answer of SERVFAIL means no upstream
        // answered.
        let answered = self
            .inner
            .handle_request::<R, T>(request, response_handle)
            .await;
        if let Some((info, verdict)) = question {
            let outcome = if answered.response_code == ResponseCode::ServFail {
                "upstream_error"
            } else if verdict == Verdict::Allow {
                "allowed"
            } else {
                "passed"
            };
            let elapsed = started.elapsed();
            let record_type = info.query.query_type();
            metrics::record_query(outcome, record_type, elapsed);
            self.query_log.record(&QueryEvent {
                client: request.src().ip(),
                name: info.query.name(),
                record_type,
                outcome,
                rcode: answered.response_code,
                elapsed,
            });
        }
        answered
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
