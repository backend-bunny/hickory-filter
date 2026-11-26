# Spec Delta

## Purpose

Lets operators see what the filter is doing and whether it is healthy, through metrics, health and readiness endpoints, and logs.

## ADDED Requirements

### Requirement: HTTP endpoint
The server SHALL serve metrics, health and readiness over HTTP on one configurable address, default port 8080. It SHALL be separate from the DNS listeners and SHALL be possible to disable.

#### Scenario: Default endpoint
- **WHEN** the server runs with the default configuration
- **THEN** `/metrics`, `/health` and `/ready` answer on port 8080

### Requirement: Health
`/health` SHALL return 200 while the process is serving DNS, whatever the state of its lists.

#### Scenario: Healthy with failed lists
- **GIVEN** every remote list failed its last refresh
- **WHEN** a probe requests `/health`
- **THEN** it returns 200

### Requirement: Readiness
`/ready` SHALL return 503 until the initial list load has finished and the listeners are bound, and 200 after that. Later refresh failures SHALL NOT make it return 503.

#### Scenario: Not ready during initial load
- **GIVEN** the server is still fetching its lists at startup
- **WHEN** a probe requests `/ready`
- **THEN** it returns 503

#### Scenario: Ready after load
- **WHEN** the initial load has finished
- **THEN** `/ready` returns 200

### Requirement: Query metrics
`/metrics` SHALL expose, in Prometheus text format under documented names, counters of queries by outcome (`allowed` by an allow rule, `blocked`, `passed` with no match, `upstream_error`) and by query type, and the time taken to answer by outcome. The query-type label SHALL come from a fixed set of common record types, with every other type counted as `other`, so the number of series stays bounded whatever clients send. Metrics SHALL NOT carry client addresses or queried names.

#### Scenario: Blocked counter
- **WHEN** a client queries a blocked name once
- **THEN** the blocked-queries counter increases by one

#### Scenario: Query type label
- **WHEN** a client queries a blocked name for type AAAA
- **THEN** the counter for outcome `blocked` and type `AAAA` increases by one

#### Scenario: Unusual type stays bounded
- **WHEN** clients send queries of many different unassigned record types
- **THEN** they are all counted under type `other` and no new series appear

#### Scenario: No identifying labels
- **WHEN** `/metrics` is scraped after queries from several clients for several names
- **THEN** no metric carries a client address or a queried name

### Requirement: Upstream and cache metrics
`/metrics` SHALL expose the number of queries sent to each upstream and how many of them failed or timed out, and the response cache's hits and misses, so that upstream health and the cache's effectiveness are visible.

#### Scenario: Cache hit
- **GIVEN** a name was answered by an upstream and cached
- **WHEN** a client queries it again
- **THEN** the cache-hit count increases and the upstream's query count does not

#### Scenario: Unreachable upstream
- **GIVEN** an upstream that does not answer
- **WHEN** a client queries a name that isn't blocked
- **THEN** that upstream's failure count increases

### Requirement: Rule set metrics
`/metrics` SHALL expose the number of rules in the live rule set by action and kind (domain, wildcard, regex), how many times the rule set has been replaced, and the time of the last replacement.

#### Scenario: Refresh replaces the rule set
- **WHEN** a list refresh changes a list's rules
- **THEN** the replacement count increases by one and the last-replacement time moves forward

#### Scenario: Refresh without changes
- **WHEN** a refresh finds every list unchanged
- **THEN** the replacement count does not change

### Requirement: List metrics
For each list, `/metrics` SHALL expose its rule count, skipped-line count, the time of its last successful load, and the result of its last refresh attempt. Each list SHALL be labelled by action, type and URL, with any credentials in the URL removed.

#### Scenario: Failed refresh visible
- **GIVEN** a list whose last refresh got HTTP 503
- **WHEN** `/metrics` is scraped
- **THEN** that list's last refresh result shows a failure, and its last success time is unchanged

### Requirement: Logging
The server SHALL log to stderr, at a configurable level (default `info`), in plain text or JSON. List loads and refreshes SHALL be logged with list, result, rule count and skipped lines. Individual queries SHALL be logged only at `debug` level, or when the query log is enabled.

#### Scenario: Refresh failure logged
- **WHEN** a list refresh fails
- **THEN** a warning is logged with the list URL (credentials removed) and the reason

#### Scenario: No per-query logs by default
- **WHEN** clients send queries at the default log level
- **THEN** no per-query lines are logged

### Requirement: Query log
The server SHALL offer a query log, off by default, that writes one line per answered query to the same stderr stream, so that a log collector can build per-client and per-name views the metrics deliberately don't carry. Each line SHALL carry the client, the queried name, the query type, the outcome (as in the query metrics), the response code and the time taken, under documented field names.

#### Scenario: Query log off
- **GIVEN** the query log is not enabled
- **WHEN** a client queries a blocked name at the `info` level
- **THEN** no line for that query is logged

#### Scenario: Query line fields
- **GIVEN** the query log is enabled with JSON output
- **WHEN** client `192.0.2.10` queries the blocked name `ads.example` for type A
- **THEN** one line is logged whose object has `client` `192.0.2.10`, `name` `ads.example`, `type` `A`, `outcome` `blocked`, `rcode` `NOERROR` and a duration

### Requirement: Query log lines are selectable
In JSON format the query-log fields SHALL be top-level members of the line's object, and query lines SHALL carry a fixed target that lets a log collector select them from all other lines.

#### Scenario: Selecting query lines
- **GIVEN** the query log is enabled with JSON output, and a list refresh also logs a line
- **WHEN** a collector selects lines by the query-log target
- **THEN** it gets the query lines and not the refresh line, and finds `client` and `name` at the top level of each

### Requirement: Query log client privacy
The query log SHALL log the client as its address, as its address with the host part masked (the last octet of an IPv4 address zeroed, an IPv6 address cut to its first 48 bits), or omit it, as configured.

#### Scenario: Masked client
- **GIVEN** the query log is enabled with the client masked
- **WHEN** client `192.0.2.10` sends a query
- **THEN** the logged client is `192.0.2.0`

#### Scenario: Masked IPv6 client
- **GIVEN** the query log is enabled with the client masked
- **WHEN** client `2001:db8:1234:5678::9` sends a query
- **THEN** the logged client is `2001:db8:1234::`

#### Scenario: Omitted client
- **GIVEN** the query log is enabled with the client omitted
- **WHEN** a client sends a query
- **THEN** the line has no client field
