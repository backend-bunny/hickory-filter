# Design

## Context

See proposal.md for the motivation and specs/ for the required behaviour. Constraints that shape the approach:

- `hickory-server` is pre-1.0. Its public extension points are the `RequestHandler` trait (the whole request → response), the `ZoneHandler` trait plus `Catalog` (per-zone handler chains), `ForwardZoneHandler` (forwarding with a resolver cache), and `Server` (UDP/TCP/TLS/HTTPS listeners). Their signatures change between minor releases: `RequestHandler::handle_request` differs between the v0.26 line and the current development branch.
- The query path is hot and concurrent. List refreshes are slow and can fail, and lists can hold 10⁵–10⁶ names.

## Goals / Non-Goals

**Goals:**
- One static decision point per query: normalise → allow? → block? → answer or forward.
- The query path takes no locks and does no I/O. A refresh costs the query path one atomic pointer load.
- Behaviour per specs/, tested at the DNS wire level.

**Non-Goals:**
- DoT/DoH/DoQ listeners for clients (later change; `Server` supports them).
- Per-client policies, groups or schedules; a web UI; query log storage.
- Recursive resolution (forwarding only).
- Hot reload of the configuration file itself (rules from lists refresh; config changes need a restart).
- Response modes per list, or per-rule TTLs.

## Decisions

### D1. Wrap the catalog in our own `RequestHandler`
`FilterHandler` implements `RequestHandler`. It checks the query against the rule set and either builds the blocked response itself or delegates to an inner `Catalog` that has a `ForwardZoneHandler` on `.`.
- *Why:* blocked-responses needs exact control of the rcode, the AA flag and the SOA in the authority section. Doing it at request level makes that a matter of message construction, not of how `Catalog` maps zone-handler errors. It also guarantees filtering happens before the forwarder's cache (dns-forwarding, Response cache).
- *Alternative:* a `ZoneHandler` chained before the forwarder in `Catalog::upsert(".", vec![filter, forwarder])`, returning `LookupControlFlow::Skip` when not blocked. Simpler wiring, but NXDOMAIN-with-SOA and the AA flag depend on catalog internals. It stays the fallback if the spike (task 1.2) shows the request-level API is impractical in the pinned release.
- *Pin:* the latest `hickory-server` release (v0.26.x at time of writing), exact version, with `Cargo.lock` committed.
- *Spike outcome (task 1.2, `hickory-server` 0.26.4):* the request-level approach works and stays. `FilterHandler` implements `RequestHandler` and wraps a `Catalog` holding a `ForwardZoneHandler` on `.`. `tests/wire.rs` runs a real server on loopback and checks, over UDP, that a blocked name gets NXDOMAIN with AA set and an SOA in the authority section and no answers, and that any other name is forwarded to a stub upstream. Findings:
  - `MessageResponseBuilder::build` takes the SOA as its own section, so the blocked response is plain message construction.
  - `ResponseInfo::serve_failed` is crate-private. The handler builds the failure value through the public `From<Header> for ResponseInfo`.
  - `Catalog` REFUSES a query without the RD flag, because the forwarder only serves recursive queries. Blocked names are answered whether or not RD is set, since the filter runs first. Whether the forwarded path should instead answer non-RD queries is a behaviour question for dns-forwarding, not a blocker.
  - The crate's minimum Rust version is 1.88, so the Alpine branch's Rust (packaging D1) must be at least that.
  - The crate also ships an optional built-in `blocklist` zone handler (feature `blocklist`). It isn't used: our rule types, list formats and atomic swap need the request-level hook anyway.

### D2. Rule set: immutable, built whole, swapped atomically
A `RuleSet` holds, separately for allow and block:
- `exact: HashSet<Box<str>>` (domain and hosts rules),
- `suffix: HashSet<Box<str>>` (wildcard base names),
- `regex: regex::RegexSet` (all regex rules compiled together).

