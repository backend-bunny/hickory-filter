# Spec Delta

## MODIFIED Requirements

### Requirement: Upstream forwarding
In forward mode, queries that aren't blocked SHALL be forwarded to the configured upstream resolvers, and their answers returned to the client. Upstreams SHALL be given as IP addresses with optional port, over plain DNS or DNS-over-TLS (with the server name to verify). At least one upstream SHALL be required in forward mode. In recursive mode no upstreams are configured (see `recursive-resolution`).

#### Scenario: Forwarded answer
- **GIVEN** an upstream resolver at `192.0.2.53`
- **WHEN** a client queries a name that isn't blocked
- **THEN** the client receives the upstream's answer

#### Scenario: No upstream configured
- **WHEN** the configuration has no upstream resolvers and the mode is `forward`
- **THEN** startup fails with an error saying at least one is required

### Requirement: Response cache
Answers for queries that aren't blocked, whether forwarded or resolved recursively, SHALL be cached for at most their TTL, with a configurable maximum number of entries. A change to the rules SHALL take effect for the next query even when its name is cached, because filtering happens before the cache is consulted.

#### Scenario: Newly blocked name was cached
- **GIVEN** `news.example` was answered from upstream and cached
- **WHEN** a refresh adds `block domain news.example` and a client queries `news.example`
- **THEN** the client receives the blocked answer

#### Scenario: Cache in recursive mode
- **GIVEN** recursive mode and a name that was resolved and cached
- **WHEN** a client queries it again within its TTL
- **THEN** no name server receives a query for it

### Requirement: Non-root operation
The server SHALL run as an unprivileged user with no capabilities and a read-only root filesystem, given a listen port of 1024 or higher. It SHALL NOT write files except in the optional list cache directory and the optional encryption state file of recursive mode.

#### Scenario: Restricted container
- **GIVEN** the server runs as uid 65532 with all capabilities dropped and a read-only root filesystem
- **WHEN** it starts with the default configuration and an upstream
- **THEN** it serves queries normally

#### Scenario: Restricted container in recursive mode
- **GIVEN** the same restricted container and `mode = "recursive"` without a state file
- **WHEN** it starts
- **THEN** it serves queries normally and writes no file
