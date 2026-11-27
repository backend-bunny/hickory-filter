# Spec Delta

## MODIFIED Requirements

### Requirement: Readiness
`/ready` SHALL return 503 until the initial list load has finished and the listeners are bound, and 200 after that. In recursive mode it SHALL also return 503 until a query to the root servers has succeeded since startup, so a server that can't reach them never reports ready. Later refresh failures and later resolution failures SHALL NOT make it return 503.

#### Scenario: Not ready during initial load
- **GIVEN** the server is still fetching its lists at startup
- **WHEN** a probe requests `/ready`
- **THEN** it returns 503

#### Scenario: Ready after load
- **WHEN** the initial load has finished
- **THEN** `/ready` returns 200

#### Scenario: Roots unreachable in recursive mode
- **GIVEN** recursive mode, finished lists and root servers that don't answer
- **WHEN** a probe requests `/ready`
- **THEN** it returns 503

#### Scenario: Roots answer
- **WHEN** a root server answers the server's first query
- **THEN** `/ready` returns 200, and stays 200 if a later resolution fails

### Requirement: Upstream and cache metrics
`/metrics` SHALL expose the number of queries sent to the upstreams, by transport protocol, how many of their responses succeeded and how many timed out or failed, and the response cache's hits and misses, so that upstream health and the cache's effectiveness are visible. In recursive mode the upstreams are the name servers contacted, and the transport protocol label SHALL distinguish encrypted (`tls`) from plain (`udp`, `tcp`) queries, so the share of encrypted traffic is visible.

#### Scenario: Cache hit
- **GIVEN** a name was answered by an upstream and cached
- **WHEN** a client queries it again
- **THEN** the cache-hit count increases and the count of queries sent upstream does not

#### Scenario: Unreachable upstream
- **GIVEN** an upstream that does not answer
- **WHEN** a client queries a name that isn't blocked
- **THEN** the count of timed-out or failed upstream responses increases

#### Scenario: Encrypted share in recursive mode
- **GIVEN** recursive mode and a name server that accepts TLS
- **WHEN** clients query names it serves
- **THEN** the query count for the `tls` transport increases and the `udp` count for that server does not
