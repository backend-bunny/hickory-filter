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
- Rules: exact names and wildcard bases in reversed-domain FSTs (one walk
  over the automaton per lookup), and all regexes compiled into one set (one
  pass per query).
- A refresh builds a complete new rule set off the query path and swaps it
  in atomically, so queries never see a half-loaded list.

## Configuration

One TOML file, `--config <file>` (default `/etc/hickory-filter/config.toml`).
Unknown keys, invalid values and unreadable files stop startup with an error
naming the file, the key and the reason. `--check` validates the file and
exits without serving. See `examples/hickory-filter.toml` for every key.

Upstream and bootstrap resolvers are IP addresses, never host names:

| Form | Meaning |
|---|---|
| `192.0.2.53`, `dns://192.0.2.53` | plain DNS, port 53 |
| `192.0.2.53:5353` | plain DNS, explicit port |
| `tls://192.0.2.53#dns.example` | DNS-over-TLS, port 853, certificate verified for `dns.example` |
| `tls://[2001:db8::53]:8853#dns.example` | DNS-over-TLS, IPv6, explicit port |

`tls://` requires the `#name`. Refresh intervals take units (`"6h"`, `"90m"`);
`0` disables.

## Metrics and logs

An HTTP endpoint serves `/health`, `/ready` and `/metrics` on the address in
`http` (default `0.0.0.0:8080`; `http = false` turns it off). `/health` is 200
while the process runs, whatever state the lists are in. `/ready` is 503 until
the listeners are bound and the first list load has finished, then 200; a later
refresh failure never makes it fail. `/metrics` is the Prometheus text format.

Metrics never carry a client address or a queried name: those would make the
number of series unbounded and are personal data. Per-client and per-name
views come from the query log below.

### Metrics reference

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `hickory_filter_queries_total` | counter | `outcome`, `type` | Queries answered. `outcome` is `allowed` (an allow rule matched, forwarded), `blocked`, `passed` (no match, forwarded) or `upstream_error` (forwarded, no upstream answered). `type` is A, AAAA, CNAME, HTTPS, MX, NS, PTR, SOA, SRV, SVCB, TXT, or `other` |
| `hickory_filter_query_duration_seconds` | histogram | `outcome` | Time taken to answer |
| `hickory_filter_rules` | gauge | `action`, `kind` | Rules in the live rule set, by `allow` or `block` and `domain`, `wildcard` or `regex` |
| `hickory_filter_ruleset_swaps_total` | counter | | Times the rule set was replaced (the first rule set at startup counts) |
| `hickory_filter_ruleset_last_swap_timestamp_seconds` | gauge | | Unix time of the last replacement |
| `hickory_filter_list_rules` | gauge | `action`, `type`, `url` | Rules in a list's last good content |
| `hickory_filter_list_skipped_lines` | gauge | `action`, `type`, `url` | Lines skipped as invalid in the last successful load |
| `hickory_filter_list_last_success_timestamp_seconds` | gauge | `action`, `type`, `url` | Unix time of the last successful load or refresh |
| `hickory_filter_list_last_refresh_success` | gauge | `action`, `type`, `url` | 1 if the last refresh attempt succeeded, 0 if it failed |
| `hickory_resolver_outgoing_queries_total` | counter | `protocol` | Queries sent to the upstreams |
| `hickory_resolver_name_server_query_responses_total` | counter | `protocol`, `status` | Upstream responses by outcome (`success`, `timeout`, and failures), aggregated over the upstreams |
| `hickory_resolver_cache_hit_total` | counter | | Queries answered from the response cache |
| `hickory_resolver_cache_miss_total` | counter | | Queries that had to go upstream |

The list `url` label has credentials removed, and a query string is shown as
`?…`. A list that has not been loaded yet has no series.

### Query log

The query log is off by default. Queries are then logged only at the `debug`
level. With `query_log = true` in the `[log]` table, every answered query is
logged at `info` (and shows even at a quieter level), so that a log collector
can turn the lines into per-client and per-name views. Lines carry the target
`hickory_filter::query`; in JSON (`format = "json"`) the fields below are
top-level members of the line's object, so a collector can select by target
and read them directly.

