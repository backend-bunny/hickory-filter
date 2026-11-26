# Tasks

## 1. Dashboard

- [x] 1.1 Create `charts/hickory-filter/dashboards/hickory-filter.json` with the datasource, instance and log stream variables and the overview panels from D3 (queries, blocked percentage, rate by outcome, answer-time quantiles, types, upstreams and cache, rule counts, last replacement); verify the file parses, has a fixed UID and title, and has no hard-coded datasource UIDs or import inputs
- [x] 1.2 Add the list health table (rules, skipped lines, age of the last success, last refresh result); verify it against a scrape of a server with a successful and a failed list
- [x] 1.3 Add the usage section from the query log (top blocked names, top permitted names, top clients, rate per client) with the note that it needs the query log; verify each query selects by the fixed target and uses only documented fields

## 2. Keeping it honest

- [x] 2.1 Write the drift test (D4): run a server through every event that yields a metric, scrape `/metrics`, and check every metric in the Prometheus panels is present and every field in the Loki panels is documented; verify it fails for a deliberately wrong metric name and a deliberately wrong field name in a scratch copy of the dashboard
- [x] 2.2 Document the dashboard (what each panel shows, the query log requirement, how it is loaded through the chart) in the chart README and README.md; verify every metric and field named there exists

## Workflow follow-up

- Start after `add-filtering-dns-forwarder` has its metrics and query log (tasks 8.2 to 8.4). The chart's opt-in dashboard resources are in `add-release-packaging`.
- Archive after the dashboard has been loaded into a Grafana and checked against a running server, and check the archived specs in `openspec/specs/`.
