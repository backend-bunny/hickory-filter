//! README.md and the code and example config agree: every configuration key,
//! its default, the resolver forms, and the Quick start configuration.

use std::{collections::BTreeSet, path::Path, process::Command};

use hickory_filter::config::{Config, Resolver};

mod common;
use common::{backticked, readme, readme_table};

const EXAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/hickory-filter.toml");

/// Every key the example config shows, written or commented out, as a path:
/// `listen`, `log.level`, `filter.lists[].rule`.
fn example_keys() -> BTreeSet<String> {
    let text = std::fs::read_to_string(EXAMPLE).expect("the example config");
    let key = regex::Regex::new(r"^#?\s*([a-z_0-9]+) = ").unwrap();
    let mut table = String::new();
    let mut keys = BTreeSet::new();
    for line in text.lines() {
        // A commented-out table header (`# [recursion]`) still opens its table.
        let line = line
            .strip_prefix("# ")
            .filter(|rest| rest.starts_with('['))
            .unwrap_or(line);
        if let Some(name) = line.strip_prefix("[[").and_then(|l| l.strip_suffix("]]")) {
            table = format!("{name}[]");
        } else if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            table = name.to_owned();
        } else if let Some(captures) = key.captures(line) {
            let name = &captures[1];
            keys.insert(if table.is_empty() {
                name.to_owned()
            } else {
                format!("{table}.{name}")
            });
        }
    }
    keys
}

fn reference_rows() -> Vec<Vec<String>> {
    readme_table("### Configuration reference")
}

fn documented_keys() -> BTreeSet<String> {
    reference_rows()
        .iter()
        .map(|row| backticked(&row[0])[0].clone())
        .collect()
}

#[test]
fn the_documented_keys_are_exactly_the_keys_of_the_example_config() {
    let documented = documented_keys();
    let example = example_keys();
    let undocumented: Vec<_> = example.difference(&documented).collect();
    let missing: Vec<_> = documented.difference(&example).collect();
    assert!(
        undocumented.is_empty() && missing.is_empty(),
        "in the example config but not in the README: {undocumented:?}; \
         in the README but not in the example config: {missing:?}"
    );
}

