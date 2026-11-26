//! Logging: the subscriber, and the query log.
//!
//! Logs go to stderr as plain text or JSON at the configured level. This
//! crate's lines follow that level; other crates' lines are limited to
//! warnings, so turning on `debug` doesn't bury the output in library noise.
//!
//! The query log writes one line per answered query under a fixed target. It is
//! off by default: queries are then only logged at `debug`. With it on, the
//! lines are `info` and stay visible even at a quieter level, and in JSON
//! format their fields are top-level members of the object, so a log collector
//! can select them by target and read `client`, `name`, `type`, `outcome`,
//! `rcode` and `duration_ms` directly.

use std::{
    net::{IpAddr, Ipv6Addr},
    time::Duration,
};

use hickory_server::proto::{op::ResponseCode, rr::LowerName, rr::RecordType};
use tracing::{Level, Subscriber, level_filters::LevelFilter};
use tracing_subscriber::{
    Layer, Registry,
    filter::Targets,
    fmt::{self, MakeWriter},
    layer::SubscriberExt,
};

use crate::config::{Log, LogFormat, LogLevel, QueryLogClient};

/// The target of every query-log line.
pub const QUERY_TARGET: &str = "hickory_filter::query";

/// The fields of a query-log line, besides the message `query`.
pub const QUERY_FIELDS: &[&str] = &["client", "name", "type", "outcome", "rcode", "duration_ms"];

fn level_filter(level: LogLevel) -> LevelFilter {
    match level {
        LogLevel::Error => LevelFilter::ERROR,
        LogLevel::Warn => LevelFilter::WARN,
        LogLevel::Info => LevelFilter::INFO,
        LogLevel::Debug => LevelFilter::DEBUG,
        LogLevel::Trace => LevelFilter::TRACE,
    }
}

/// A subscriber writing to `writer` as `log` says.
pub fn subscriber<W>(log: &Log, writer: W) -> Box<dyn Subscriber + Send + Sync>
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    let level = level_filter(log.level);
    let mut targets = Targets::new()
        .with_default(level.min(LevelFilter::WARN))
        .with_target("hickory_filter", level);
    if log.query_log {
        // Query lines were asked for: they show even at a quieter level.
        targets = targets.with_target(QUERY_TARGET, level.max(LevelFilter::INFO));
    }
    let layer = fmt::layer().with_writer(writer).with_ansi(false);
    let layer: Box<dyn Layer<Registry> + Send + Sync> = match log.format {
        LogFormat::Text => Box::new(layer),
        LogFormat::Json => Box::new(layer.json().flatten_event(true)),
    };
    Box::new(Registry::default().with(layer.with_filter(targets)))
}

/// Install the global subscriber, writing to stderr.
pub fn init(log: &Log) -> Result<(), tracing::subscriber::SetGlobalDefaultError> {
    tracing::subscriber::set_global_default(subscriber(log, std::io::stderr))
}

/// A client address with its host part masked: the last octet of an IPv4
/// address zeroed, an IPv6 address cut to its first 48 bits (an IPv4-mapped
/// IPv6 address is masked as the IPv4 address it carries).
pub fn mask_client(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            format!("{a}.{b}.{c}.0")
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => mask_client(IpAddr::V4(v4)),
            None => {
                let [a, b, c, ..] = v6.segments();
                Ipv6Addr::new(a, b, c, 0, 0, 0, 0, 0).to_string()
            }
        },
    }
}

/// How `ip` appears in a query line under `mode`, or `None` when omitted.
pub fn client_text(mode: QueryLogClient, ip: IpAddr) -> Option<String> {
    match mode {
        QueryLogClient::Address => Some(ip.to_string()),
        QueryLogClient::Masked => Some(mask_client(ip)),
        QueryLogClient::Omit => None,
    }
}

/// The conventional name of a response code (`NOERROR`, `NXDOMAIN`, ...).
pub fn rcode_name(code: ResponseCode) -> String {
    match code {
        ResponseCode::NoError => "NOERROR".to_owned(),
        ResponseCode::FormErr => "FORMERR".to_owned(),
        ResponseCode::ServFail => "SERVFAIL".to_owned(),
        ResponseCode::NXDomain => "NXDOMAIN".to_owned(),
        ResponseCode::NotImp => "NOTIMP".to_owned(),
        ResponseCode::Refused => "REFUSED".to_owned(),
        other => format!("RCODE{}", u16::from(other)),
    }
}

