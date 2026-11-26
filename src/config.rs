//! The configuration file: one TOML file, strict about unknown keys.
//!
//! Rules are kept as the text lines they are written as. Loading a
//! configuration runs every line through the rule-line parser, so a bad rule
//! stops startup (and fails `--check`) with the rule named.

use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use serde::{
    Deserialize, Deserializer,
    de::{self, Visitor},
};

use crate::rules::{RuleLine, parse_rule_line};

/// Where the server looks for its configuration when `--config` isn't given.
pub const DEFAULT_CONFIG_PATH: &str = "/etc/hickory-filter/config.toml";

/// Why a configuration could not be loaded. Every variant names the file.
#[derive(Debug)]
pub enum ConfigError {
    Read {
        file: PathBuf,
        source: std::io::Error,
    },
    Parse {
        file: PathBuf,
        message: String,
    },
    Invalid {
        file: PathBuf,
        key: &'static str,
        reason: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { file, source } => write!(f, "{}: cannot read: {source}", file.display()),
            Self::Parse { file, message } => {
                write!(f, "{}: {}", file.display(), message.trim_end())
            }
            Self::Invalid { file, key, reason } => {
                write!(f, "{}: `{key}`: {reason}", file.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// The whole configuration file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// DNS listen addresses (UDP and TCP). Default: port 1053 on all addresses.
    #[serde(default = "default_listen")]
    pub listen: Vec<SocketAddr>,
    /// Upstream resolvers; at least one is required.
    #[serde(default)]
    pub upstreams: Vec<Resolver>,
    /// The most forwarded answers kept in the response cache; `0` disables it.
    /// Default 4096.
    #[serde(default = "default_cache_size")]
    pub cache_size: u64,
    #[serde(default)]
    pub http: Http,
    #[serde(default)]
    pub filter: Filter,
    #[serde(default)]
    pub log: Log,
}

impl Config {
    /// Read, parse and validate the file at `path`.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            file: path.to_owned(),
            source,
        })?;
        Self::parse(path, &text)
    }

    /// Parse and validate `text`; `file` is only used in error messages.
    pub fn parse(file: &Path, text: &str) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(text).map_err(|e| ConfigError::Parse {
            file: file.to_owned(),
            message: e.to_string(),
        })?;
        config.validate(file)?;
        Ok(config)
    }

    /// The inline rules (`filter.rules`) as parsed rules. Every rule was
    /// validated when the configuration loaded, so none is dropped here.
    pub fn inline_rules(&self) -> Vec<crate::rules::Rule> {
        self.filter
            .rules
            .iter()
            .filter_map(|line| match crate::rules::parse_rule_line(line) {
                Ok(crate::rules::RuleLine::Rule(rule)) => Some(rule),
                _ => None,
            })
            .collect()
    }

    fn validate(&self, file: &Path) -> Result<(), ConfigError> {
        let invalid = |key: &'static str, reason: &str| ConfigError::Invalid {
            file: file.to_owned(),
            key,
            reason: reason.to_owned(),
        };
        if self.listen.is_empty() {
            return Err(invalid("listen", "at least one listen address is required"));
        }
        if self.upstreams.is_empty() {
            return Err(invalid(
                "upstreams",
                "at least one upstream resolver is required",
            ));
        }
        let response = &self.filter.response;
        match response.mode {
            ResponseMode::Address => {
                if response.ipv4.is_none() && response.ipv6.is_none() {
                    return Err(invalid(
                        "filter.response",
                        "mode `address` needs `ipv4`, `ipv6` or both",
                    ));
                }
            }
            _ => {
                if response.ipv4.is_some() || response.ipv6.is_some() {
                    return Err(invalid(
                        "filter.response",
                        "`ipv4` and `ipv6` only apply to mode `address`",
                    ));
                }
            }
        }
        for rule in &self.filter.rules {
            match parse_rule_line(rule) {
                Ok(RuleLine::Rule(_)) => {}
                Ok(RuleLine::List(_)) => {
                    return Err(invalid(
                        "filter.rules",
                        &format!(
                            "rule `{rule}`: lists are declared in `[[filter.lists]]`, not in `rules`"
                        ),
                    ));
                }
                Err(e) => return Err(invalid("filter.rules", &e.to_string())),
            }
        }
        for list in &self.filter.lists {
            match parse_rule_line(&list.rule) {
                Ok(RuleLine::List(_)) => {}
                Ok(RuleLine::Rule(_)) => {
                    return Err(invalid(
                        "filter.lists",
                        &format!(
                            "rule `{}`: a list entry must be `<action> list <type> <url>`",
                            list.rule
                        ),
                    ));
                }
                Err(e) => return Err(invalid("filter.lists", &e.to_string())),
            }
        }
        Ok(())
    }
}