#[test]
fn every_documented_key_is_accepted_by_the_config() {
    // `deny_unknown_fields` rejects a key the code doesn't have, so a README key
    // that was renamed in the code fails here.
    let snippets = [
        ("listen", r#"listen = ["127.0.0.1:1053"]"#),
        ("cache_size", "cache_size = 10"),
        ("http", r#"http = "127.0.0.1:8080""#),
        ("log.level", "[log]\nlevel = \"debug\""),
        ("log.format", "[log]\nformat = \"json\""),
        ("log.query_log", "[log]\nquery_log = true"),
        (
            "log.query_log_client",
            "[log]\nquery_log_client = \"masked\"",
        ),
        ("filter.refresh", "[filter]\nrefresh = \"1h\""),
        ("filter.bootstrap", "[filter]\nbootstrap = \"192.0.2.53\""),
        ("filter.cache_dir", "[filter]\ncache_dir = \"/var/cache/x\""),
        (
            "filter.rules",
            "[filter]\nrules = [\"block domain a.example\"]",
        ),
        (
            "filter.response.mode",
            "[filter.response]\nmode = \"nodata\"",
        ),
        (
            "filter.response.ipv4",
            "[filter.response]\nmode = \"address\"\nipv4 = \"192.0.2.1\"",
        ),
        (
            "filter.response.ipv6",
            "[filter.response]\nmode = \"address\"\nipv6 = \"2001:db8::1\"",
        ),
        ("filter.response.ttl", "[filter.response]\nttl = 60"),
        (
            "filter.lists[].refresh",
            "[[filter.lists]]\nrule = \"block list domain file:///x\"\nrefresh = \"1h\"",
        ),
        (
            "filter.lists[].required",
            "[[filter.lists]]\nrule = \"block list domain file:///x\"\nrequired = true",
        ),
        ("mode", "mode = \"forward\""),
        (
            "recursion.roots",
            "[recursion]\nroots = \"/tmp/root.hints\"",
        ),
        ("recursion.dnssec", "[recursion]\ndnssec = \"off\""),
        (
            "recursion.qname_minimization",
            "[recursion]\nqname_minimization = \"relaxed\"",
        ),
        (
            "recursion.opportunistic_encryption",
            "[recursion]\nopportunistic_encryption = false",
        ),
        (
            "recursion.state_file",
            "[recursion]\nstate_file = \"/tmp/tls.toml\"",
        ),
    ];
    let documented = documented_keys();
    for (key, snippet) in snippets {
        assert!(documented.contains(key), "{key} is not documented");
        // Top-level keys must come before tables.
        let text = if key.starts_with("recursion.") {
            // Recursive mode takes no upstreams.
            format!("mode = \"recursive\"\n{snippet}\n")
        } else if snippet.contains('[') {
            format!("upstreams = [\"192.0.2.53\"]\n{snippet}\n")
        } else {
            format!("{snippet}\nupstreams = [\"192.0.2.53\"]\n")
        };
        Config::parse(Path::new("readme.toml"), &text)
            .unwrap_or_else(|e| panic!("{key} is documented but not accepted: {e}"));
    }
    // Every documented key has a snippet above, or is one of the two that
    // every working config needs.
    let covered: BTreeSet<&str> = snippets.iter().map(|(key, _)| *key).collect();
    for key in &documented {
        assert!(
            covered.contains(key.as_str())
                || ["upstreams", "filter.lists[].rule"].contains(&key.as_str()),
            "{key} is documented but has no acceptance check here"
        );
    }
}

#[test]
fn the_documented_defaults_are_the_defaults_in_the_code() {
    let config = Config::parse(
        Path::new("defaults.toml"),
        "upstreams = [\"192.0.2.53\"]\n[[filter.lists]]\nrule = \"block list domain file:///x\"\n",
    )
    .expect("a minimal config");
    let lower = |debug: String| debug.to_lowercase();
    let secs = config.filter.refresh.0.as_secs();
    let expected: Vec<(&str, Vec<String>)> = vec![
        (
            "listen",
            config.listen.iter().map(ToString::to_string).collect(),
        ),
        ("mode", vec![lower(format!("{:?}", config.mode))]),
        ("cache_size", vec![config.cache_size.to_string()]),
        (
            "recursion.roots",
            vec![hickory_filter::config::DEFAULT_ROOT_HINTS.to_owned()],
        ),
        (
            "recursion.dnssec",
            vec![lower(format!("{:?}", config.recursion().dnssec))],
        ),
        (
            "recursion.qname_minimization",
            vec![lower(format!(
                "{:?}",
                config.recursion().qname_minimization
            ))],
        ),
        (
            "recursion.opportunistic_encryption",
            vec![config.recursion().opportunistic_encryption.to_string()],
        ),
        (
            "http",
            vec![match config.http {
                hickory_filter::config::Http::Listen(addr) => addr.to_string(),
                hickory_filter::config::Http::Disabled => "false".to_owned(),
            }],
        ),
        ("log.level", vec![lower(format!("{:?}", config.log.level))]),
        (
            "log.format",
            vec![lower(format!("{:?}", config.log.format))],
        ),
        ("log.query_log", vec![config.log.query_log.to_string()]),
        (
            "log.query_log_client",
            vec![lower(format!("{:?}", config.log.query_log_client))],
        ),
        ("filter.refresh", vec![format!("{}h", secs / 3600)]),
        (
            "filter.response.mode",
            vec![lower(format!("{:?}", config.filter.response.mode))],
        ),
        (
            "filter.response.ttl",
            vec![config.filter.response.ttl.to_string()],
        ),
        (
            "filter.lists[].required",
            vec![config.filter.lists[0].required.to_string()],
        ),
    ];
    let rows = reference_rows();
    for (key, want) in expected {
        let row = rows
            .iter()
            .find(|row| backticked(&row[0])[0] == key)
            .unwrap_or_else(|| panic!("{key} is not documented"));
        assert_eq!(backticked(&row[1]), want, "the documented default of {key}");
    }
    // The keys without a default say so, in words rather than backticks.
    for key in [
        "upstreams",
        "filter.bootstrap",
        "filter.cache_dir",
        "filter.rules",
        "recursion.state_file",
    ] {
        let row = rows
            .iter()
            .find(|row| backticked(&row[0])[0] == key)
            .unwrap();
        assert!(
            backticked(&row[1]).is_empty(),
            "{key} has no default value: {row:?}"
        );
    }
}

#[test]
fn every_documented_resolver_form_parses() {
    let rows = readme_table("### Resolver addresses");
    assert!(rows.len() >= 4);
    for row in rows {
        for form in backticked(&row[0]) {
            form.parse::<Resolver>()
                .unwrap_or_else(|e| panic!("documented resolver form {form}: {e}"));
        }
    }
}

#[test]
fn the_quick_start_configuration_passes_check() {
    let readme = readme();
    let start = readme
        .find("## Quick start")
        .expect("a Quick start section");
    let block = readme[start..]
        .split("```toml\n")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("a toml block in the Quick start");
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, block).expect("write");

    let output = Command::new(env!("CARGO_BIN_EXE_hickory-filter"))
        .arg("--config")
        .arg(&path)
        .arg("--check")
        .output()
        .expect("run hickory-filter");
    assert!(
        output.status.success(),
        "the Quick start config fails --check: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn the_quick_start_commands_use_options_that_exist() {
    let readme = readme();
    let help = Command::new(env!("CARGO_BIN_EXE_hickory-filter"))
        .arg("--help")
        .output()
        .expect("run hickory-filter");
    let help = String::from_utf8_lossy(&help.stdout).into_owned();
    for option in ["--config", "--check", "--version", "--help"] {
        assert!(readme.contains(option), "the README mentions {option}");
        assert!(help.contains(option), "the binary's help mentions {option}");
    }
}
