# Tasks

## 1. Dependencies and configuration

- [x] 1.1 Enable the `recursor` and `dnssec-aws-lc-rs` features of `hickory-server` (the exact pin stays) and regenerate `Cargo.lock`; verify `cargo build --locked`, `cargo deny check licenses` and the notices generation pass, and record the binary size before and after
- [x] 1.2 Add `mode` (`forward` default, `recursive`) and the `[recursion]` table (`roots`, `dnssec`, `qname_minimization`, `opportunistic_encryption`, `state_file`, `state_save_interval`) to the configuration with `deny_unknown_fields`, and validate that `upstreams` is rejected in recursive mode and `[recursion]` in forward mode, each error naming the key; verify unit tests for every mode scenario and `--check` on each configuration
- [x] 1.3 Document the new keys in `examples/hickory-filter.toml` and the README configuration reference; verify the README drift tests pass and that the example runs through `--check`

## 2. Recursion core

- [x] 2.1 Build the hermetic harness for recursion tests: an unshared user and network namespace containing the server and fake root, TLD and authoritative servers on port 53, at addresses outside the ranges the recursor refuses (add them to the namespace's loopback interface, falling back to a documented test-only relaxation of the server filter only if the namespace can't); verify a test resolves a name through root, TLD and authoritative servers with the harness alone
- [x] 2.2 Read and validate the root hints file (`recursion.roots`, default `/usr/share/hickory-filter/root.hints`) and build the catalog in recursive mode with the library's recursive zone handler, mapping `cache_size` to its response cache and where `0` disables the cache as in forward mode (the library's response cache accepts it); verify unit tests for a missing, empty and valid hints file and for the cache size cases, and a harness test (2.3) that `0` sends every query on
- [x] 2.3 Resolve queries that aren't blocked from the roots; verify the harness test that a name is answered through a delegation chain and only the root and the authoritative server were contacted, that a cached name sends nothing, and that a blocked name sends no query to any server
- [x] 2.4 Return SERVFAIL when no name server answers, never a blocked-style answer and never a forwarded one; verify a harness test with unreachable name servers, and that the failure is counted in the existing upstream response metrics
- [x] 2.5 Confirm that delegations to private, loopback and other refused addresses are never contacted; verify a harness test with a delegation to `10.0.0.1` that sends nothing there and returns SERVFAIL

## 3. Readiness and metrics

- [x] 3.1 Resolve one query through the recursive handler at startup with retries and backoff, and keep `/ready` at 503 until it succeeds, without letting later failures flip it back; verify harness tests with unreachable roots (503), answering roots (200) and a later failure (still 200)
- [x] 3.2 Confirm the library's outgoing query, name server response and cache series are exposed in recursive mode with the existing names and labels, and extend the README metric descriptions to say what the upstream series mean there; verify a metrics test in recursive mode and the README drift tests

## 4. Privacy defaults

- [x] 4.1 Map `qname_minimization` (default strict); verify a harness test that the root server never receives the full name and the TLD server at most the second-level name, and a config test that `case_randomization` is rejected as an unknown key (0x20 case randomization is a future improvement, see the design)
- [x] 4.2 Map `opportunistic_encryption` (default on) and add a fake name server that accepts TLS with a self-signed certificate to the harness; verify tests that queries to it move to TLS and the `tls` count rises, that a plain-only server is still answered, and that `false` attempts no TLS connection
- [x] 4.3 Implement `recursion.state_file` and `recursion.state_save_interval` (persist the learned TLS support across restarts, saved on that interval; no file written without it); verify a harness test that a restart reuses the state without probing, and the restricted-container sandbox test in recursive mode without a state file writes no file

## 5. DNSSEC

- [ ] 5.1 Spike how to produce signed fake zones for the harness with what the library already provides, and write down the finding next to the tests; verify a test that a signed fake zone, with its key as the trust anchor, validates
- [ ] 5.2 Map `recursion.dnssec` (`validate` default with the built-in anchor, `off`, or a trust anchor file path); verify harness tests for a valid chain, a tampered answer (SERVFAIL), an unsigned zone (answered), validation off (answered) and a custom anchor file
- [ ] 5.3 Check whether the library distinguishes validation failures from other failures; if it does, count them in a documented metric, otherwise record in the design that they can't be told apart; verify the decision is written in the design and, if a metric was added, tested and in the README table

## 6. Packaging, documentation and checks

- [ ] 6.1 Add the root hints file to the repository, record its source and date, and install it at `/usr/share/hickory-filter/root.hints` through the melange source directory; verify the image smoke test finds the file and that the image starts in recursive mode in the restricted container with `/health` 200
- [ ] 6.2 Extend the chart's values schema with `mode` and `recursion`, keep forward as the default, and make sure an enabled NetworkPolicy renders no egress rules; verify new golden renders for recursive values and for the NetworkPolicy in recursive mode, and `hack/chart-test.sh` passes
- [ ] 6.3 Write the README section for recursive mode: what it is for, the defaults, the egress it needs (UDP and TCP 53, TCP 853), that opportunistic encryption is unauthenticated and covers passive observers and TLS-accepting servers only, the trust anchor caveat, that 0x20 case randomization isn't supported yet and is planned for when `hickory-server` applies it, that the listener is as open as in forward mode, and private egress as guidance for network privacy; verify a test that the README states the unauthenticated, passive-only and TLS-accepting-servers-only limits, and the policy check passes
- [ ] 6.4 Check the dashboard against a running recursive-mode server (the live Prometheus, Loki and Grafana check used for the dashboard change) and decide whether any panel titles need to change; verify the panel queries return data and record the result in the PR
- [ ] 6.5 Run recursive mode once against the real root servers where the network allows: resolve a handful of names, confirm validation and encryption behave, and record memory use and latency; verify and record the numbers in the PR, or record the reason it couldn't be run

## Workflow follow-up

- Release through the release PR; verify the published image contains the hints and the chart accepts the new values.
- Archive after the first release containing it verifies, and check the main specs, including `recursive-resolution`.
- Update the status text in AGENTS.md when the change is archived.
