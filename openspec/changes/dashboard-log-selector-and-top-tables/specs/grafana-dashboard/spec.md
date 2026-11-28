# Spec Delta

## MODIFIED Requirements

### Requirement: Usage from the query log
The dashboard SHALL show, from the query log in Loki, the most requested blocked names, the most requested permitted names, the busiest clients and the query rate per client, and SHALL state that these panels are empty unless the query log is enabled. The most requested blocked names, the most requested permitted names and the busiest clients SHALL each be a table with one row per name or client and its query count, sorted by the count, highest first.

#### Scenario: Usage panels use only query lines
- **WHEN** the usage panels' queries are inspected
- **THEN** each selects lines by the fixed query-log target and uses only documented query-log fields

#### Scenario: Rankings are tables of names and counts
- **WHEN** the most requested blocked names, the most requested permitted names and the busiest clients panels are inspected
- **THEN** each is a table that shows the name or client and its query count in one row per name or client, sorted by count, without reducing its rows