fn default_listen() -> Vec<SocketAddr> {
    vec![
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 1053),
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 1053),
    ]
}

fn default_cache_size() -> u64 {
    4096
}

/// An upstream or bootstrap resolver: an IP address with optional port, over
/// plain DNS or DNS-over-TLS.
///
/// Syntax: `[dns://]ip[:port]` (default port 53) or `tls://ip[:port]#name`
/// (default port 853; `name` is the server name the certificate is verified
/// against). IPv6 addresses with a port are written `[::1]:53`. Host names
/// are rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolver {
    pub addr: SocketAddr,
    /// `Some(name)` for DNS-over-TLS, `None` for plain DNS.
    pub tls_name: Option<String>,
}

impl FromStr for Resolver {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let (scheme, rest) = s.split_once("://").unwrap_or(("dns", s));
        let (hostport, name) = match rest.split_once('#') {
            Some((hostport, name)) => (hostport, Some(name)),
            None => (rest, None),
        };
        let (tls, default_port) = match scheme {
            "dns" => (false, 53),
            "tls" => (true, 853),
            other => {
                return Err(format!(
                    "unsupported scheme `{other}://`; use `dns://` or `tls://`"
                ));
            }
        };
        let addr = parse_addr(hostport, default_port)?;
        let tls_name = match (tls, name) {
            (true, Some(name)) => {
                validate_server_name(name)?;
                Some(name.to_owned())
            }
            (true, None) => {
                return Err(
                    "`tls://` needs the server name to verify, as in `tls://192.0.2.53#dns.example`"
                        .to_owned(),
                );
            }
            (false, Some(_)) => {
                return Err("a server name (`#name`) only applies to `tls://`".to_owned());
            }
            (false, None) => None,
        };
        Ok(Self { addr, tls_name })
    }
}

fn parse_addr(s: &str, default_port: u16) -> Result<SocketAddr, String> {
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Ok(addr);
    }
    let bare = s
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(s);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, default_port));
    }
    Err(format!(
        "`{s}` is not an IP address with an optional port (host names are not accepted)"
    ))
}

fn validate_server_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 253
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(format!("`{name}` is not a valid TLS server name"))
    }
}

impl<'de> Deserialize<'de> for Resolver {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(de::Error::custom)
    }
}

/// The HTTP endpoint for metrics, health and readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Http {
    Disabled,
    Listen(SocketAddr),
}

impl Default for Http {
    fn default() -> Self {
        Self::Listen(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080))
    }
}

impl<'de> Deserialize<'de> for Http {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Http;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an address such as \"0.0.0.0:8080\", or `false` to disable")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Http, E> {
                v.parse()
                    .map(Http::Listen)
                    .map_err(|e| E::custom(format!("invalid address `{v}`: {e}")))
            }

            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Http, E> {
                if v {
                    Err(E::custom(
                        "`true` is not an address; give one such as \"0.0.0.0:8080\", or `false` to disable",
                    ))
                } else {
                    Ok(Http::Disabled)
                }
            }
        }
        deserializer.deserialize_any(V)
    }
}

/// A refresh interval. `0` means disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval(pub Duration);

impl Interval {
    pub fn is_disabled(self) -> bool {
        self.0.is_zero()
    }
}

