# Tasks

## 1. Project setup and API spike

- [x] 1.1 Create the Cargo project (`hickory-filter` binary), `rust-toolchain.toml` with a pinned stable toolchain, `LICENSE-MIT` and `LICENSE-APACHE` (`license = "MIT OR Apache-2.0"` in `Cargo.toml`), and CI-equivalent local checks; verify `cargo fmt --check`, `cargo clippy -- -D warnings` and `cargo test` pass on the empty skeleton
- [x] 1.2 Spike D1 against the latest `hickory-server` release, pinned exactly: a `RequestHandler` that answers one hard-coded name with NXDOMAIN + SOA in the authority section and AA set, and delegates everything else to a `Catalog` with a `ForwardZoneHandler`; verify with a wire-level test (hickory client against the running server) and record the outcome (request-level, or the ZoneHandler fallback) in design.md D1

## 2. Configuration

- [x] 2.1 Define the TOML config types with `deny_unknown_fields` (listen, upstreams, http, filter: refresh, response, bootstrap, cache_dir, rules, lists) and load from `--config` with a documented default path; verify unit tests reject an unknown key, an unreadable file and an invalid duration with errors naming file/key
- [x] 2.2 Implement `--check` (parse + validate, no listeners, no fetches); verify an integration test where `--check` on a valid config exits 0 and opens no sockets
- [x] 2.3 Settle and implement the upstream/bootstrap address syntax (plain DNS and DNS-over-TLS with server name; IP-only for bootstrap); verify unit tests for valid forms and rejection of host names, then document the syntax in README.md and design.md D5
- [x] 2.4 Write an annotated example config `examples/hickory-filter.toml`; verify it passes `--check` in a test

## 3. Rule parsing

- [x] 3.1 Implement name normalisation and the inline rule-line parser (`<action> <type> <value>`, `<action> list <type> <url>`); verify unit tests for each spec scenario in filter-rules "Rule actions and types" and "Name normalisation"
- [x] 3.2 Implement the wildcard parser for `x`, `*.x`, `.x`, `||x^`, `address=/x/<any>`, skipping `$option` lines and rejecting invalid names; verify table-driven tests covering every spelling and the filter-rules wildcard scenarios
- [x] 3.3 Implement the domain, hosts (any IPv4/IPv6, any whitespace) and regex line parsers with comment stripping and per-line skip counting; verify tests for the filter-rules "List line syntax" and "Unparseable list lines" scenarios

## 4. Rule set and matching

- [x] 4.1 Implement `RuleSet` (exact, suffix and `RegexSet` for allow and block) with matching in the order allow → block, exact → suffix → regex; verify tests for every filter-rules matching scenario, including allow inside a blocked wildcard and allow-list over `block regex .*`
- [x] 4.2 Put the live `RuleSet` in an `ArcSwap` and build it from inline rules plus each list's current content; verify a concurrency test where readers running during repeated swaps only ever see a complete old or new set
- [x] 4.3 Add a micro-benchmark (criterion) for lookups against a 1M-name generated set, and an RSS measurement for steady state and during a swap; verify the benchmark runs and record the numbers in README.md under Memory
- [ ] 4.4 Implement the FST-backed name index per D8: reversed-label keys, sorted and deduplicated, one `fst::Set` for exact names and one for wildcard bases, and a single allocation-free automaton walk per lookup; verify unit tests for exact, wildcard base and subdomains, label boundaries (`badads.example`), case, trailing dot and single-label names, plus a test comparing it with the hash-set implementation over generated and edge-case names
- [ ] 4.5 Use it in `RuleSet` for both sides, keeping the hash-set version only as a test oracle behind `NameIndex`; verify every existing ruleset test and the concurrency test pass unchanged and `cargo deny check licenses` accepts the `fst` dependency
- [ ] 4.6 Move the benchmark and memory example to a more varied name distribution (several TLDs, varied label lengths and depths, many subdomains per registrable domain) and re-measure steady, peak-during-swap and after-swap memory and lookup times against the hash-set baseline; verify the README Memory section shows both and states the data used

