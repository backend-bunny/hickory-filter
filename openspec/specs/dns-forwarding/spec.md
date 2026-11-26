# dns-forwarding Specification

## Purpose
Serves DNS to clients and forwards every query the filter doesn't block to configured upstream resolvers, starting only from a valid configuration.

## Requirements

### Requirement: Configuration file
The server SHALL read its configuration from one TOML file, given by a command-line flag, with a documented default path. Unknown keys, invalid values and unreadable files SHALL fail startup with an error naming the file, key and reason. A `--check` flag SHALL validate the configuration and exit, without serving.

#### Scenario: Unknown key
- **WHEN** the configuration contains the key `listen_adress`
- **THEN** startup fails with an error naming the unknown key

#### Scenario: Check only
- **WHEN** the server runs with `--check` and a valid configuration
- **THEN** it exits with status 0 without opening any listener or fetching any list

### Requirement: Listeners
The server SHALL answer DNS over UDP and TCP on each configured listen address. The default is port 1053 on all addresses. Binding to a privileged port SHALL be possible when the runtime allows it, but SHALL NOT be the default.

#### Scenario: Default listener
- **GIVEN** a configuration without listen addresses
- **WHEN** a client sends a query to port 1053 over UDP and over TCP
- **THEN** both get an answer

#### Scenario: Bind failure
- **WHEN** a configured listen address can't be bound
- **THEN** startup fails with an error naming the address

### Requirement: Upstream forwarding
Queries that aren't blocked SHALL be forwarded to the configured upstream resolvers, and their answers returned to the client. Upstreams SHALL be given as IP addresses with optional port, over plain DNS or DNS-over-TLS (with the server name to verify). At least one upstream SHALL be required.

#### Scenario: Forwarded answer
- **GIVEN** an upstream resolver at `192.0.2.53`
- **WHEN** a client queries a name that isn't blocked
- **THEN** the client receives the upstream's answer

#### Scenario: No upstream configured
- **WHEN** the configuration has no upstream resolvers
- **THEN** startup fails with an error saying at least one is required

### Requirement: Upstream failure
When no upstream answers in time, the server SHALL return SERVFAIL to the client, and SHALL NOT return a blocked-style answer.

#### Scenario: Upstreams unreachable
- **GIVEN** all upstream resolvers are unreachable
- **WHEN** a client queries a name that isn't blocked
- **THEN** the client receives SERVFAIL

### Requirement: Response cache
Forwarded answers SHALL be cached for at most their TTL, with a configurable maximum number of entries. A change to the rules SHALL take effect for the next query even when its name is cached, because filtering happens before the cache is consulted.

#### Scenario: Newly blocked name was cached
- **GIVEN** `news.example` was answered from upstream and cached
- **WHEN** a refresh adds `block domain news.example` and a client queries `news.example`
- **THEN** the client receives the blocked answer

### Requirement: Non-root operation
The server SHALL run as an unprivileged user with no capabilities and a read-only root filesystem, given a listen port of 1024 or higher. It SHALL NOT write files except in the optional list cache directory.

#### Scenario: Restricted container
- **GIVEN** the server runs as uid 65532 with all capabilities dropped and a read-only root filesystem
- **WHEN** it starts with the default configuration and an upstream
- **THEN** it serves queries normally

### Requirement: Graceful shutdown
On SIGTERM or SIGINT the server SHALL stop accepting new queries, finish queries in flight for up to a bounded grace period (default 5 seconds), and exit with status 0.

#### Scenario: SIGTERM
- **WHEN** the server receives SIGTERM while idle
- **THEN** it exits with status 0 within the grace period