impl<'de> Deserialize<'de> for Interval {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Interval;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a duration such as \"6h\" or \"90m\", or 0 to disable")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Interval, E> {
                if v.trim() == "0" {
                    return Ok(Interval(Duration::ZERO));
                }
                humantime::parse_duration(v)
                    .map(Interval)
                    .map_err(|e| E::custom(format!("invalid duration `{v}`: {e}")))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Interval, E> {
                if v == 0 {
                    Ok(Interval(Duration::ZERO))
                } else {
                    Err(E::custom(format!(
                        "a bare number must be 0 (disabled); write a unit, as in \"{v}s\""
                    )))
                }
            }
        }
        deserializer.deserialize_any(V)
    }
}

/// How verbose the logs are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

/// How log lines are written to stderr.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
}

/// How the client is written in query log lines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueryLogClient {
    /// The client's address.
    #[default]
    Address,
    /// The address with the host part masked: the last octet of an IPv4
    /// address zeroed, an IPv6 address cut to its first 48 bits.
    Masked,
    /// No client field at all.
    Omit,
}

/// The `[log]` table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Log {
    /// `error`, `warn`, `info` (the default), `debug` or `trace`.
    #[serde(default)]
    pub level: LogLevel,
    /// `text` (the default) or `json`.
    #[serde(default)]
    pub format: LogFormat,
    /// Log one line per answered query. Off by default; without it queries are
    /// only logged at the `debug` level.
    #[serde(default)]
    pub query_log: bool,
    /// How the client appears in query lines.
    #[serde(default)]
    pub query_log_client: QueryLogClient,
}

/// The `[filter]` table.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    /// Default refresh interval for lists. Default: 24 hours; `0` disables.
    #[serde(default = "default_refresh")]
    pub refresh: Interval,
    #[serde(default)]
    pub response: Response,
    /// Resolver used for list host names; the system resolver when absent.
    #[serde(default)]
    pub bootstrap: Option<Resolver>,
    /// Directory for the optional on-disk list cache.
    #[serde(default)]
    pub cache_dir: Option<PathBuf>,
    /// Inline rule lines, such as `block wildcard *.ads.example`.
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub lists: Vec<List>,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            refresh: default_refresh(),
            response: Response::default(),
            bootstrap: None,
            cache_dir: None,
            rules: Vec::new(),
            lists: Vec::new(),
        }
    }
}

fn default_refresh() -> Interval {
    Interval(Duration::from_secs(24 * 60 * 60))
}

/// One `[[filter.lists]]` entry.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct List {
    /// A rule line declaring the list, such as
    /// `block list hosts https://lists.example/hosts.txt`.
    pub rule: String,
    /// Overrides `filter.refresh` for this list.
    #[serde(default)]
    pub refresh: Option<Interval>,
    /// When true, startup fails if the list can't be loaded.
    #[serde(default)]
    pub required: bool,
}

/// What a blocked query is answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResponseMode {
    Null,
    Address,
    Nodata,
    Nxdomain,
}

/// The blocked-response settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    #[serde(default = "default_mode")]
    pub mode: ResponseMode,
    /// Address for blocked A queries in `address` mode.
    #[serde(default)]
    pub ipv4: Option<Ipv4Addr>,
    /// Address for blocked AAAA queries in `address` mode.
    #[serde(default)]
    pub ipv6: Option<Ipv6Addr>,
    /// TTL of blocked answers, in seconds. Default 3600.
    #[serde(default = "default_ttl")]
    pub ttl: u32,
}

impl Default for Response {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            ipv4: None,
            ipv6: None,
            ttl: default_ttl(),
        }
    }
}

fn default_mode() -> ResponseMode {
    ResponseMode::Null
}

