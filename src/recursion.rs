//! Recursive mode: resolving queries that aren't blocked from the root servers.
//!
//! A `Catalog` with one `RecursiveZoneHandler` on the root, which starts from
//! the root server hints, follows delegations down to the authoritative
//! servers and caches what it learns. No third-party resolver is asked.
//! Filtering happens in front of it (`FilterHandler`), exactly as in forward
//! mode.

use std::{fmt, net::IpAddr, path::Path, sync::Arc};

use hickory_server::{
    net::runtime::TokioRuntimeProvider,
    proto::{
        rr::{LowerName, Name, RData, RecordSet},
        serialize::txt::Parser,
    },
    resolver::recursor::{
        DnssecPolicyConfig, QNameMinimization as LibQnameMinimization, RecursiveConfig,
        RecursorOptions,
    },
    store::recursor::RecursiveZoneHandler,
    zone_handler::{Catalog, ZoneType},
};

use crate::config::{Config, Dnssec, QnameMinimization, Recursion};

/// Why recursive mode could not be started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecursionError(String);

impl fmt::Display for RecursionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot start recursive mode: {}", self.0)
    }
}

impl std::error::Error for RecursionError {}

/// The addresses of the root servers in the hints file at `path`. A file that
/// is missing, unreadable, malformed or lists no address is an error naming it.
pub fn read_root_hints(path: &Path) -> Result<Vec<IpAddr>, RecursionError> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        RecursionError(format!(
            "root hints file {}: cannot read: {e}",
            path.display()
        ))
    })?;
    let (_origin, records) = Parser::new(text, Some(path.to_owned()), Some(Name::root()))
        .parse()
        .map_err(|e| RecursionError(format!("root hints file {}: {e}", path.display())))?;
    let addresses: Vec<IpAddr> = records
        .values()
        .flat_map(RecordSet::records_without_rrsigs)
        .filter_map(|record| RData::ip_addr(&record.data))
        .collect();
    if addresses.is_empty() {
        return Err(RecursionError(format!(
            "root hints file {}: lists no root server address",
            path.display()
        )));
    }
    Ok(addresses)
}

/// The library's configuration for the settings in `recursion`, with the
/// response cache sized by `cache_size`.
fn recursive_config(recursion: &Recursion, cache_size: u64) -> RecursiveConfig {
    let dnssec_policy = match &recursion.dnssec {
        // Not asked for and not processed.
        Dnssec::Off => DnssecPolicyConfig::SecurityUnaware,
        Dnssec::Validate => DnssecPolicyConfig::ValidateWithStaticKey {
            path: None,
            nsec3_soft_iteration_limit: None,
            nsec3_hard_iteration_limit: None,
            validation_cache_size: None,
        },
        Dnssec::Anchor(path) => DnssecPolicyConfig::ValidateWithStaticKey {
            path: Some(path.clone()),
            nsec3_soft_iteration_limit: None,
            nsec3_hard_iteration_limit: None,
            validation_cache_size: None,
        },
    };
    let options = RecursorOptions {
        response_cache_size: cache_size,
        case_randomization: recursion.case_randomization,
        qname_minimization: match recursion.qname_minimization {
            QnameMinimization::Strict => LibQnameMinimization::Strict,
            QnameMinimization::Relaxed => LibQnameMinimization::Relaxed,
        },
        ..RecursorOptions::default()
    };
    RecursiveConfig {
        roots: recursion.roots.clone(),
        dnssec_policy,
        options,
    }
}

/// The catalog that resolves every query from the root servers.
pub async fn build_catalog(config: &Config) -> Result<Catalog, RecursionError> {
    let recursion = config.recursion();
    // Checked here so the error names the file and says what is wrong with it.
    read_root_hints(&recursion.roots)?;
    let handler = RecursiveZoneHandler::try_from_config(
        Name::root(),
        ZoneType::External,
        recursive_config(&recursion, config.cache_size),
        None,
        TokioRuntimeProvider::default(),
    )
    .await
    .map_err(RecursionError)?;

    let mut catalog = Catalog::new();
    catalog.upsert(LowerName::new(&Name::root()), vec![Arc::new(handler)]);
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hints(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("root.hints");
        std::fs::write(&file, text).expect("write");
        (dir, file)
    }

    fn config(extra: &str, hints: &Path) -> Config {
        Config::parse(
            Path::new("test.toml"),
            &format!(
                "mode = \"recursive\"\n{extra}\n[recursion]\nroots = \"{}\"\n",
                hints.display()
            ),
        )
        .expect("valid config")
    }

    const GOOD: &str = ". 3600000 IN NS a.root.\na.root. 3600000 IN A 198.18.0.10\n\
                        a.root. 3600000 IN AAAA 2001:db8::1\n";

    #[test]
    fn valid_hints_give_their_addresses() {
        let (_dir, file) = hints(GOOD);
        let addresses = read_root_hints(&file).expect("valid hints");
        assert_eq!(addresses.len(), 2);
        assert!(addresses.contains(&"198.18.0.10".parse().unwrap()));
    }

    #[test]
    fn a_missing_hints_file_names_the_file() {
        let error = read_root_hints(Path::new("/nonexistent/root.hints")).unwrap_err();
        assert!(
            error.to_string().contains("/nonexistent/root.hints"),
            "{error}"
        );
    }

    #[test]
    fn an_empty_hints_file_names_the_file() {
        let (_dir, file) = hints("");
        let error = read_root_hints(&file).unwrap_err();
        let text = error.to_string();
        assert!(
            text.contains("root.hints") && text.contains("no root server address"),
            "{text}"
        );
    }

    #[test]
    fn hints_without_an_address_are_rejected() {
        let (_dir, file) = hints(". 3600000 IN NS a.root.\n");
        assert!(read_root_hints(&file).is_err());
    }

    #[test]
    fn malformed_hints_are_rejected() {
        let (_dir, file) = hints("this is not a zone file\n");
        assert!(read_root_hints(&file).is_err());
    }

    #[tokio::test]
    async fn the_catalog_builds_from_valid_hints() {
        let (_dir, file) = hints(GOOD);
        assert!(build_catalog(&config("", &file)).await.is_ok());
    }

    #[tokio::test]
    async fn the_cache_size_comes_from_the_config_and_zero_is_accepted() {
        let (_dir, file) = hints(GOOD);
        for size in [0, 1, 4096] {
            let config = config(&format!("cache_size = {size}"), &file);
            assert!(build_catalog(&config).await.is_ok(), "cache_size = {size}");
        }
    }

    #[tokio::test]
    async fn a_missing_hints_file_stops_the_catalog() {
        let config = config("", Path::new("/nonexistent/root.hints"));
        let error = build_catalog(&config).await.map(|_| ()).unwrap_err();
        assert!(
            error.to_string().contains("/nonexistent/root.hints"),
            "{error}"
        );
    }
}
