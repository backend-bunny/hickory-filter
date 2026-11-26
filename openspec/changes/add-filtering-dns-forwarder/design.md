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
- *Blocked response details (tasks 5.1, 5.2):* `src/blocked.rs` builds the answer as plain data from the mode (testable without a server), and `FilterHandler` sends it with the query's ID, question and RD flag echoed and the recursion-available flag set, as a recursive resolver would. Only `nxdomain` mode sets AA and carries an SOA: owned by the queried name, with the configured TTL as both record TTL and negative-caching minimum, `hostmaster.invalid.` as the mailbox and the queried name as the primary server. Other modes answer with no authority section, as the spec describes. Only standard queries (opcode QUERY, message type query) are filtered; anything else goes to the catalog.

### D2. Rule set: immutable, built whole, swapped atomically
A `RuleSet` holds, separately for allow and block:
- `exact`: an `fst::Set` of names (domain and hosts rules), stored as reversed-label keys (D8),
- `suffix`: an `fst::Set` of wildcard base names, same key form,
- `regex: regex::RegexSet` (all regex rules compiled together).

Matching normalises the qname once (lower case, no trailing dot) and checks allow first, then block: exact → suffix → regex set. Exact and suffix lookups are one pass over the automaton that reads the query's labels from the TLD down, checking at each label boundary whether a wildcard base ends there. The live set sits in an `ArcSwap<RuleSet>`. A refresh builds a new `RuleSet` from every list's current content (fresh or last good) plus the inline rules, then stores it.
- *Why:* readers do one atomic load with no lock contention. Atomicity (blocklist-sources, Atomic rule updates) holds by construction. `RegexSet` checks all patterns in one linear-time pass, and the `regex` crate guarantees linear time, so no catastrophic backtracking.
- *Alternatives:* an `RwLock` around mutable maps (readers block on writers during a big rebuild); hickory `LowerName` keys (≈ 80 bytes inline each, and compared label-wise, which we don't need); hash sets (simpler and faster per lookup, but measured at about 85 bytes per name and a 2x refresh peak, see D8).
- *Cost:* a rebuild briefly holds two rule sets in memory.

### D3. Lists keep their last good content independently
Each list has a `ListState`: the last good parsed rules (stored by type), the validators (`ETag`, `Last-Modified`), the last success time, the last attempt's result, and the skipped-line count. A refresh cycle:
1. Fetches every due list, with bounded concurrency (default 4).
2. On `200`, parses and replaces that list's state. On `304`, keeps it. On error, keeps it and records the failure.
3. Rebuilds and swaps the `RuleSet` once, if any list changed.

- *Why:* a single failing list never removes its rules (Last good copy on failure). One swap per cycle keeps rebuilds cheap.
- One refresher task with a timer per list's interval. Cycles never overlap (a `tokio` interval with `MissedTickBehavior::Skip`).
- *On-disk cache (task 6.5, `src/cache.rs`):* `CachedLoader` wraps any `ListLoader` and is used only when `filter.cache_dir` is set, so without it nothing is written (wiring checks this). Each remote list is one file named by a stable hash of its URL (`<16 hex digits>.cache`), holding a small header (format, URL, validators) and then the body, written to a temporary file in the same directory and renamed into place, so a reader never sees a half-written copy and a crash leaves the previous one. The file records its URL, so a hash collision or a corrupt or foreign file is simply not a cached copy. The first load of each list in a process sends the cached validators: a `304` or a failed fetch then serves the cached copy as the list's content (it counts as loaded, so a `required` list passes and the server becomes ready), and a `200` replaces both the rules and the cache. Later failures never read the cache: the in-memory last good rules are already what is served. A cache that can't be written (directory missing, not writable) never fails a load; the error is kept in `last_write_error`. `file://` lists are never cached. There is no expiry in this change (see the risks).
- *Implemented (task 6.4, `src/refresh.rs`):* one task owns every `ListState`. Each list has its own due time (its own interval or the global one; `0` leaves it unscheduled). The task sleeps until the earliest due time, loads the lists that are due with bounded concurrency (a semaphore, default 4), applies the results, and, only if some list's rules actually changed (content equal to the last good copy, `304`s and failures change nothing), rebuilds the rule set once and swaps it. Cycles run one after another so they never overlap, and the next cycle for a list is due one interval after the previous one finished, so a slow cycle is not followed by a burst of catch-up cycles (the effect of `MissedTickBehavior::Skip`, without a shared tick). The initial load swaps exactly once, then fails startup for a `required` list that never loaded. Parsing and the rule-set build run on the blocking pool, and each list's rules are shared (`Arc`) so a rebuild takes a cheap snapshot instead of copying every rule. A build failure keeps the previous set and is recorded. Where lists come from is a `ListLoader` trait (`DefaultLoader` reads files and fetches over HTTP(S)), so the scheduling is tested on a paused clock with no network. `LiveRules::generation()` counts swaps. The refresher stops on a shutdown future, even in the middle of a load.

### D4. HTTP fetching: `reqwest` + rustls with a custom resolver
`reqwest` is built with rustls and no default features. A custom `reqwest::dns::Resolve` is backed by a `hickory-resolver` that points only at the bootstrap resolver when one is configured, or uses the system configuration otherwise. The client sets the timeout, a streaming size cap (count bytes, abort above the limit) and a redirect policy (https only, or http → http).
- *Why:* the bootstrap resolver must be the only path for list host names (blocklist-sources, Bootstrap resolver). Reusing hickory's resolver gives plain DNS and DNS-over-TLS without a second DNS stack.
- *Alternative:* hand-rolled hyper with a custom connector: more code, same result.
- *Server lifecycle (tasks 7.2, 7.3, `src/server.rs`, `main`):* `start` binds every `listen` address first (so a bad address fails before any list is fetched), then loads every list (the initial load, which fails startup for a `required` list), builds the forwarder and the `FilterHandler`, registers the sockets and starts the refresher. Each address gets a TCP listener and a UDP socket on the same port (a configured port 0 is resolved, once, for both). Sockets are made with `socket2` so IPv6 ones are v6-only, which lets the default `0.0.0.0:1053` and `[::]:1053` coexist; TCP sets `SO_REUSEADDR` so a restarted server rebinds at once. A bind failure names the address and the protocol. The one tolerated failure is an IPv6 address on a host without IPv6 (`EAFNOSUPPORT`, `EADDRNOTAVAIL`): it is skipped with a note, so the default of listening on all addresses also works in IPv4-only containers, as long as one address binds. The list loader is `ConfiguredLoader`: the caching wrapper only when `filter.cache_dir` is set, so without it nothing is written. `main` runs `start`, prints the bound addresses to stderr, and waits for SIGTERM or SIGINT; `Running::shutdown` stops accepting, gives queries in flight up to 5 seconds to finish (idle TCP connections are closed after 5 seconds), stops the refresher, and the process exits 0 either way. A config error or a startup failure exits 1 with the reason on stderr.
- *Forwarding (task 7.1, `src/forwarder.rs`):* `build_catalog` makes a `Catalog` with one `ForwardZoneHandler` on the root whose name servers are the configured `upstreams` (the same plain-DNS and DNS-over-TLS mapping as the bootstrap resolver; the TLS server name is verified). It never reads the hosts file, caches forwarded answers for at most their TTL with `cache_size` entries (top-level config key, default 4096, `0` disables), and gives up on a dead upstream after 2 attempts of 2 seconds, so the client gets SERVFAIL in about four seconds, before a typical stub resolver times out. Filtering runs in `FilterHandler` in front of the catalog, so a rule change applies to the next query even for a cached name. Checked against a real DNS-over-TLS resolver: the right name resolves and a wrong name gives SERVFAIL.
- *Implemented (task 6.2, `src/fetch.rs`):* `reqwest` 0.13 with default features off and `rustls`, which brings the aws-lc-rs crypto provider and `rustls-platform-verifier`, so certificates are checked against the system CA bundle that the image ships; there is no OpenSSL or native-tls anywhere in the tree. A real HTTPS fetch works and an expired certificate is rejected. The timeout is 30 seconds and the size cap 64 MiB (fixed defaults for now; the spec doesn't make them configurable), checked from `Content-Length` and again while streaming. The redirect policy is `redirect_allowed`: only to `https`, or to `http` when the original was `http`, at most 10 hops. Environment proxies are deliberately not used: through a proxy the proxy would resolve the list host, bypassing the bootstrap resolver guarantee. `ETag` and `Last-Modified` are sent back as `If-None-Match` and `If-Modified-Since`; a `304` keeps the rules, and a `304` with no previous copy counts as a failure.
- *Bootstrap resolver (task 6.3, `src/bootstrap.rs`):* `BootstrapResolver` is the client's DNS resolver, backed by a `hickory-resolver` (pinned to the same exact version as `hickory-server`, with the `tokio`, `tls-aws-lc-rs` and `rustls-platform-verifier` features) whose only name server is the configured bootstrap: plain DNS over UDP and TCP on its port, or DNS-over-TLS on its port verified against the configured server name. It never reads the hosts file or the system configuration, and does not fall back to anything else: if the bootstrap resolver doesn't answer, the fetch fails and the list keeps its last good copy. Without a bootstrap resolver the client uses the system resolver, and `Fetcher::from_config` picks between the two. hickory's DoT client sends no SNI, but it verifies the certificate against the configured name: checked once against a real DoT resolver (a fetch through it succeeds with the right name and is rejected with `certificate not valid for name` with a wrong one), since a loopback test would need a trusted test certificate.
- *Packaging risk:* aws-lc-rs builds C code (`aws-lc-sys`), so the static musl build in the melange environment needs a C toolchain there (`build-base`, already planned) and must be verified in packaging task 2.1; the fallback is `rustls-no-provider` with the `ring` provider.

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
One parser per list type returns `Option<Rule>` per line, and the caller counts the `None`s. Comment stripping happens first (`#` anywhere; `!` at the start of a wildcard-list line). Wildcard spellings normalise to a base name and must pass a DNS-name check. `hosts` splits on whitespace, requires exactly two fields with the first a valid IP, and keeps only the name. Regex lines compile individually during parsing, so a bad line is skipped and counted, never poisoning the whole `RegexSet`. `#` starts a comment anywhere, except in regex lists, where it counts only at the start of a line or after whitespace, so a pattern containing `#` isn't cut into a different valid pattern. Names that are IP address literals are not valid names, so a hosts line like `0.0.0.0 0.0.0.0` is skipped. Inline `rules` must not declare lists (they go in `[[filter.lists]]`), and every `filter.lists[].rule` must be a list declaration; both are validated at load time, so `--check` reports a bad rule with the rule named.

### D7. Observability: `metrics` + Prometheus exporter, `tracing` logs, a query log for per-client and per-name views
The `metrics` facade with `metrics-exporter-prometheus`. `/health`, `/ready` and `/metrics` are served by a minimal `hyper` service on its own listener. Readiness is an `AtomicBool`, set after the first rule set is stored and the listeners are bound. Logging uses `tracing` + `tracing-subscriber` (text or JSON, one of five levels from the configuration). List URLs are passed through a credential-stripping display helper before being logged or used as labels.
- *HTTP endpoint (task 8.1, `src/http.rs`):* a small `hyper` 1.x HTTP/1 server (already in the dependency tree through the HTTP client) on its own listener, serving `GET` and `HEAD` for `/health` (always 200 while the process runs), `/ready` (503 until the DNS listeners are bound and the initial list load is done, 200 after, and 503 again once shutdown begins) and `/metrics` (the recorder's Prometheus text, an empty but valid exposition until the metrics of task 8.2 exist); other paths are 404 and other methods 405. It starts before the lists are loaded, so a slow initial load doesn't fail liveness probes and `/ready` honestly says 503; if startup then fails the endpoint is stopped and its port freed. A bind failure names the address and `http`. Open connections get two seconds to finish on shutdown. Because the per-list metrics will be recorded through the `metrics` facade by the refresher itself, the endpoint needs no access to the refresher's state.
- *Logging (task 8.3, `src/logging.rs`):* the `[log]` table sets `level` (`error`, `warn`, `info`, `debug`, `trace`; default `info`), `format` (`text` or `json`), `query_log` and `query_log_client`, and there is no environment override, so the configuration file is the one source. The subscriber filters by target: this crate's lines follow the level and other crates' lines are limited to warnings, so `debug` doesn't bury the output in library noise. When the query log is on, its target is allowed at `info` even if the level is quieter, so asking for query lines always works. The JSON formatter flattens fields to the top level. Operational messages (listening, the endpoint, signals, list loads and refreshes, rule set replacements) go through `tracing`; a fatal startup error is still printed plainly as `error: ...` because it must show whatever the level. A list load logs `list`, `action`, `list_type`, `result` (`loaded`, `unchanged`, `failed`), `rules` and `skipped`, as a warning with `reason` when it failed. List URLs in log lines, metric labels and failure reasons have credentials removed (`display_url`), and HTTP client errors are stripped of their URL, so a credentialed URL never reaches stderr.
- *Metric names* (the contract the dashboard in `add-grafana-dashboard` is built on; the recorder is installed once per process and counts everything through the `metrics` facade, so tests compare scrapes before and after, and the first rule set built at startup counts as the first replacement):
  - `hickory_filter_queries_total{outcome,type}` and `hickory_filter_query_duration_seconds{outcome}` (a histogram), where `type` is one of a fixed set of common record types (A, AAAA, CNAME, HTTPS, MX, NS, PTR, SOA, SRV, SVCB, TXT) or `other`;
  - upstream and cache metrics come from the resolver library through the same facade (its `metrics` feature), under its own names: `hickory_resolver_outgoing_queries_total{protocol}`, `hickory_resolver_name_server_query_responses_total{protocol,status}` (`success`, `timeout` and failures) and `hickory_resolver_cache_hit_total` and `hickory_resolver_cache_miss_total`. They are aggregates across the upstreams, not per upstream: the library's per-name-server tracking is off by default and the forwarder doesn't expose it, so a dead upstream among several shows as timeouts, not by address;
  - `hickory_filter_rules{action,kind}`, `hickory_filter_ruleset_swaps_total` and `hickory_filter_ruleset_last_swap_timestamp_seconds`;
  - per list, labelled `action`, `type` and `url`: `hickory_filter_list_rules`, `hickory_filter_list_skipped_lines`, `hickory_filter_list_last_success_timestamp_seconds` and `hickory_filter_list_last_refresh_success` (1 or 0).
- *No identifying labels.* Client addresses and queried names never become metric labels: they would make the series count unbounded and are personal data. Per-client and per-name views come from the query log instead, which an operator enables on purpose, and which a log collector (Loki, for example) turns into top lists.
- *Query log.* One `tracing` event per answered query with the fixed target `hickory_filter::query` and the fields `client`, `name`, `type`, `outcome`, `rcode` and `duration_ms`. It is emitted at `info` when the query log is enabled and at `debug` otherwise, so the default is no per-query lines. The JSON formatter flattens event fields to the top level of the line's object. Configuration, in a `[log]` table: `level` (default `info`), `format` (`text` or `json`, default `text`), `query_log` (default `false`) and `query_log_client` (`address`, the default when the query log is on, `masked` or `omit`). Masking zeroes the last octet of an IPv4 address and keeps the first 48 bits of an IPv6 address.
- *Alternatives:* top-N client and name gauges in Prometheus (series churn as ranks change, and the data is still personal); an in-memory top-K sketch behind a JSON endpoint (more server code, and a dashboard would need a plugin to read it); a query database (a writable volume, retention, a much larger design).

### D8. Memory: reversed-domain FSTs behind `NameIndex`
Decision (maintainer, 2026-11-26): exact and wildcard names are stored in finite-state transducers (the `fst` crate), to shrink the footprint. Measured baseline with hash sets (task 4.3), for 1M exact names: 83 MiB resident (about 85 bytes per name), 176 MiB peak while a refresh holds old and new sets, and 142 MiB after the swap because millions of small freed allocations aren't returned to the OS.
- *Keys:* a name's labels in reverse order, joined with `.` (`ads.example` becomes `example.ads`), so names under one suffix share a prefix. Keys are sorted, deduplicated and built into one `fst::Set` for exact names and one for wildcard bases, per side (allow, block).
- *Lookup:* one walk over the automaton following the query's labels from the TLD down, with no allocation. At each label boundary, a final state in the wildcard set means a base ends there; at the end of the name, a final state in the exact set means an exact match. This replaces the label-by-label hash lookups with a single pass.
- *Build:* collect the keys, sort, dedupe, build. The sort buffer is a transient cost during a refresh. The replacement is still built off the query path and swapped whole (D2), and the finished automaton is one contiguous buffer, so freeing the old one returns its memory.
- *Interface:* `RuleSet` matching stays behind `NameIndex` (`contains_exact`, `matches_suffix`) in `src/ruleset.rs`, so parsing, refresh and the handler don't change. The hash-set implementation stays as a test oracle: a test checks that both give the same answers over generated and edge-case names. Queries are normalised leniently there, lower case and one trailing dot, without the rule parser's name validation, so unusual query names still match by case and never panic.
- *Trade-offs accepted:* each lookup is slower than a hash lookup (expected a few hundred nanoseconds against about 61 ns, still far below a DNS round trip), a refresh pays a sort, `fst` is one more dependency (Unlicense OR MIT, which the allowlist covers via MIT), and the automaton is immutable, which we already rely on.
- *Measured result (task 4.6, 1M names, generated to look like a merged blocklist):* steady memory 70 MiB with hash sets against 21 MiB with the FST; peak during a refresh 153 MiB against 83 MiB; after the swap 119 MiB against 23 MiB; lookups 2 to 4 times slower (exact hit 42 ns against 175 ns). The automaton itself is 7.9 MiB: the rest of the resident figure was the build's transient allocations, so the keys are packed into one contiguous buffer (not one `String` per name), which took steady memory from 50 MiB to 21 MiB. Generated names still differ from real lists, so the figures should be confirmed on a real merged list and on the musl release build.

## Risks / Trade-offs

- [`hickory-server` API changes between releases] → Pin exactly. Keep all hickory glue in one module. Upgrades are separate changes with the wire tests as the safety net.
- [Request-level handler is impractical in the pinned release] → Spike first (task 1.2). The fallback is the `ZoneHandler` chain (D1 alternative), accepting whatever NXDOMAIN shape `Catalog` produces, documented.
- [A rebuild doubles rule memory briefly] → Document it for sizing. Bounded fetch concurrency. FST storage (D8) lowers both the steady and the peak figure.
- [Huge regex lists make `RegexSet` slow to compile or large] → Count regex rules in metrics, document that regex lists are the expensive kind, and consider a per-list cap if needed.
- [List host resolution loops back to this server] → The bootstrap resolver is IP-only, and the system resolver is used only when none is set, with a documented warning that the system resolver must not point at hickory-filter itself.
- [A malicious or broken list server (huge or endless body, redirect tricks)] → Timeout, size cap, redirect policy (D4). Content is parsed as untrusted text, and invalid lines are skipped.
- [Stale cache served forever if a list disappears] → Last success time per list in metrics, so alerting can catch it. No automatic expiry in this change.

## Migration Plan

None: a new project. Rollout is the first tagged release.

## Open Questions

- Exact spelling of upstream/bootstrap addresses in TOML (D5). It doesn't affect behaviour, and is decided in task 2.3.
- Default bounded fetch concurrency (4) and size cap (64 MiB) may be tuned after benchmarking. The behaviour is the same.