fn default_ttl() -> u32 {
    3600
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"upstreams = ["192.0.2.53"]"#;

    fn parse(text: &str) -> Result<Config, ConfigError> {
        Config::parse(Path::new("test.toml"), text)
    }

    fn error_text(text: &str) -> String {
        parse(text)
            .expect_err("config should be rejected")
            .to_string()
    }

    #[test]
    fn minimal_config_gets_the_documented_defaults() {
        let config = parse(MINIMAL).expect("valid");
        assert_eq!(config.listen, default_listen());
        assert_eq!(config.cache_size, 4096);
        assert_eq!(config.http, Http::default());
        assert_eq!(config.filter.refresh, Interval(Duration::from_secs(86400)));
        assert_eq!(config.filter.response, Response::default());
        assert_eq!(config.filter.response.mode, ResponseMode::Null);
        assert_eq!(config.filter.response.ttl, 3600);
        assert!(config.filter.bootstrap.is_none());
    }

    #[test]
    fn unknown_key_is_rejected_and_named_with_the_file() {
        let text = error_text(&format!("{MINIMAL}\nlisten_adress = [\"0.0.0.0:53\"]\n"));
        assert!(text.contains("test.toml"), "{text}");
        assert!(text.contains("listen_adress"), "{text}");
    }

    #[test]
    fn unknown_key_in_a_nested_table_is_rejected() {
        let text = error_text(&format!("{MINIMAL}\n[filter]\nrefersh = \"1h\"\n"));
        assert!(text.contains("refersh"), "{text}");
    }

    #[test]
    fn unreadable_file_names_the_file_and_the_reason() {
        let err =
            Config::load(Path::new("/nonexistent/hickory-filter.toml")).expect_err("missing file");
        let text = err.to_string();
        assert!(text.contains("/nonexistent/hickory-filter.toml"), "{text}");
        assert!(text.contains("cannot read"), "{text}");
    }

    #[test]
    fn invalid_duration_names_the_key_and_the_value() {
        let text = error_text(&format!("{MINIMAL}\n[filter]\nrefresh = \"soon\"\n"));
        assert!(text.contains("test.toml"), "{text}");
        assert!(text.contains("refresh"), "{text}");
        assert!(text.contains("invalid duration `soon`"), "{text}");
    }

    #[test]
    fn durations_accept_units_and_zero_disables() {
        let config = parse(&format!(
            "{MINIMAL}\n[filter]\nrefresh = \"90m\"\n[[filter.lists]]\nrule = \"block list domain file:///a\"\nrefresh = 0\n[[filter.lists]]\nrule = \"block list domain file:///b\"\nrefresh = \"0\"\n"
        ))
        .expect("valid");
        assert_eq!(
            config.filter.refresh,
            Interval(Duration::from_secs(90 * 60))
        );
        assert!(config.filter.lists[0].refresh.expect("set").is_disabled());
        assert!(config.filter.lists[1].refresh.expect("set").is_disabled());
        assert!(!config.filter.refresh.is_disabled());
    }

    #[test]
    fn a_bare_nonzero_number_is_not_a_duration() {
        let text = error_text(&format!("{MINIMAL}\n[filter]\nrefresh = 5\n"));
        assert!(text.contains("write a unit"), "{text}");
    }

    #[test]
    fn at_least_one_upstream_is_required() {
        let text = error_text("listen = [\"127.0.0.1:1053\"]\n");
        assert!(text.contains("`upstreams`"), "{text}");
        assert!(text.contains("at least one upstream"), "{text}");
    }

    #[test]
    fn the_response_cache_size_is_configurable_and_zero_disables_it() {
        let config = parse(&format!("{MINIMAL}\ncache_size = 100\n")).expect("valid");
        assert_eq!(config.cache_size, 100);
        let off = parse(&format!("{MINIMAL}\ncache_size = 0\n")).expect("valid");
        assert_eq!(off.cache_size, 0);
        assert!(error_text(&format!("{MINIMAL}\ncache_size = -1\n")).contains("cache_size"));
    }

    #[test]
    fn the_log_table_has_documented_defaults_and_values() {
        let defaults = parse(MINIMAL).expect("valid");
        assert_eq!(defaults.log, Log::default());
        assert_eq!(defaults.log.level, LogLevel::Info);
        assert_eq!(defaults.log.format, LogFormat::Text);
        assert!(!defaults.log.query_log);
        assert_eq!(defaults.log.query_log_client, QueryLogClient::Address);

        let set = parse(&format!(
            "{MINIMAL}\n[log]\nlevel = \"debug\"\nformat = \"json\"\nquery_log = true\nquery_log_client = \"masked\"\n"
        ))
        .expect("valid");
        assert_eq!(set.log.level, LogLevel::Debug);
        assert_eq!(set.log.format, LogFormat::Json);
        assert!(set.log.query_log);
        assert_eq!(set.log.query_log_client, QueryLogClient::Masked);
        let omit =
            parse(&format!("{MINIMAL}\n[log]\nquery_log_client = \"omit\"\n")).expect("valid");
        assert_eq!(omit.log.query_log_client, QueryLogClient::Omit);
    }

    #[test]
    fn invalid_log_values_and_unknown_log_keys_are_rejected() {
        for (line, needle) in [
            ("level = \"loud\"", "loud"),
            ("format = \"xml\"", "xml"),
            ("query_log_client = \"hash\"", "hash"),
            ("query_log = \"yes\"", "query_log"),
            ("quer_log = true", "quer_log"),
        ] {
            let text = error_text(&format!("{MINIMAL}\n[log]\n{line}\n"));
            assert!(text.contains("test.toml"), "{line}: {text}");
            assert!(text.contains(needle), "{line}: {text}");
        }
    }

    #[test]
    fn http_is_an_address_or_false() {
        let on = parse(&format!("{MINIMAL}\nhttp = \"127.0.0.1:9090\"\n")).expect("valid");
        assert_eq!(on.http, Http::Listen("127.0.0.1:9090".parse().unwrap()));
        let off = parse(&format!("{MINIMAL}\nhttp = false\n")).expect("valid");
        assert_eq!(off.http, Http::Disabled);
        assert!(error_text(&format!("{MINIMAL}\nhttp = true\n")).contains("not an address"));
    }

    #[test]
    fn address_mode_needs_an_address_and_other_modes_reject_one() {
        let ok = parse(&format!(
            "{MINIMAL}\n[filter.response]\nmode = \"address\"\nipv4 = \"192.0.2.1\"\nttl = 60\n"
        ))
        .expect("valid");
        assert_eq!(ok.filter.response.ttl, 60);
        assert!(
            error_text(&format!(
                "{MINIMAL}\n[filter.response]\nmode = \"address\"\n"
            ))
            .contains("needs `ipv4`")
        );
        assert!(
            error_text(&format!(
                "{MINIMAL}\n[filter.response]\nmode = \"null\"\nipv4 = \"192.0.2.1\"\n"
            ))
            .contains("only apply to mode `address`")
        );
        assert!(
            error_text(&format!(
                "{MINIMAL}\n[filter.response]\nmode = \"sinkhole\"\n"
            ))
            .contains("sinkhole")
        );
    }

    #[test]
    fn resolver_syntax_valid_forms() {
        let plain = |addr: &str| Resolver {
            addr: addr.parse().unwrap(),
            tls_name: None,
        };
        assert_eq!("192.0.2.53".parse(), Ok(plain("192.0.2.53:53")));
        assert_eq!("192.0.2.53:5353".parse(), Ok(plain("192.0.2.53:5353")));
        assert_eq!("dns://192.0.2.53".parse(), Ok(plain("192.0.2.53:53")));
        assert_eq!(
            "dns://192.0.2.53:5353".parse(),
            Ok(plain("192.0.2.53:5353"))
        );
        assert_eq!("2001:db8::53".parse(), Ok(plain("[2001:db8::53]:53")));
        assert_eq!("[2001:db8::53]".parse(), Ok(plain("[2001:db8::53]:53")));
        assert_eq!(
            "[2001:db8::53]:5353".parse(),
            Ok(plain("[2001:db8::53]:5353"))
        );
        assert_eq!(
            "tls://192.0.2.53#dns.example".parse(),
            Ok(Resolver {
                addr: "192.0.2.53:853".parse().unwrap(),
                tls_name: Some("dns.example".to_owned()),
            })
        );
        assert_eq!(
            "tls://[2001:db8::53]:8853#dns.example".parse(),
            Ok(Resolver {
                addr: "[2001:db8::53]:8853".parse().unwrap(),
                tls_name: Some("dns.example".to_owned()),
            })
        );
    }

    #[test]
    fn resolver_syntax_rejects_host_names_and_bad_forms() {
        for bad in [
            "resolver.example",
            "dns://resolver.example",
            "resolver.example:53",
            "tls://resolver.example#dns.example",
            "192.0.2.53:99999",
            "",
        ] {
            let err = bad.parse::<Resolver>().expect_err(bad);
            assert!(err.contains("IP address"), "{bad}: {err}");
        }
        for (bad, needle) in [
            ("tls://192.0.2.53", "server name"),
            ("dns://192.0.2.53#dns.example", "only applies to `tls://`"),
            ("tls://192.0.2.53#", "valid TLS server name"),
            ("tls://192.0.2.53#bad name", "valid TLS server name"),
            ("https://192.0.2.53", "unsupported scheme"),
        ] {
            let err = bad.parse::<Resolver>().expect_err(bad);
            assert!(err.contains(needle), "{bad}: {err}");
        }
    }

    #[test]
    fn valid_inline_rules_and_lists_load() {
        let config = parse(&format!(
            "{MINIMAL}\n[filter]\nrules = [\"block wildcard ||ads.example^\", \"allow domain ok.ads.example\", \"block regex ^ad[0-9]+\\\\.\"]\n[[filter.lists]]\nrule = \"block list hosts https://lists.example/hosts.txt\"\n"
        ))
        .expect("valid");
        assert_eq!(config.filter.rules.len(), 3);
    }

    #[test]
    fn a_bad_inline_rule_fails_startup_naming_file_key_and_rule() {
        for (rule, needle) in [
            ("block glob *.example", "accepted types"),
            ("block wildcard ||exa mple^", "not a valid wildcard"),
            ("block regex (unclosed", "regular expression"),
            ("block domain", "missing value"),
        ] {
            let text = error_text(&format!("{MINIMAL}\n[filter]\nrules = [\"{rule}\"]\n"));
            assert!(text.contains("test.toml"), "{text}");
            assert!(text.contains("`filter.rules`"), "{text}");
            assert!(text.contains(rule), "{text}");
            assert!(text.contains(needle), "{rule}: {text}");
        }
    }

    #[test]
    fn lists_are_declared_in_lists_not_in_rules() {
        let text = error_text(&format!(
            "{MINIMAL}\n[filter]\nrules = [\"block list hosts https://lists.example/h.txt\"]\n"
        ));
        assert!(text.contains("[[filter.lists]]"), "{text}");
    }

    #[test]
    fn a_bad_list_declaration_fails_startup() {
        let text = error_text(&format!(
            "{MINIMAL}\n[[filter.lists]]\nrule = \"block list hosts ftp://lists.example/a.txt\"\n"
        ));
        assert!(text.contains("`filter.lists`"), "{text}");
        assert!(text.contains("unsupported URL scheme"), "{text}");
        let text = error_text(&format!(
            "{MINIMAL}\n[[filter.lists]]\nrule = \"block domain ads.example\"\n"
        ));
        assert!(
            text.contains("must be `<action> list <type> <url>`"),
            "{text}"
        );
    }

    #[test]
    fn bootstrap_given_a_name_fails_startup() {
        let text = error_text(&format!(
            "{MINIMAL}\n[filter]\nbootstrap = \"dns://resolver.example\"\n"
        ));
        assert!(text.contains("bootstrap"), "{text}");
        assert!(text.contains("IP address"), "{text}");
    }
}
