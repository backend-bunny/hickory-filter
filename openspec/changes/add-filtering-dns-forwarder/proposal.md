# Proposal

## Why

A network-wide ad and tracker blocker has to do more than block a fixed set of names. It has to combine allow and block rules, read the list formats blocklists are published in, and keep those lists current without restarting. This change defines hickory-filter's first release: a DNS forwarder that does all of that, built on the `hickory-server` library.

## What Changes

- New binary `hickory-filter`: a DNS forwarder (UDP and TCP) that checks every query against a filter before forwarding it to the configured upstream resolvers.
- Rule language: `allow` / `block` actions over `domain`, `wildcard` and `regex` rules. Allow always wins. Wildcards accept the spellings `x`, `*.x`, `.x`, `||x^` and `address=/x/#`.
- Lists: rules from `file://`, `http://` and `https://` lists of type `domain`, `hosts`, `wildcard` or `regex`. Lines that can't be parsed are skipped and counted.
- Scheduled refresh of lists, with conditional requests, keeping the last good copy on failure (optionally persisted on disk) and an atomic swap of the rule set.
- Bootstrap resolver (plain DNS or DNS-over-TLS) used only to resolve list hosts.
- Response modes for blocked names: `null` (default), `address`, `nodata`, `nxdomain`, each correct for the query type.
- Operations: Prometheus metrics, `/health`, and a `/ready` endpoint that is ready only after the first rule load.
- Configuration in one TOML file. Rules are written as lines (`block wildcard *.x`) inside it.

## Capabilities

### New Capabilities

- `filter-rules`: rule actions and types, matching semantics, allow precedence, and the syntax of inline rules and list lines.
- `blocklist-sources`: where lists come from, scheduled refresh, conditional fetching, the last good copy and its on-disk cache, and the bootstrap resolver for list hosts.
- `blocked-responses`: the answer returned for a blocked query in each response mode and query type.
- `dns-forwarding`: listeners, startup and configuration validation, and forwarding of queries that aren't blocked.
- `observability`: metrics, health and readiness endpoints, and logging.

### Modified Capabilities

None (first change).

## Impact

- New Rust crate and binary. Dependencies: `hickory-server` and its resolver (pinned to an exact release), `tokio`, an HTTP client with rustls, a regex engine with a multi-pattern set, a lock-free swap of the rule set, `serde` + TOML, and a Prometheus exporter.
- Container runtime contract: non-root, ports ≥ 1024 by default, read-only root filesystem, an optional cache directory.
- No existing behaviour changes. This is a new project.