/// One answered query, as the query log sees it.
pub struct QueryEvent<'a> {
    pub client: IpAddr,
    pub name: &'a LowerName,
    pub record_type: RecordType,
    pub outcome: &'static str,
    pub rcode: ResponseCode,
    pub elapsed: Duration,
}

/// The query-log settings.
#[derive(Debug, Clone, Copy)]
pub struct QueryLog {
    enabled: bool,
    client: QueryLogClient,
}

impl Default for QueryLog {
    fn default() -> Self {
        Self {
            enabled: false,
            client: QueryLogClient::Address,
        }
    }
}

macro_rules! emit {
    ($level:expr, $client_mode:expr, $event:expr) => {{
        let client = client_text($client_mode, $event.client);
        let name = $event.name.to_string();
        tracing::event!(
            target: QUERY_TARGET,
            $level,
            client = client.as_deref(),
            name = name.trim_end_matches('.'),
            r#type = %$event.record_type,
            outcome = $event.outcome,
            rcode = %rcode_name($event.rcode),
            duration_ms = $event.elapsed.as_secs_f64() * 1000.0,
            "query"
        );
    }};
}

impl QueryLog {
    pub fn from_config(log: &Log) -> Self {
        Self {
            enabled: log.query_log,
            client: log.query_log_client,
        }
    }

    /// Log the query: at `info` when the query log is on, at `debug`
    /// otherwise. Nothing is formatted when the line would be dropped.
    pub fn record(&self, event: &QueryEvent<'_>) {
        if self.enabled {
            if tracing::enabled!(target: QUERY_TARGET, Level::INFO) {
                emit!(Level::INFO, self.client, event);
            }
        } else if tracing::enabled!(target: QUERY_TARGET, Level::DEBUG) {
            emit!(Level::DEBUG, self.client, event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().expect("ip")
    }

    #[test]
    fn ipv4_clients_are_masked_to_their_network() {
        assert_eq!(mask_client(ip("192.0.2.10")), "192.0.2.0");
        assert_eq!(mask_client(ip("10.1.2.255")), "10.1.2.0");
        assert_eq!(mask_client(ip("0.0.0.0")), "0.0.0.0");
    }

    #[test]
    fn ipv6_clients_are_cut_to_their_first_48_bits() {
        assert_eq!(mask_client(ip("2001:db8:1234:5678::9")), "2001:db8:1234::");
        assert_eq!(
            mask_client(ip("2001:db8:1234:5678:9abc:def0:1234:5678")),
            "2001:db8:1234::"
        );
        assert_eq!(mask_client(ip("::1")), "::");
        assert_eq!(mask_client(ip("fe80::1")), "fe80::");
    }

    #[test]
    fn an_ipv4_mapped_ipv6_client_is_masked_as_ipv4() {
        assert_eq!(mask_client(ip("::ffff:192.0.2.10")), "192.0.2.0");
    }

    #[test]
    fn the_client_mode_decides_what_is_logged() {
        let client = ip("192.0.2.10");
        assert_eq!(
            client_text(QueryLogClient::Address, client).as_deref(),
            Some("192.0.2.10")
        );
        assert_eq!(
            client_text(QueryLogClient::Masked, client).as_deref(),
            Some("192.0.2.0")
        );
        assert_eq!(client_text(QueryLogClient::Omit, client), None);
    }

    #[test]
    fn response_codes_have_their_conventional_names() {
        assert_eq!(rcode_name(ResponseCode::NoError), "NOERROR");
        assert_eq!(rcode_name(ResponseCode::NXDomain), "NXDOMAIN");
        assert_eq!(rcode_name(ResponseCode::ServFail), "SERVFAIL");
        assert_eq!(rcode_name(ResponseCode::Refused), "REFUSED");
        assert_eq!(rcode_name(ResponseCode::FormErr), "FORMERR");
        assert_eq!(rcode_name(ResponseCode::NotImp), "NOTIMP");
    }
}
