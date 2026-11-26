# Spec Delta

## Purpose

Defines exactly what a client receives when its query is blocked, per response mode and query type, so blocked names fail fast and predictably.

## ADDED Requirements

### Requirement: Response mode selection
One response mode SHALL apply to all blocked queries: `null` (the default), `address`, `nodata` or `nxdomain`. An unknown mode SHALL be a configuration error.

#### Scenario: Default mode
- **GIVEN** no response mode is configured
- **WHEN** a client queries a blocked name for type A
- **THEN** the answer is `0.0.0.0`

### Requirement: Null mode
In `null` mode, a blocked A query SHALL be answered with `0.0.0.0` and a blocked AAAA query with `::`. Every other query type SHALL get NOERROR with no answer records.

#### Scenario: AAAA in null mode
- **WHEN** a client queries a blocked name for type AAAA
- **THEN** the answer is `::` with rcode NOERROR

#### Scenario: MX in null mode
- **WHEN** a client queries a blocked name for type MX
- **THEN** the response is NOERROR with an empty answer section (no A record)

### Requirement: Address mode
In `address` mode, the configuration SHALL give at most one IPv4 and at most one IPv6 address, and at least one of them. A blocked A or AAAA query SHALL be answered with the configured address of that family, or with NOERROR and no records when that family isn't configured. Other types SHALL get NOERROR with no answer records.

#### Scenario: Sinkhole address
- **GIVEN** `address` mode with IPv4 `192.0.2.80` and IPv6 `2001:db8::80`
- **WHEN** a client queries a blocked name for A and for AAAA
- **THEN** the answers are `192.0.2.80` and `2001:db8::80`

#### Scenario: Missing family
- **GIVEN** `address` mode with only IPv4 `192.0.2.80`
- **WHEN** a client queries a blocked name for AAAA
- **THEN** the response is NOERROR with no answer records

### Requirement: Nodata mode
In `nodata` mode, every blocked query SHALL get NOERROR with no answer records.

#### Scenario: Nodata
- **WHEN** a client queries a blocked name for type A in `nodata` mode
- **THEN** the response is NOERROR with an empty answer section

### Requirement: Nxdomain mode
In `nxdomain` mode, every blocked query SHALL get rcode NXDOMAIN, with the authoritative-answer flag set and an SOA record for the queried name in the authority section, so resolvers can cache the negative answer.

#### Scenario: NXDOMAIN with SOA
- **WHEN** a client queries `ads.example` for type A in `nxdomain` mode
- **THEN** the response has rcode NXDOMAIN, the AA flag, and an SOA record for `ads.example` in the authority section

### Requirement: Blocked answer TTL
Records in blocked answers, including the SOA's negative-caching value, SHALL use one configurable TTL, defaulting to 3600 seconds.

#### Scenario: Custom TTL
- **GIVEN** a blocked-answer TTL of 60 seconds in `null` mode
- **WHEN** a client queries a blocked name for type A
- **THEN** the `0.0.0.0` record has TTL 60

### Requirement: Blocked answers are not forwarded
A blocked query SHALL be answered by the server without contacting any upstream resolver. Its response SHALL echo the query's ID, question and recursion-desired flag.

#### Scenario: No upstream traffic
- **GIVEN** an upstream resolver that records every query it receives
- **WHEN** a client queries a blocked name
- **THEN** the client gets the blocked answer and the upstream receives nothing for that name
