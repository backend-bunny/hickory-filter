# hickory-filter

A filtering (ad/tracker-blocking) DNS forwarder built on the `hickory-server`
library. It answers queries for blocked names itself, forwards everything
else to upstream resolvers, and keeps its blocklists current by fetching them
from web servers on a schedule.

> **Status.** The DNS server is implemented: filtering, lists with scheduled
> refresh, forwarding, metrics and logs. The container image and the Helm chart
> build and are tested in CI; the first signed release is still to come (see
> [Container image, chart and releases](#container-image-chart-and-releases)).
> The design lives in [`openspec/`](openspec/).

## Quick start

Write a configuration. Only `upstreams` is required; give it your resolver's
IP address:

```toml
upstreams = ["192.0.2.53"]

[filter]
rules = ["block wildcard *.ads.example", "allow domain ok.ads.example"]
```

Check it, then run it:

```sh
hickory-filter --config config.toml --check
hickory-filter --config config.toml
```

By default it answers DNS on port 1053 (UDP and TCP, all addresses) and serves
`/health`, `/ready` and `/metrics` on port 8080. Try it:

```sh
dig @127.0.0.1 -p 1053 x.ads.example     # blocked: 0.0.0.0
dig @127.0.0.1 -p 1053 ok.ads.example    # allowed: forwarded
curl -s localhost:8080/ready
```

`--version` prints the version and `--help` the options. SIGTERM or SIGINT
stops accepting queries, gives the ones in flight up to five seconds, and exits
with status 0. A configuration error, a listen address that can't be bound, or
a list marked `required` that can't be loaded stops startup with the reason on
stderr.

## Features

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

## How it works

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
exits without serving. `examples/hickory-filter.toml` is an annotated example
with every key, and the end-to-end tests run the binary from it.

### Configuration reference

Keys of the `[filter]`, `[filter.response]`, `[log]` and `[[filter.lists]]`
tables are written with their table: `filter.refresh` is `refresh` in
`[filter]`.

| Key | Default | Meaning |
|---|---|---|
| `listen` | `0.0.0.0:1053`, `[::]:1053` | DNS listen addresses, each answering UDP and TCP. An IPv6 address on a host without IPv6 is skipped with a note |
| `upstreams` | required | Resolvers for names that aren't blocked, as IP addresses (forms below). At least one |
| `cache_size` | `4096` | Most forwarded answers kept in the response cache, each for at most its TTL. `0` disables it |
| `http` | `0.0.0.0:8080` | Address of the health, readiness and metrics endpoint, or `false` for none |
| `log.level` | `info` | `error`, `warn`, `info`, `debug` or `trace` |
| `log.format` | `text` | `text` or `json` (one object per line) |
| `log.query_log` | `false` | One log line per answered query; see Metrics and logs |
| `log.query_log_client` | `address` | How the client appears in query lines: `address`, `masked` or `omit` |
| `filter.refresh` | `24h` | How often lists are re-fetched; `0` never |
| `filter.bootstrap` | none | Resolver for list host names, as an IP address; the system resolver when unset. Never this server |
| `filter.cache_dir` | none | Directory for the on-disk copy of each remote list. Nothing is written without it |
| `filter.rules` | none | Inline rules, one line each, such as `block wildcard *.ads.example` |
| `filter.response.mode` | `null` | What a blocked query gets: `null`, `address`, `nodata` or `nxdomain` |
| `filter.response.ipv4` | none | The address for blocked A queries in `address` mode |
| `filter.response.ipv6` | none | The address for blocked AAAA queries in `address` mode |
| `filter.response.ttl` | `3600` | TTL of blocked answers, in seconds |
| `filter.lists[].rule` | required | A list declaration, `<allow\|block> list <domain\|hosts\|wildcard\|regex> <url>` |
| `filter.lists[].refresh` | `filter.refresh` | This list's refresh interval; `0` never |
| `filter.lists[].required` | `false` | Fail startup if the list can't be loaded (a cached copy counts as loaded) |

### Resolver addresses

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

### Grafana dashboard

`charts/hickory-filter/dashboards/hickory-filter.json` is a Grafana dashboard
for the metrics and the query log above. It picks its datasources when it is
opened (a **Metrics** variable for a Prometheus-compatible store and a **Logs**
variable for Loki), so the same file works however it is loaded: imported by
hand, or provisioned from a ConfigMap or an operator resource.

- **Overview**, from the metrics only: queries and the share blocked, rules
  loaded, rule set age, queries per second by outcome
  (`hickory_filter_queries_total`), answer-time quantiles
  (`hickory_filter_query_duration_seconds`), queries by type, upstream queries
  and responses (`hickory_resolver_outgoing_queries_total`,
  `hickory_resolver_name_server_query_responses_total`), the response cache hit
  ratio (`hickory_resolver_cache_hit_total`, `hickory_resolver_cache_miss_total`),
  rules by action and kind (`hickory_filter_rules`), and a table of lists with
  each list's rules, skipped lines, time since its last successful load and last
  refresh result (`hickory_filter_list_rules`, `hickory_filter_list_skipped_lines`,
  `hickory_filter_list_last_success_timestamp_seconds`,
  `hickory_filter_list_last_refresh_success`).
- **Usage**, from the query log in Loki: the most requested blocked names, the
  most requested permitted names, the busiest clients and the query rate per
  client. These stay empty unless `log.query_log = true` with `format = "json"`
  and the logs are collected into Loki. The **Log stream selector** variable
  holds the labels that identify hickory-filter's log lines. The queries select
  lines by the target `hickory_filter::query` and use only the documented
  fields.

A test fails if a panel queries a metric that isn't in the metrics reference or
uses a log field that isn't in the query log fields.

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

## Container image, chart and releases

- **Image:** `ghcr.io/backend-bunny/hickory-filter`, built with melange and apko
  (no Dockerfile). A static musl binary plus CA certificates: no shell, no
  libc. Runs as uid/gid 65532, so it needs no capabilities and works with a
  read-only root filesystem (the only writable path it needs is `/tmp`, plus a
  cache directory if you enable one). The configuration is read from
  `/etc/hickory-filter/config.toml`. Tags are `X.Y.Z` and `sha-<commit>`, never
  `latest`: **pin by digest**.
- **Helm chart:** `oci://ghcr.io/backend-bunny/charts/hickory-filter`. Each
  release pins the image digest it shipped with, and its defaults meet the
  `restricted` Pod Security Standard. See [`charts/hickory-filter`](charts/hickory-filter/README.md).
- **Verifiable:** image and chart are signed keylessly with cosign and carry
  build provenance. The image also carries an SPDX SBOM (including the Rust
  crates) and a third-party license notices attestation. The notices are
  also in the image at `/usr/share/doc/hickory-filter/`.

### Verify a release

Every release's notes list the image digest and the chart digest. Use them
with the identity of the release workflow:

```sh
IMAGE=ghcr.io/backend-bunny/hickory-filter@sha256:<digest>
IDENTITY='^https://github.com/backend-bunny/hickory-filter/'
ISSUER=https://token.actions.githubusercontent.com

# signature, SBOM and license notices on the image index
cosign verify "$IMAGE" --certificate-identity-regexp "$IDENTITY" --certificate-oidc-issuer "$ISSUER"
cosign verify-attestation --type spdxjson "$IMAGE" \
  --certificate-identity-regexp "$IDENTITY" --certificate-oidc-issuer "$ISSUER"
cosign verify-attestation \
  --type https://github.com/backend-bunny/hickory-filter/notices/v1 "$IMAGE" \
  --certificate-identity-regexp "$IDENTITY" --certificate-oidc-issuer "$ISSUER"

# build provenance of the image and of the chart
gh attestation verify "oci://$IMAGE" --repo backend-bunny/hickory-filter
gh attestation verify "oci://ghcr.io/backend-bunny/charts/hickory-filter@sha256:<chart digest>" \
  --repo backend-bunny/hickory-filter
cosign verify ghcr.io/backend-bunny/charts/hickory-filter@sha256:<chart digest> \
  --certificate-identity-regexp "$IDENTITY" --certificate-oidc-issuer "$ISSUER"
```

### How a release is made

Releases come from Conventional Commit messages. Every merge to `main`
updates one pull request titled `chore: release X.Y.Z`, which bumps the
version everywhere it appears (`Cargo.toml`, `Cargo.lock`, the package
recipe and the chart) and updates `CHANGELOG.md`. **Merging that pull
request is the release:** it creates the `vX.Y.Z` tag and the GitHub Release,
runs the checks again on the tagged commit, builds and tests the image, pushes
it, signs and attests it, then publishes and signs the chart pinned to that
image digest. Pushing a tag by hand publishes nothing. Locally, `task build`
builds the image and `task test` runs the smoke test (both need a container
runtime), `task chart:test` checks the chart, and `task publish:dry-run` shows
the tags and packages the chart without pushing.

## License

Licensed under either of MIT or Apache-2.0, at your option (`MIT OR Apache-2.0`).
