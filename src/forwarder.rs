//! Forwarding queries that aren't blocked to the configured upstreams.
//!
//! A `Catalog` with one `ForwardZoneHandler` on the root, asking only the
//! configured upstreams (plain DNS or DNS-over-TLS with a verified server
//! name) and keeping a bounded cache of their answers. It never consults the
//! hosts file. When no upstream answers in time the client gets SERVFAIL.
//! Filtering happens in front of it (`FilterHandler`), before the cache, so a
//! rule change applies to the very next query even for a cached name.

use std::{fmt, sync::Arc, time::Duration};

use hickory_server::{
    proto::rr::{LowerName, Name},
    resolver::config::ResolveHosts,
    store::forwarder::{ForwardConfig, ForwardZoneHandler},
    zone_handler::Catalog,
};

use crate::{bootstrap::name_server, config::Config};

/// How long one attempt at one upstream may take, and how many attempts are
/// made, so a dead upstream turns into SERVFAIL in about four seconds, before
/// a typical client gives up.
pub const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(2);
pub const UPSTREAM_ATTEMPTS: usize = 2;

/// Why the forwarder could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwarderError(String);

impl fmt::Display for ForwarderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot build the forwarder: {}", self.0)
    }
}

impl std::error::Error for ForwarderError {}

/// The catalog that forwards every query to the configured upstreams.
pub fn build_catalog(config: &Config) -> Result<Catalog, ForwarderError> {
    let name_servers = config.upstreams.iter().map(name_server).collect();
    let mut builder = ForwardZoneHandler::builder_tokio(ForwardConfig {
        name_servers,
        options: None,
    });
    let options = builder.options_mut();
    options.cache_size = config.cache_size;
    options.timeout = UPSTREAM_TIMEOUT;
    options.attempts = UPSTREAM_ATTEMPTS;
    // Answer from the upstreams, never from this machine's hosts file.
    options.use_hosts_file = ResolveHosts::Never;
    let forwarder = builder.build().map_err(ForwarderError)?;

    let mut catalog = Catalog::new();
    catalog.upsert(LowerName::new(&Name::root()), vec![Arc::new(forwarder)]);
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn config(upstreams: &str, extra: &str) -> Config {
        Config::parse(
            Path::new("test.toml"),
            &format!("upstreams = [{upstreams}]\n{extra}\n"),
        )
        .expect("config")
    }

    #[tokio::test]
    async fn plain_and_dns_over_tls_upstreams_build_without_touching_the_network() {
        let config = config(
            "\"192.0.2.53\", \"192.0.2.54:5353\", \"tls://192.0.2.55#dns.example\"",
            "",
        );
        assert!(build_catalog(&config).is_ok());
    }

    #[tokio::test]
    async fn the_cache_size_comes_from_the_config() {
        for size in [0, 1, 100_000] {
            let config = config("\"192.0.2.53\"", &format!("cache_size = {size}"));
            assert!(build_catalog(&config).is_ok(), "cache_size = {size}");
        }
    }

    #[test]
    fn a_dead_upstream_becomes_servfail_before_a_typical_client_gives_up() {
        let worst_case = UPSTREAM_TIMEOUT * u32::try_from(UPSTREAM_ATTEMPTS).unwrap();
        assert!(worst_case < Duration::from_secs(5), "{worst_case:?}");
    }
}
