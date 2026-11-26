# blocklist-sources Specification

## Purpose
Loads rule lists from local files and web servers, keeps them current on a schedule, and keeps serving the last good rules when a source fails.

## Requirements

### Requirement: List declaration
A list SHALL be declared with an action (`allow` or `block`), a list type (`domain`, `hosts`, `wildcard` or `regex`) and a URL with scheme `file`, `http` or `https`. Any other scheme, an `http(s)` URL without a host, or an unknown list type SHALL be a configuration error.

#### Scenario: Remote list declared
- **WHEN** the configuration declares `block list hosts https://lists.example/hosts.txt`
- **THEN** the server fetches that URL and applies its lines as block rules

#### Scenario: Unsupported scheme rejected
- **WHEN** the configuration declares a list at `ftp://lists.example/a.txt`
- **THEN** startup fails with an error naming the URL and the supported schemes

### Requirement: Initial load
At startup the server SHALL load every list before it reports ready. A list that can't be loaded and has no cached copy SHALL be logged and counted as failed. The server SHALL still start with the rules it could load, unless that list is marked `required`, in which case startup SHALL fail.

#### Scenario: Optional list unavailable at startup
- **GIVEN** a block list whose server is unreachable, with no cached copy
- **WHEN** the server starts
- **THEN** it starts, serves the other rules, and reports that list as failed in its metrics

#### Scenario: Required list unavailable at startup
- **GIVEN** a block list marked `required` whose file doesn't exist and which has no cached copy
- **WHEN** the server starts
- **THEN** startup fails with an error naming the list

### Requirement: Scheduled refresh
Lists SHALL be reloaded on a refresh interval. It defaults to 24 hours, can be set globally and per list, and takes any duration. An interval of `0` SHALL disable refresh for the lists it applies to. Refreshes SHALL NOT overlap, and SHALL NOT delay the answer to any query.

#### Scenario: List changes upstream
- **GIVEN** a block list refreshed every 1 hour, which gains the line `new.example`
- **WHEN** the next refresh completes
- **THEN** queries for `new.example` are blocked, without a restart

#### Scenario: Refresh disabled
- **GIVEN** the global refresh interval `0`
- **WHEN** the server has been running longer than 24 hours
- **THEN** it has fetched each list exactly once, at startup

### Requirement: Conditional fetching
For `http(s)` lists, a refresh SHALL send the `ETag` and `Last-Modified` validators from the last successful response (`If-None-Match`, `If-Modified-Since`). A `304 Not Modified` answer SHALL keep the current rules for that list without reparsing them.

#### Scenario: Unchanged list
- **GIVEN** a list whose server answered the last fetch with an `ETag`
- **WHEN** the next refresh gets `304 Not Modified`
- **THEN** the list's rules are unchanged and the refresh counts as successful

### Requirement: Last good copy on failure
When a refresh of a list fails (network error, non-success HTTP status, timeout, or unreadable file), the server SHALL keep serving that list's rules from its last successful load and SHALL record the failure. A failed fetch SHALL NOT remove that list's rules.

#### Scenario: Server down during refresh
- **GIVEN** a block list that loaded `ads.example` and whose server then returns HTTP 503
- **WHEN** the refresh runs
- **THEN** `ads.example` stays blocked, and the list's last refresh result shows a failure

### Requirement: Atomic rule updates
A refresh SHALL take effect all at once: every query SHALL see either the complete previous rule set or the complete new one, never a mix or a partly loaded list.

#### Scenario: Query during refresh
- **GIVEN** a refresh replacing a large list
- **WHEN** queries arrive while the new list is being parsed
- **THEN** they are answered from the complete previous rule set until the new one is in place

### Requirement: Optional on-disk cache
When a cache directory is configured, the server SHALL store the last successful content and validators of each remote list there. On startup it SHALL use a cached copy for any list it can't fetch. Without a cache directory, the server SHALL write nothing to disk.

#### Scenario: Restart without network
- **GIVEN** a cache directory holding a previous copy of a remote list
- **WHEN** the server starts while that list's server is unreachable
- **THEN** it serves the cached list's rules and reports ready

### Requirement: Fetch limits
List fetches SHALL use a timeout (default 30 seconds) and a maximum list size (default 64 MiB). A fetch that exceeds either SHALL count as failed and keep the last good copy. Redirects SHALL be followed only to `https` URLs, or to `http` when the original URL was `http`.

#### Scenario: Oversized list
- **GIVEN** a list whose response grows beyond the maximum size
- **WHEN** it is fetched
- **THEN** the fetch fails, the previous rules stay in effect, and the failure is logged

### Requirement: Bootstrap resolver for list hosts
When a bootstrap resolver is configured, list host names SHALL be resolved only through it. It accepts an IP address with optional port, using plain DNS (`dns://`, the default) or DNS-over-TLS (`tls://`, with the server name to verify). Without one, the system resolver SHALL be used. List hosts SHALL NOT be resolved through this server's own filter.

#### Scenario: DNS-over-TLS bootstrap
- **GIVEN** the bootstrap resolver `tls://192.0.2.53` with server name `dns.example`
- **WHEN** a remote list is fetched
- **THEN** its host is resolved over TLS to 192.0.2.53, verifying the certificate for `dns.example`

#### Scenario: Name instead of address rejected
- **WHEN** the bootstrap resolver is configured as `dns://resolver.example`
- **THEN** startup fails, because the bootstrap resolver must be an IP address