## 5. Blocked responses

- [ ] 5.1 Implement the `null`, `address`, `nodata` and `nxdomain` response builders with a configurable TTL, echoing ID, question and RD; verify unit tests at the message level for every blocked-responses scenario (including MX in null mode, a missing address family, and SOA in the authority section)
- [ ] 5.2 Wire the builders into `FilterHandler` per D1 (or the fallback recorded in 1.2); verify a wire-level test that a blocked query gets the expected answer and the upstream receives nothing

## 6. List sources and refresh

- [ ] 6.1 Implement `file://` loading and `ListState` (last good rules, skipped count, last success, last result); verify tests for an initial load, a required list missing (startup fails) and an optional list missing (starts, failure recorded)
- [ ] 6.2 Implement HTTP(S) fetching with reqwest + rustls: timeout, streamed size cap, redirect policy, `ETag`/`If-Modified-Since`; verify tests against a local test HTTP server for 200, 304, 503, timeout, an oversized body and a redirect to http from https (rejected)
- [ ] 6.3 Implement the bootstrap resolver as reqwest's DNS resolver backed by `hickory-resolver` (plain DNS and DoT), falling back to the system resolver; verify a test where list host resolution goes only to a local stub resolver
- [ ] 6.4 Implement the refresher: per-list intervals, `0` disables, no overlapping cycles, bounded concurrency, one swap per cycle; verify tests with a paused tokio clock for the blocklist-sources "Scheduled refresh" and "Last good copy on failure" scenarios
- [ ] 6.5 Implement the optional on-disk cache (content + validators per list, atomic file writes, nothing written without `cache_dir`); verify a test where a restart with the list server down serves the cached rules and becomes ready

## 7. Forwarding and server lifecycle

- [ ] 7.1 Configure the `ForwardZoneHandler` from `upstreams` with a bounded response cache, returning SERVFAIL when no upstream answers; verify wire-level tests for a forwarded answer, SERVFAIL with unreachable upstreams, and a newly blocked name that was cached
- [ ] 7.2 Bind UDP and TCP listeners (default port 1053), failing startup with the address on a bind error; verify tests for the dns-forwarding "Listeners" scenarios
- [ ] 7.3 Implement graceful shutdown on SIGTERM/SIGINT with a grace period; verify a test that the process exits 0 within the grace period

## 8. Observability

- [ ] 8.1 Serve `/health`, `/ready` and `/metrics` on the HTTP address (disable-able), with readiness set after the first rule set and the listener binding; verify tests for each observability health and readiness scenario
- [ ] 8.2 Emit query outcome counters and latency, plus per-list gauges (rules, skipped, last success, last result) labelled with credential-stripped URLs; verify a test scraping `/metrics` after blocked, allowed, passed and failed-refresh events
- [ ] 8.3 Set up `tracing` logging (level, text or JSON), list load/refresh log lines, and per-query logs only at debug or with query logging on; verify a test capturing logs for a refresh failure and showing no per-query lines at info

## 9. Integration

- [ ] 9.1 End-to-end test: run the binary with the example config against a local stub upstream and a local list server, exercising one scenario from each capability over the wire; verify it passes in `cargo test`
- [ ] 9.2 Run the binary as an unprivileged user, with no capabilities, on a read-only root filesystem (e.g. `unshare`/`bwrap`, no container build in this repo); verify it serves queries and writes nothing outside `cache_dir`
- [ ] 9.3 Update README.md from "planned" to the implemented behaviour, with config reference and examples; verify every config key in the README exists in the example config and passes `--check`

## Workflow follow-up

- Decided: license `MIT OR Apache-2.0` (task 1.1 adds both license files), config format TOML with rules as lines, repo hosting GitHub (`backend-bunny/hickory-filter`).
- Archive the change after review, and verify the archived specs in `openspec/specs/`.
