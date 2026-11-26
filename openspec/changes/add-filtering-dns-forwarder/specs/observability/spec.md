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
`/metrics` SHALL expose, in Prometheus text format, counters of queries by outcome (`allowed` by an allow rule, `blocked`, `passed` with no match, `upstream_error`), and the time taken to answer.

#### Scenario: Blocked counter
- **WHEN** a client queries a blocked name once
- **THEN** the blocked-queries counter increases by one

### Requirement: List metrics
For each list, `/metrics` SHALL expose its rule count, skipped-line count, the time of its last successful load, and the result of its last refresh attempt. Each list SHALL be labelled by action, type and URL, with any credentials in the URL removed.

#### Scenario: Failed refresh visible
- **GIVEN** a list whose last refresh got HTTP 503
- **WHEN** `/metrics` is scraped
- **THEN** that list's last refresh result shows a failure, and its last success time is unchanged

### Requirement: Logging
The server SHALL log to stderr, at a configurable level (default `info`), in plain text or JSON. List loads and refreshes SHALL be logged with list, result, rule count and skipped lines. Individual queries SHALL be logged only at `debug` level, or when query logging is enabled.

#### Scenario: Refresh failure logged
- **WHEN** a list refresh fails
- **THEN** a warning is logged with the list URL (credentials removed) and the reason

#### Scenario: No per-query logs by default
- **WHEN** clients send queries at the default log level
- **THEN** no per-query lines are logged
