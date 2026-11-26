# Proposal

## Why

Operators who run hickory-filter want to see what it is doing without building a dashboard themselves: how many queries are answered, how many are blocked, how fast, whether the upstreams and the lists are healthy, and, when they choose to log queries, which clients and names are most active. The server exposes bounded Prometheus metrics and an opt-in structured query log (see `add-filtering-dns-forwarder`); a ready dashboard turns them into one view, and a test keeps the dashboard and the server from drifting apart.

## What Changes

- A **Grafana dashboard** shipped with the project, as one JSON file in the chart's `dashboards/` directory (the single source the chart's opt-in dashboard resources load).
- **Overview panels** from the Prometheus metrics: query rate by outcome, the percentage blocked, answer-time quantiles, queries by type, upstream and cache health, rule set size and last replacement, and a list health table.
- **Usage panels** from the query log in Loki: top blocked names, top permitted names, top clients and queries per client. They are empty unless the operator enables the query log, and say so.
- **Datasources are chosen when the dashboard is opened** (datasource variables for a Prometheus-compatible store and for Loki), so the same file works on any Grafana, provisioned by the operator custom resource, a ConfigMap sidecar, or an import.
- **A drift test:** every metric a panel queries is exported by the server, and every query-log field a panel uses is a documented field.

## Capabilities

### New Capabilities

- `grafana-dashboard`: what the dashboard shows, how it picks its datasources, and the guarantee that it only uses metrics and log fields the server actually provides.

### Modified Capabilities

None.

## Impact

- New files: `charts/hickory-filter/dashboards/hickory-filter.json`, a drift test, and the documentation of the panels.
- Depends on `add-filtering-dns-forwarder` (the metric names and query-log fields, tasks 8.2 to 8.4). The opt-in chart resources that load the file (a Grafana operator `GrafanaDashboard` and a sidecar ConfigMap) are part of `add-release-packaging`.
- No server code and no new runtime dependencies. Grafana, a Prometheus-compatible store and Loki are only needed by whoever uses the dashboard.