Matching normalises the qname once (lower case, no trailing dot) and checks allow first, then block: exact → suffix walk (the name, then each parent after a `.`) → regex set. The live set sits in an `ArcSwap<RuleSet>`. A refresh builds a new `RuleSet` from every list's current content (fresh or last good) plus the inline rules, then stores it.
- *Why:* readers do one atomic load with no lock contention. Atomicity (blocklist-sources, Atomic rule updates) holds by construction. `RegexSet` checks all patterns in one linear-time pass, and the `regex` crate guarantees linear time, so no catastrophic backtracking.
- *Alternatives:* an `RwLock` around mutable maps (readers block on writers during a big rebuild); hickory `LowerName` keys (≈ 80 bytes inline each, and compared label-wise, which we don't need); FST storage (smaller, see D8).
- *Cost:* a rebuild briefly holds two rule sets in memory.

### D3. Lists keep their last good content independently
Each list has a `ListState`: the last good parsed rules (stored by type), the validators (`ETag`, `Last-Modified`), the last success time, the last attempt's result, and the skipped-line count. A refresh cycle:
1. Fetches every due list, with bounded concurrency (default 4).
2. On `200`, parses and replaces that list's state. On `304`, keeps it. On error, keeps it and records the failure.
3. Rebuilds and swaps the `RuleSet` once, if any list changed.

- *Why:* a single failing list never removes its rules (Last good copy on failure). One swap per cycle keeps rebuilds cheap.
- One refresher task with a timer per list's interval. Cycles never overlap (a `tokio` interval with `MissedTickBehavior::Skip`).

### D4. HTTP fetching: `reqwest` + rustls with a custom resolver
`reqwest` is built with rustls and no default features. A custom `reqwest::dns::Resolve` is backed by a `hickory-resolver` that points only at the bootstrap resolver when one is configured, or uses the system configuration otherwise. The client sets the timeout, a streaming size cap (count bytes, abort above the limit) and a redirect policy (https only, or http → http).
- *Why:* the bootstrap resolver must be the only path for list host names (blocklist-sources, Bootstrap resolver). Reusing hickory's resolver gives plain DNS and DNS-over-TLS without a second DNS stack.
- *Alternative:* hand-rolled hyper with a custom connector: more code, same result.

### D5. Configuration: TOML via `serde`, rules as lines
Sketch:
```toml
listen = ["0.0.0.0:1053", "[::]:1053"]
upstreams = ["tls://192.0.2.53#dns.example", "192.0.2.54:53"]
http = "0.0.0.0:8080"          # or false

[filter]
refresh = "24h"                # 0 disables
response = { mode = "null" }   # address: ipv4/ipv6; ttl = 3600
bootstrap = "tls://192.0.2.53#dns.example"
cache_dir = "/var/cache/hickory-filter"   # optional
rules = [
  "block wildcard *.ads.example",
  "allow domain ok.ads.example",
]

[[filter.lists]]
rule = "block list hosts https://lists.example/hosts.txt"
refresh = "6h"
required = false
```
`serde(deny_unknown_fields)` everywhere. Durations via `humantime`. Rule lines reuse the list-line parser, so inline and list syntax can't drift. `--check` runs parsing and validation only.
- *Why:* TOML is strict and easy to validate. Rule lines keep rules readable and copyable. *Alternative:* fully structured rules (`{ action, type, value }`): verbose for long rule lists.
- `upstreams`/`bootstrap` address syntax (settled in task 2.3): `[dns://]ip[:port]` for plain DNS (port 53) or `tls://ip[:port]#name` for DNS-over-TLS (port 853, certificate verified for `name`). IPv6 with a port is `[::1]:53`. `tls://` requires `#name`, `#name` is rejected on `dns://`, and host names are rejected for both, which is what keeps list fetches from resolving through ourselves. Documented in README.md.
- Rule lines (`filter.rules`, `filter.lists[].rule`) are kept as text by the config types. The shared line parser (section 3) validates them, and `--check` runs it once it exists.

### D6. Parsing: per-type line parsers, skip-and-count
One parser per list type returns `Option<Rule>` per line, and the caller counts the `None`s. Comment stripping happens first (`#` anywhere; `!` at the start of a wildcard-list line). Wildcard spellings normalise to a base name and must pass a DNS-name check. `hosts` splits on whitespace, requires exactly two fields with the first a valid IP, and keeps only the name. Regex lines compile individually during parsing, so a bad line is skipped and counted, never poisoning the whole `RegexSet`.

### D7. Observability: `metrics` + Prometheus exporter on a small HTTP server
The `metrics` facade with `metrics-exporter-prometheus`. `/health`, `/ready` and `/metrics` are served by a minimal `hyper` service on its own listener. Readiness is an `AtomicBool`, set after the first rule set is stored and the listeners are bound. Logging uses `tracing` + `tracing-subscriber` (text or JSON, `RUST_LOG`-style level). List URLs are passed through a credential-stripping display helper before being logged or used as labels.

### D8. Memory: hash sets now, FST later behind the same interface
`RuleSet` matching sits behind a small internal trait (`contains_exact`, `matches_suffix`). A later change can swap in reversed-domain FSTs without touching parsing, refresh or the handler. Benchmark first: build a 1M-name list, then measure RSS steady and during a swap.

## Risks / Trade-offs

- [`hickory-server` API changes between releases] → Pin exactly. Keep all hickory glue in one module. Upgrades are separate changes with the wire tests as the safety net.
- [Request-level handler is impractical in the pinned release] → Spike first (task 1.2). The fallback is the `ZoneHandler` chain (D1 alternative), accepting whatever NXDOMAIN shape `Catalog` produces, documented.
- [A rebuild doubles rule memory briefly] → Document it for sizing. Bounded fetch concurrency. FST later (D8).
- [Huge regex lists make `RegexSet` slow to compile or large] → Count regex rules in metrics, document that regex lists are the expensive kind, and consider a per-list cap if needed.
- [List host resolution loops back to this server] → The bootstrap resolver is IP-only, and the system resolver is used only when none is set, with a documented warning that the system resolver must not point at hickory-filter itself.
- [A malicious or broken list server (huge or endless body, redirect tricks)] → Timeout, size cap, redirect policy (D4). Content is parsed as untrusted text, and invalid lines are skipped.
- [Stale cache served forever if a list disappears] → Last success time per list in metrics, so alerting can catch it. No automatic expiry in this change.

## Migration Plan

None: a new project. Rollout is the first tagged release.

## Open Questions

- Exact spelling of upstream/bootstrap addresses in TOML (D5). It doesn't affect behaviour, and is decided in task 2.3.
- Default bounded fetch concurrency (4) and size cap (64 MiB) may be tuned after benchmarking. The behaviour is the same.
