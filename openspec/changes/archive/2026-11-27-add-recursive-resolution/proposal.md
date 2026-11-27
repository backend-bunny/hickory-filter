# Proposal

## Why

hickory-filter forwards every query it doesn't block to upstream resolvers that someone else runs, so those operators see every name each client asks for, and the server depends on their uptime, policy and honesty. Operators who want neither the privacy cost nor the dependency need the server to resolve names itself, starting from the root servers, and to prove what it gets back.

## What Changes

- **A recursive mode.** A new `mode` setting selects `forward` (today's behaviour, the default) or `recursive`. In recursive mode, queries that aren't blocked are resolved by iterating from the root servers down to the authoritative servers, with no third-party resolver involved. The two modes are exclusive and there is no automatic fallback from one to the other, because a silent fallback would send queries to exactly the operators the mode exists to avoid.
- **Integrity by default.** Recursive mode validates DNSSEC against the root trust anchor, so a spoofed or tampered answer becomes SERVFAIL. It also uses QNAME minimization (each server sees only the labels it needs), which can be relaxed. 0x20 case randomization, a further spoofing mitigation for UDP, is not part of this change: the library doesn't apply it yet (see the design's future improvements).
- **Opportunistic encryption, on by default.** Recursive mode probes authoritative servers for TLS and uses it where it is accepted. It is unauthenticated and protects against passive observers only, for servers that accept TLS only. It can be turned off, and the documentation says what it does and doesn't give. Stronger network privacy (a VPN or other private egress) is documented guidance, not built.
- **Operating it.** `/ready` additionally waits for the root servers to be reachable, queries that can't be resolved return SERVFAIL (never a blocked-style answer and never a forwarded one), the existing upstream and cache metrics describe the name servers contacted, and the metrics' protocol label shows how much traffic was actually encrypted.
- **Packaging.** The image ships the root server hints the recursor starts from, the chart's values schema accepts the new settings, and the chart's NetworkPolicy does not restrict the egress recursion needs.
- **Revisits a recorded non-goal.** The first change listed recursive resolution as out of scope ("forwarding only"). This change replaces that boundary for the new mode; forward mode is unchanged.

## Capabilities

### New Capabilities

- `recursive-resolution`: the recursive mode: how it is selected, how it resolves from the roots, its failure behaviour, DNSSEC validation, QNAME minimization, name server address filtering, opportunistic encryption, and how filtering behaves unchanged on top of it.

### Modified Capabilities

- `dns-forwarding`: upstreams are required in forward mode only, the response cache covers resolved answers in both modes, and the "no files written" rule allows the optional encryption state file.
- `observability`: readiness in recursive mode waits for root reachability, and the upstream and cache metrics describe the name servers contacted in that mode.
- `helm-chart`: the values schema accepts the new settings with forward mode as the default, and an enabled NetworkPolicy does not block the egress recursion needs.
- `container-image`: the image ships the root server hints at a fixed path and starts in recursive mode in the restricted container.

## Impact

- Code: configuration (`mode`, `[recursion]`), a recursive branch next to the forwarder, a root priming and readiness check, metrics wiring, a startup check of the hints file.
- Dependencies: `hickory-server` gains its `recursor` feature and `dnssec-aws-lc-rs` (the TLS backend is already aws-lc-rs); the exact pin stays. New transitive crates go through the license allowlist and the notices.
- Packaging: a root hints file in the image (not under `/etc/hickory-filter`, which the chart mounts over), chart values and schema, README and chart README.
- Tests: recursion needs a fake root, TLD and authoritative servers on port 53 on non-loopback, non-private addresses, so hermetic tests run in an unshared user and network namespace; DNSSEC needs signed fake zones and encryption needs a fake server that speaks TLS.
- Operations: outbound UDP and TCP 53 and TCP 853 to arbitrary hosts must be allowed for recursive mode; some networks block or intercept port 53, in which case the server stays not ready.
- Depends on the archived changes for the forwarder, the filter, the metrics, the image and the chart. Forward mode and the dashboard's existing panels stay valid.
