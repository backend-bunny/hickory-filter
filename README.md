# hickory-filter

A filtering (ad/tracker-blocking) DNS forwarder built on the `hickory-server`
library. It answers queries for blocked names itself, forwards everything
else to upstream resolvers, and keeps its blocklists current by fetching them
from web servers on a schedule.

> **Status: design.** Nothing is implemented yet. The design lives in
> [`openspec/`](openspec/): start with the open change under
> `openspec/changes/`.

## Features (planned)

**Rules.** Each rule is `allow` or `block`, and **allow always wins**.

| Type | Matches | Example |
|------|---------|---------|
| `domain` | that exact name only | `block domain ads.example` |
| `wildcard` | the name and every subdomain | `block wildcard *.ads.example` |
| `regex` | names matching the expression | `block regex ^ad[0-9]+\.` |

Wildcards accept several spellings for the same rule: `x`, `*.x`, `.x`,
`||x^` and `address=/x/#`.

**Lists.** Rules can be inline or come from lists at `file://`, `http://` or
`https://` URLs. Each list has one type: `domain`, `hosts` (`<address> <name>`
lines with any address, e.g. `0.0.0.0 x` or `127.0.0.1 x`), `wildcard` or
`regex`. Comments and blank lines are ignored. Lines that can't be parsed
(including `$option` suffixes such as `||x^$important`) are skipped and
counted, never misread.

**Refresh.** Each remote list is re-fetched on an interval (default 24h,
`0` = never). `ETag`/`If-Modified-Since` mean unchanged lists aren't
downloaded again. A failed fetch keeps serving the **last good copy** of that
list, optionally cached on disk so a restart works without network. List
hosts are resolved through a configurable bootstrap resolver (plain DNS or
DNS-over-TLS), never through hickory-filter itself.

**Responses** to blocked names:

| Mode | A / AAAA query | Other types |
|------|----------------|-------------|
| `null` (default) | `0.0.0.0` / `::` | NOERROR, no records |
| `address` | the configured IPv4 / IPv6 | NOERROR, no records |
| `nodata` | NOERROR, no records | NOERROR, no records |
| `nxdomain` | NXDOMAIN + SOA | NXDOMAIN + SOA |

**Operations.** Prometheus metrics (queries allowed/blocked, rules per list,
last refresh time and result), `/health`, and `/ready` (ready only after the
first successful rule load). Runs as non-root, binds ports ≥ 1024 by default,
needs no writable root filesystem, and logs to stderr.

## How it works (planned)

A standalone binary using `hickory-server` as a library:

```
          DNS query (UDP/TCP, later DoT/DoH)
                       │
   ┌───────────────────┴────────────────────┐
   │ FilterHandler (impl RequestHandler)     │──blocked──▶ blocked response
   └───────────────────┬────────────────────┘
                       │ not blocked
   ┌───────────────────┴────────────────────┐
   │ Catalog "." → ForwardZoneHandler        │──▶ upstream resolvers
   └─────────────────────────────────────────┘

 list refresher (tokio task) ──fetch/parse──▶ new RuleSet ──swap──▶ FilterHandler
```

- The filter sits in front of the catalog, so it decides before anything is
  forwarded or cached, and it builds blocked responses itself (rcode, flags,
  SOA).
- Rules: exact names in a hash set, wildcards as a suffix set checked label
  by label, and all regexes compiled into one set (one pass per query).
- A refresh builds a complete new rule set off the query path and swaps it
  in atomically, so queries never see a half-loaded list.

## Memory (estimates, not measured)

About 85–95 bytes per blocked domain, so roughly 90–110 MiB steady state for
1M domains. There's no garbage collector, so memory tracks the live rule
set. A refresh briefly holds the old and new sets at once. A reversed-domain
FST is a later option, plausibly 10–30 MB for 1M domains. Benchmark before
relying on any of these numbers.

## Packaging (planned)

- **Image:** `<registry>/<owner>/hickory-filter`, built with melange and apko
  (no Dockerfile). A static musl binary plus CA certificates: no shell, no
  libc. Runs as uid/gid 65532 with all capabilities dropped and a read-only
  root filesystem. Tags `X.Y.Z` and `sha-<commit>`; **pin by digest**.
- **Helm chart:** `oci://<registry>/<owner>/charts/hickory-filter`. Each
  release pins the image digest it shipped with, and its defaults meet the
  `restricted` Pod Security Standard.
- **Verifiable:** image and chart are signed keylessly with cosign and carry
  SLSA provenance. The image also carries an SPDX SBOM (including the Rust
  crates) and a third-party license notices attestation. The notices are
  also in the image at `/usr/share/doc/hickory-filter/`.

## License

Not decided yet; the proposal is `MIT OR Apache-2.0`.
