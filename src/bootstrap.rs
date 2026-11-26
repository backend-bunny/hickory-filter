//! The bootstrap resolver: the only path list host names take when one is
//! configured.
//!
//! It plugs into the HTTP client as its DNS resolver and sends every lookup to
//! one configured resolver, over plain DNS or DNS-over-TLS (verifying the
//! server name). It never reads the hosts file or the system configuration,
//! and never asks this server, so a list host can't be filtered or loop back
//! into us. Without a bootstrap resolver the client uses the system resolver.

use std::{fmt, net::SocketAddr, sync::Arc};

use hickory_resolver::{
    Resolver, TokioResolver,
    config::{ConnectionConfig, NameServerConfig, ResolveHosts, ResolverConfig},
    net::runtime::TokioRuntimeProvider,
};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::config;

/// Why the bootstrap resolver could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapError(String);

impl fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot build the bootstrap resolver: {}", self.0)
    }
}

impl std::error::Error for BootstrapError {}

/// The name server a configured resolver describes: plain DNS over UDP and TCP
/// on its port, or DNS-over-TLS on its port with the server name to verify.
pub fn name_server(resolver: &config::Resolver) -> NameServerConfig {
    let connections = match &resolver.tls_name {
        Some(name) => {
            let mut tls = ConnectionConfig::tls(Arc::from(name.as_str()));
            tls.port = resolver.addr.port();
            vec![tls]
        }
        None => {
            let mut udp = ConnectionConfig::udp();
            udp.port = resolver.addr.port();
            let mut tcp = ConnectionConfig::tcp();
            tcp.port = resolver.addr.port();
            vec![udp, tcp]
        }
    };
    NameServerConfig::new(resolver.addr.ip(), true, connections)
}

/// A DNS resolver for the HTTP client that asks only the configured resolver.
pub struct BootstrapResolver {
    resolver: Arc<TokioResolver>,
}

impl BootstrapResolver {
    pub fn new(resolver: &config::Resolver) -> Result<Self, BootstrapError> {
        let config = ResolverConfig::from_name_servers(vec![name_server(resolver)]);
        let mut builder = Resolver::builder_with_config(config, TokioRuntimeProvider::default());
        // Only the configured resolver answers: no hosts file either.
        builder.options_mut().use_hosts_file = ResolveHosts::Never;
        let resolver = builder.build().map_err(|e| BootstrapError(e.to_string()))?;
        Ok(Self {
            resolver: Arc::new(resolver),
        })
    }
}

impl Resolve for BootstrapResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let resolver = Arc::clone(&self.resolver);
        Box::pin(async move {
            let lookup = resolver.lookup_ip(name.as_str()).await?;
            // The client fills in the port.
            let addrs: Vec<SocketAddr> = lookup.iter().map(|ip| SocketAddr::new(ip, 0)).collect();
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use hickory_resolver::config::ProtocolConfig;

    use super::*;

    fn resolver(text: &str) -> config::Resolver {
        text.parse().expect("resolver")
    }

    #[test]
    fn plain_dns_uses_udp_and_tcp_on_the_given_port() {
        let ns = name_server(&resolver("192.0.2.53:5353"));
        assert_eq!(ns.ip, "192.0.2.53".parse::<std::net::IpAddr>().unwrap());
        assert_eq!(ns.connections.len(), 2);
        assert!(ns.connections.iter().all(|c| c.port == 5353));
        assert!(matches!(ns.connections[0].protocol, ProtocolConfig::Udp));
        assert!(matches!(ns.connections[1].protocol, ProtocolConfig::Tcp));
    }

    #[test]
    fn the_default_ports_are_53_and_853() {
        let plain = name_server(&resolver("192.0.2.53"));
        assert!(plain.connections.iter().all(|c| c.port == 53));
        let tls = name_server(&resolver("tls://192.0.2.53#dns.example"));
        assert_eq!(tls.connections.len(), 1);
        assert_eq!(tls.connections[0].port, 853);
    }

    #[test]
    fn dns_over_tls_carries_the_server_name_to_verify() {
        let ns = name_server(&resolver("tls://[2001:db8::53]:8853#dns.example"));
        assert_eq!(ns.connections.len(), 1);
        assert_eq!(ns.connections[0].port, 8853);
        match &ns.connections[0].protocol {
            ProtocolConfig::Tls { server_name } => assert_eq!(&**server_name, "dns.example"),
            other => panic!("{other:?}"),
        }
    }
}
