# Spec Delta

## Purpose

Gives operators a ready Grafana dashboard for hickory-filter, built only on the metrics and query-log fields the server provides, and loadable on any Grafana without editing.

## ADDED Requirements

### Requirement: Dashboard ships with the project
The project SHALL ship one Grafana dashboard definition as a JSON file in the Helm chart's `dashboards/` directory, with a fixed UID and title, so it can be provisioned or imported without changes.

#### Scenario: Valid dashboard file
- **WHEN** the dashboard file is parsed
- **THEN** it is valid JSON with a title, a fixed UID, and at least one panel

### Requirement: Datasources are chosen at runtime
The dashboard SHALL select its Prometheus-compatible and Loki datasources through datasource template variables, with no hard-coded datasource UIDs and no import inputs, and SHALL let the user select the instance and the log stream selector.

#### Scenario: No hard-coded datasources
- **WHEN** the dashboard file is inspected
- **THEN** every panel's datasource refers to a datasource variable, and the file has no import inputs

### Requirement: Overview from metrics
The dashboard SHALL show, from the server's Prometheus metrics: total queries and the percentage blocked, the query rate by outcome, answer-time quantiles, queries by type, upstream queries and failures, the response cache hit ratio, the number of rules by action and kind, and when the rule set was last replaced. It SHALL show a table of lists with each list's rule count, skipped lines, time since its last success, and last refresh result.

#### Scenario: Overview needs only metrics
- **GIVEN** the query log is disabled
- **WHEN** the dashboard is opened
- **THEN** every overview panel is backed by Prometheus metrics and none by Loki

#### Scenario: Failed list visible
- **GIVEN** a list whose last refresh failed
- **WHEN** the list table is viewed
- **THEN** that list shows a failed last result and the age of its last success

### Requirement: Usage from the query log
The dashboard SHALL show, from the query log in Loki, the most requested blocked names, the most requested permitted names, the busiest clients and the query rate per client, and SHALL state that these panels are empty unless the query log is enabled.

#### Scenario: Usage panels use only query lines
- **WHEN** the usage panels' queries are inspected
- **THEN** each selects lines by the fixed query-log target and uses only documented query-log fields

### Requirement: Only what the server provides
Every metric used by a panel SHALL be exported by the server, and every query-log field used by a panel SHALL be a documented query-log field. A test SHALL fail when this stops being true.

#### Scenario: Unknown metric
- **WHEN** a panel is changed to query a metric the server does not export
- **THEN** the drift test fails and names the metric

#### Scenario: Unknown log field
- **WHEN** a Loki panel is changed to use a field that is not a documented query-log field
- **THEN** the drift test fails and names the field
