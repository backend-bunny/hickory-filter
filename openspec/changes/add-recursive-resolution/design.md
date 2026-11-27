# Design

## Context

See proposal.md for the motivation. Today `build_catalog` (`src/forwarder.rs`) builds a `Catalog` with one `ForwardZoneHandler` on the root, fed by `upstreams`, and `FilterHandler` sits in front of the catalog. `/ready` is an atomic flag set once the listeners are bound and the lists are loaded.

What the library offers, observed in `hickory-server` 0.26.4 and `hickory-resolver` 0.26.4:

- `hickory-server` has a public `RecursiveZoneHandler` behind its `recursor` feature (which enables `hickory-resolver/recursor`). It is a zone handler like `ForwardZoneHandler` and plugs into the same `Catalog`. Its only constructor, `try_from_config`, takes a `RecursiveConfig` whose `roots` is a **file** of root hints that is read from disk.
- `RecursorOptions` has `qname_minimization` (default strict), `case_randomization` (present, but not applied by the recursor's query path; see Future improvements), `opportunistic_encryption` (default disabled; periods of 3 days for success and 1 day for failure, a cap on concurrent probes and an optional state file), `deny_server` (defaults to loopback, private, link-local, documentation, multicast and other non-routable ranges), recursion limits and cache sizes.
- The DNSSEC policy is security-unaware by default. Validation needs the `dnssec-aws-lc-rs` feature and uses a static trust anchor (the library's built-in one, or a file); automatic trust anchor rollover is not implemented there.
- Roots and glue addresses are IP-only, so name servers are contacted on port 53. The library's metrics cover outgoing queries by protocol, name server responses by outcome and cache hits, misses and size.
- With opportunistic encryption on, the recursor disables TLS peer verification for the probes.

The image is distroless with a read-only root filesystem, and the chart mounts the configuration ConfigMap over `/etc/hickory-filter`.

## Goals / Non-Goals

**Goals:**
- A recursive mode that keeps every filtering behaviour and the existing observability, and that fails visibly, never by sending a query somewhere else.
- Defaults that make recursive mode safe to use: validation and name minimization on, opportunistic encryption on.
- Hermetic tests of the resolution chain, failure, DNSSEC and encryption that need no internet.

**Non-Goals:**
- Forwarding for named zones next to recursion (an internal domain, for example), or any mixing of the modes.
- Access control for who may query the server. The listener is as open as in forward mode today; recursion makes an open listener a bigger amplification target, so this is documented, not solved.
- Authenticated encryption to name servers, oblivious DNS, proxying or built-in VPN or Tor egress. Network-level privacy beyond opportunistic encryption is documentation: run the pods behind private egress.
- Resolving list host names through the recursor. `filter.bootstrap` keeps its meaning: list hosts are resolved through it, or through the system resolver when it is unset, so those host names are visible to that resolver.
- Automatic root trust anchor rollover, and serving authoritative zones.

## Decisions

Status in brackets: **[confirmed]** by the user, **[proposed]** for review.

### D1. Use the library's recursive zone handler, not our own wrapper **[proposed]**
Build the catalog with `RecursiveZoneHandler::try_from_config` and a `RecursiveConfig` assembled from our configuration.
- *Why:* it is public API and the same shape as the forwarder, so the filter, listeners and metrics stay as they are. It carries the library's DNSSEC negative-answer handling, which a hand-written zone handler would have to reproduce.
- *Alternative:* wrap the public `Recursor` (which takes root IP addresses, so no hints file) in our own zone handler. That avoids the file but means implementing the zone handler's lookup and DNSSEC proof methods ourselves, and tracking the library as it changes. Revisit only if the hints file proves a problem.
- *Consequence:* the roots have to be a file (D2).

### D2. Ship the root hints in the image, outside `/etc/hickory-filter` **[proposed]**
The hints file is installed at `/usr/share/hickory-filter/root.hints` (through the melange source dir, like the notices) and is the default of `recursion.roots`. Outside the image the operator points `recursion.roots` at their own copy. Startup fails, naming the file, when it is missing, unreadable or lists no usable address.
- *Why not `/etc/hickory-filter`:* the chart mounts the ConfigMap there and would hide the file.
- *Alternatives:* embed the addresses in the binary and write a file at startup (breaks "writes no files"; `/tmp` would be needed), or a memory-backed file descriptor (needs `unsafe` or another crate; the project forbids `unsafe`).
- *Freshness:* root server addresses change rarely, and the recursor learns the live set from its first answers. The file is data under version control; refreshing it is a reviewed change, and the file records where and when it was taken.

### D3. Two exclusive modes, forward by default, no fallback **[proposed]**
`mode = "forward" | "recursive"`, validated at startup: `upstreams` in recursive mode and `[recursion]` in forward mode are errors naming the key. No automatic fallback in either direction.
- *Why:* a fallback from recursive to forward would quietly send queries to third parties, which defeats the mode's purpose and hides a broken network.
- *Alternative:* recursion as one upstream source among others, or per-zone forwarding. More useful and more surface; left for a later change.
- Recursive stays opt-in for the first release so forward deployments see no change.

### D4. Defaults **[confirmed: DNSSEC in scope, encryption on; proposed: the rest]**
In recursive mode: DNSSEC validation on, QNAME minimization strict, opportunistic encryption on, and the library's non-routable server ranges refused. Configuration, all under `[recursion]`: `roots`, `dnssec` (`"validate"`, `"off"`, or a trust anchor file path), `qname_minimization` (`"strict"`, `"relaxed"`), `opportunistic_encryption`, `state_file`. There is no case randomization setting (see Future improvements). The library's limits and cache sizes stay at its defaults.
- *Why validation on:* over plain UDP a recursor otherwise trusts whatever answer arrives first, which is weaker than verified DNS-over-TLS to one upstream. Validation is what makes "independence" not cost integrity.
- *Trust anchor staleness:* the anchor is static. When the root key rolls and the library's anchor is out of date, validating answers fail until the library is bumped. The mitigations are the `dnssec` file option and `"off"`, and a risk note below.

### D5. Opportunistic encryption: default on, unauthenticated, honest **[confirmed]**
Enabled through the library's option with its default periods and probe cap. The state file is optional and is the only file recursive mode may write (`recursion.state_file`, in a directory the operator makes writable, like the list cache). The spec and the README say plainly that it is unauthenticated, passive-observer-only and limited to servers that accept TLS. The encrypted share is visible through the existing protocol label on the upstream metrics (`tls`), and the dashboard's "Upstream queries by protocol" panel already plots it.
- *Why on by default:* the user's choice; the cost is extra probe connections and a little latency on first contact.

### D6. Readiness waits for the roots **[proposed]**
The library primes lazily, at the first query. At startup, after the listeners are bound and the lists are loaded, a task resolves one query through the recursive handler itself (for the root's NS records, bypassing the filter) with retries and backoff, and `/ready` stays 503 until one succeeds. After that, resolution failures never flip it back (as with list refresh failures).
- *Why:* a pod without working egress should not be ready, and the failure is then visible as a pod that never becomes ready, not as SERVFAILs that look like an upstream problem.
- *Alternative:* ready as soon as the listeners are up. Simpler, but a misconfigured network looks healthy.

### D7. `cache_size` and `bootstrap` **[proposed]**
`cache_size` sets the recursor's response cache size in recursive mode, with the same meaning as in forward mode, including `0` to disable it (the library's response cache is the same bounded cache and accepts a size of 0; checked in task 2.2 and by a test that every query then reaches the authoritative server). The recursor's name server cache keeps the library's default size. `filter.bootstrap` is unchanged (see Non-Goals).

### D8. Features and dependencies
`hickory-server` gets `recursor` and `dnssec-aws-lc-rs` (the TLS backend is already aws-lc-rs, so no second crypto library); the exact pin stays. `cargo deny` and the notices cover whatever new crates arrive. Binary and image size are measured and noted.

### D9. Test strategy **[proposed]**
- Hermetic resolution tests run the server, a fake root, a fake TLD and a fake authoritative server inside an unshared user and network namespace, the same technique `tests/sandbox.rs` uses for the restricted container. Root hints carry no port, so the fakes listen on port 53, which a namespace root may bind.
- The library refuses loopback, private and documentation ranges as servers, so the fakes are given addresses outside those ranges on the namespace's loopback interface (for example in the benchmarking range) and the hints name them. If the namespace can't add addresses, fall back to a test-only way to relax the server filter and say so in the task.
- DNSSEC tests need signed fake zones; a spike (task 5.1) finds how to produce them with what the library already provides before the validation tasks depend on it.
- Encryption tests need a fake name server that accepts TLS with a self-signed certificate.
- The image smoke test checks the shipped hints and the restricted-container start; it doesn't resolve against the real roots.

## Risks / Trade-offs

- [Plain DNS to the internet is visible to any on-path observer, which is worse than verified DNS-over-TLS to one upstream] → Opportunistic encryption for servers that accept TLS, honest documentation, VPN or private egress as guidance; this change targets resolver operators, not network observers.
- [Opportunistic encryption is unauthenticated and a downgrade is possible] → Documented in spec and README; never described as verified.
- [Outbound port 53 is blocked or intercepted on some networks] → The pod never becomes ready and the failure is visible; no silent fallback.
- [Root trust anchor rollover leaves validating answers failing until the library is bumped] → The `dnssec` file and `"off"` options; a release note when the library's anchor changes; revisit when the library implements rollover.
- [An open recursive listener can be used for amplification] → Same exposure as the open forwarder today, larger load; documented, access control left for a later change.
- [Forged UDP answers for unsigned zones are only held off by random transaction IDs and source ports] → DNSSEC covers signed zones; 0x20 case randomization is a future improvement, once the library applies it
- [Cache and recursion limits stay at the library defaults, which may be wrong for small deployments] → Measure memory with the dashboard and the existing memory benchmark, expose a knob only if the numbers ask for it.
- [Bogus answers and other SERVFAILs look the same to a client] → Check whether the library tells them apart so a metric can; not required by the specs.
- [The hints file ages] → It is data in the repository, refreshed through a reviewed change, and the recursor replaces it with the live set after first contact.

## Future improvements

- **0x20 case randomization.** Randomizing the case of query names sent over UDP, and rejecting answers that don't repeat it, makes a forged answer harder to land (about one more bit to guess per letter in the name). It was in the first proposal and is left out because the library doesn't deliver it: `hickory-resolver` 0.26.4, the latest release when this was written, has `RecursorOptions::case_randomization`, but the recursor's query path builds its request options without it, so queries leave in lower case whatever the setting (seen in a test against fake servers with the option on). There is therefore no `recursion.case_randomization` setting, and a configuration that names it fails at load as an unknown key. Do this once `hickory-server` applies the option: add the setting (on by default), and restore the two tests that were written and removed: a name server that answers with a lower-case question is rejected over UDP, and the same server is accepted with the setting off, which proves the rejection is the randomization. It may be worth reporting to the library's maintainers.

## Migration Plan

Nothing to migrate: forward mode is unchanged and recursive mode is opt-in. The first release containing it documents the egress requirement. Rolling back is setting `mode` back to `forward` with upstreams.

## Open Questions

- Whether the dashboard's panel titles should say "name servers" when recursion is on. The panels keep working either way, and the live check in task 6.4 decides.
- Whether a failed validation can be counted separately from other failures (see Risks).