#### Query log fields

| Field | Meaning |
|---|---|
| `client` | The client address, as set by `query_log_client`: `address` (the default), `masked` (the last octet of an IPv4 address zeroed, an IPv6 address cut to its first 48 bits) or `omit` (no field) |
| `name` | The queried name, lower case, without the trailing dot |
| `type` | The query type, for example `A` or `AAAA` |
| `outcome` | `allowed`, `blocked`, `passed` or `upstream_error`, as in the metrics |
| `rcode` | The response code sent to the client, such as `NOERROR`, `NXDOMAIN` or `SERVFAIL` |
| `duration_ms` | The time taken to answer, in milliseconds |

Other lines are logged too: list loads and refreshes (`list` with credentials
removed, `result` as `loaded`, `unchanged` or `failed`, `rules`, `skipped`, and
a warning with `reason` on failure) and rule set replacements. `level` is
`error`, `warn`, `info` (the default), `debug` or `trace`, and `format` is
`text` (the default) or `json`.

## Memory and lookup cost (measured)

Exact and wildcard names live in reversed-domain FSTs (finite-state
transducers). Measured with `cargo run --release --example memory` and
`cargo bench --bench lookup` on 1M exact names plus 5,000 wildcards, 50
regexes and 100 allow rules (x86_64 Linux, glibc allocator, Rust 1.98.1). The
names are generated to look like a merged blocklist: about 114,000
registrable domains under a skewed spread of public suffixes, about 1% of
them with 50 to 400 names and most with a handful, and subdomain labels
mixing common words, digits and random hex. Compared with the hash-set
implementation it replaced, on the same data:

| Memory | Hash sets | FST |
|---|---|---|
| Steady state, resident | 70 MiB (71 bytes per name) | 21 MiB (19 bytes per name) |
| Size of the name index itself | not measured | 7.9 MiB (about 8 bytes per name) |
| Peak while a refresh builds the replacement | 153 MiB | 83 MiB |
| After the swap | 119 MiB | 23 MiB |
| Building the set | 0.6 s | 0.7 s |

| Lookup | Hash sets | FST |
|---|---|---|
| Exact hit | 42 ns | 175 ns |
| Wildcard hit, two labels below the base | 87 ns | 290 ns |
| Regex hit (50 patterns) | 119 ns | 193 ns |
| Miss | 100 ns | 187 ns |
| Miss under a populated suffix (`com`) | 116 ns | 284 ns |

The FST gives up two to four times on lookup speed, still well under a
microsecond and negligible next to a DNS round trip, for a steady footprint
about 70% smaller and a refresh peak about 45% smaller. Memory tracks the live
rule set, since there is no garbage collector, and a refresh briefly holds the
old and new sets while the replacement builds, so size for the peak figure. The
build collects its keys in one contiguous buffer instead of one allocation per
name, which keeps the heap from fragmenting and lets freed memory go back to
the OS; most of the remaining peak is that transient buffer. The numbers come
from generated names and the development (glibc) build. The release binary is
statically linked against musl, whose allocator behaves differently, so they
should be repeated on a real merged list and on the release build.

## Packaging (planned)

- **Image:** `ghcr.io/backend-bunny/hickory-filter`, built with melange and apko
  (no Dockerfile). A static musl binary plus CA certificates: no shell, no
  libc. Runs as uid/gid 65532 with all capabilities dropped and a read-only
  root filesystem. Tags `X.Y.Z` and `sha-<commit>`; **pin by digest**.
- **Helm chart:** `oci://ghcr.io/backend-bunny/charts/hickory-filter`. Each
  release pins the image digest it shipped with, and its defaults meet the
  `restricted` Pod Security Standard.
- **Verifiable:** image and chart are signed keylessly with cosign and carry
  SLSA provenance. The image also carries an SPDX SBOM (including the Rust
  crates) and a third-party license notices attestation. The notices are
  also in the image at `/usr/share/doc/hickory-filter/`.

## License

Licensed under either of MIT or Apache-2.0, at your option (`MIT OR Apache-2.0`).
