# Spec Delta

## Purpose

Resolves queries that aren't blocked by iterating from the DNS root servers instead of asking a third-party resolver, so no outside operator sees every name clients ask for, and proves what it gets back where the data is signed.

## ADDED Requirements

### Requirement: Mode selection
The configuration SHALL have a `mode` setting with the values `forward` (the default) and `recursive`. In `recursive` mode no `upstreams` SHALL be configured, and in `forward` mode no `[recursion]` table SHALL be. Either mismatch SHALL fail startup with an error naming the key. The two modes SHALL NOT be combined, and the server SHALL NOT fall back from one to the other.

#### Scenario: Default mode
- **GIVEN** a configuration with `upstreams` and no `mode`
- **WHEN** the server starts
- **THEN** it forwards, as before

#### Scenario: Upstreams in recursive mode
- **GIVEN** `mode = "recursive"` and a non-empty `upstreams`
- **WHEN** the server starts
- **THEN** startup fails with an error naming `upstreams`

#### Scenario: Recursion table in forward mode
- **GIVEN** `mode = "forward"` and a `[recursion]` table
- **WHEN** the server starts
- **THEN** startup fails with an error naming `recursion`

### Requirement: Resolution from the root servers
In recursive mode a query that isn't blocked SHALL be resolved by asking the root servers, following delegations down to the authoritative servers, and returning the answer found there. No other resolver SHALL be asked to answer it.

#### Scenario: Resolved through a delegation chain
- **GIVEN** a root server that delegates `test` to a TLD server, which delegates `example.test` to a name server that answers `www.example.test`
- **WHEN** a client queries `www.example.test`
- **THEN** the client receives the name server's answer, and only those three servers were contacted

### Requirement: Root server hints
The root servers the resolution starts from SHALL be read at startup from a root hints file given by `recursion.roots`, defaulting to the file shipped with the image. A file that is missing, unreadable or lists no usable address SHALL fail startup with an error naming the file.

#### Scenario: Missing hints file
- **GIVEN** `recursion.roots` names a file that doesn't exist
- **WHEN** the server starts in recursive mode
- **THEN** startup fails with an error naming the file

### Requirement: Failure is visible
When a query that isn't blocked can't be resolved because no server answers in time, the server SHALL return SERVFAIL, SHALL count the failure in the metrics, and SHALL NOT return a blocked-style answer or ask any resolver outside the delegation chain.

#### Scenario: Name servers unreachable
- **GIVEN** the authoritative servers for a zone don't answer
- **WHEN** a client queries a name in that zone
- **THEN** the client receives SERVFAIL

### Requirement: DNSSEC validation
By default the server SHALL validate DNSSEC for the answers it resolves, using the built-in root trust anchor, and SHALL return SERVFAIL for an answer whose signatures are provably invalid. Answers from unsigned zones SHALL be returned normally. `recursion.dnssec` SHALL accept `validate` (the default) and `off`, and SHALL accept a path to a trust anchor file to use instead of the built-in one.

#### Scenario: Valid signature
- **GIVEN** a signed zone whose chain of trust leads to the trust anchor
- **WHEN** a client queries a name in it
- **THEN** the client receives the answer

#### Scenario: Tampered answer
- **GIVEN** a signed zone whose server returns an answer with an invalid signature
- **WHEN** a client queries that name
- **THEN** the client receives SERVFAIL

#### Scenario: Unsigned zone
- **GIVEN** a zone with no signatures and no secure delegation
- **WHEN** a client queries a name in it
- **THEN** the client receives the answer

#### Scenario: Validation off
- **GIVEN** `recursion.dnssec = "off"`
- **WHEN** a client queries a name in a zone whose signatures are invalid
- **THEN** the client receives the answer

### Requirement: Name minimization
By default each name server SHALL be sent only as much of the query name as it needs to see (QNAME minimization, strict). `recursion.qname_minimization` SHALL accept `strict` (the default) and `relaxed`.

#### Scenario: Minimized query
- **GIVEN** a root server and a TLD server observing the queries they receive
- **WHEN** a client queries `www.example.test`
- **THEN** the root server doesn't receive the full name and the TLD server receives at most `example.test`

### Requirement: Name server address filtering
The server SHALL NOT send queries to name servers at loopback, private, link-local, documentation, multicast or otherwise non-routable addresses, even when a delegation names one, so that a delegation can't make the server probe its own network.

#### Scenario: Delegation to a private address
- **GIVEN** a delegation whose only name server is at `10.0.0.1`
- **WHEN** a client queries a name in that zone
- **THEN** no query is sent to `10.0.0.1` and the client receives SERVFAIL

### Requirement: Opportunistic encryption
In recursive mode the server SHALL by default probe name servers for DNS-over-TLS and use it for the servers that accept it, and SHALL continue with plain queries for those that don't. A failed probe SHALL NOT make a client's query fail. `recursion.opportunistic_encryption` SHALL accept `true` (the default) and `false`. Queries sent over TLS SHALL be counted under the `tls` transport in the upstream metrics.

#### Scenario: Server accepts TLS
- **GIVEN** a name server that accepts DNS-over-TLS with a self-signed certificate
- **WHEN** a client queries a name it serves
- **THEN** later queries to that server use TLS, and the `tls` transport count increases

#### Scenario: Server doesn't accept TLS
- **GIVEN** a name server that doesn't accept TLS
- **WHEN** a client queries a name it serves
- **THEN** the client receives the answer over plain queries

#### Scenario: Turned off
- **GIVEN** `recursion.opportunistic_encryption = false`
- **WHEN** a client queries any name
- **THEN** no TLS connection is attempted to a name server

### Requirement: Encryption is not authenticated
Opportunistic encryption SHALL NOT verify the name server's certificate, and the documentation SHALL say so: it protects queries against passive observation only, for servers that accept TLS only, and an active attacker can force plain queries or intercept the connection. It SHALL NOT be described as verified or as equivalent to forwarding over verified DNS-over-TLS.

#### Scenario: Documentation
- **WHEN** the README's description of recursive mode is read
- **THEN** it states that the encryption is unauthenticated, protects against passive observers only, and covers only servers that accept TLS

### Requirement: Encryption state persistence
The server SHALL keep what it has learned about which name servers accept TLS in memory, and SHALL persist it across restarts only when `recursion.state_file` names a file. It SHALL NOT write that file or any other file otherwise.

#### Scenario: State file configured
- **GIVEN** `recursion.state_file` names a writable file and a server accepted TLS
- **WHEN** the server restarts
- **THEN** it uses TLS to that server without probing again

#### Scenario: No state file
- **GIVEN** no `recursion.state_file`
- **WHEN** the server runs
- **THEN** it writes no file

### Requirement: Filtering is unchanged
In recursive mode every filtering behaviour SHALL be the same as in forward mode: blocked names are answered by the server in the configured response mode without any resolution traffic, allow rules win over block rules, and a rule change applies to the next query even when its name is cached.

#### Scenario: Blocked name sends nothing
- **GIVEN** `block domain ads.example` and a server observing all its name servers' traffic
- **WHEN** a client queries `ads.example`
- **THEN** the client receives the blocked answer and no name server receives a query
