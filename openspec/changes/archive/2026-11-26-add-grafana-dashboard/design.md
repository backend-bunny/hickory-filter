# Design

## Context

The server exposes aggregate Prometheus metrics with bounded labels and an opt-in structured query log (design D7 of `add-filtering-dns-forwarder`). Per-client and per-name views deliberately don't exist as metrics: their label values are unbounded and personal. The operators this is for already run a Prometheus-compatible store and Loki, so those are the two datasources.

## Goals / Non-Goals

**Goals:**
- One dashboard file that works wherever it is loaded, with the datasources picked at runtime.
- Overview panels that need only Prometheus, and usage panels that need the query log in Loki.
- A test that fails when the dashboard asks for a metric or a log field the server doesn't provide.

**Non-Goals:**
- Alerting and recording rules (a later change, if wanted).
- Per-client friendly names or any enrichment beyond what the query log carries.
- Dashboards for other tools, or provisioning outside the chart.

## Decisions

### D1. Location: the chart's `dashboards/` directory
The canonical file is `charts/hickory-filter/dashboards/hickory-filter.json`. Helm can only read files inside the chart (`.Files.Get`), so keeping the single source there avoids a copy step and lets the opt-in `GrafanaDashboard` resource and the sidecar ConfigMap (in `add-release-packaging`) embed it directly.

### D2. Datasource variables, not import inputs
The dashboard defines two template variables of type `datasource`, one for Prometheus and one for Loki, and panels refer to them. It has no `__inputs` section and no hard-coded datasource UIDs, so provisioning through the operator resource, a ConfigMap or a manual import all work without substitution. Further variables select the instance (a label of the scraped job) and the Loki stream selector (a variable holding the stream labels that identify hickory-filter's log lines, with a default for the common pod-label mapping), because how container logs are labelled differs between collectors.

### D3. Panels
- *Overview (Prometheus):* total queries and the percentage blocked over the selected range; query rate by outcome; answer-time quantiles from the histogram; queries by type; upstream queries and timeouts by protocol; cache hit ratio; rules by action and kind; replacements and time since the last one; and a table of lists with rule count, skipped lines, age of the last success and the last refresh result.
- *Usage (Loki):* top blocked names, top permitted names, top clients, and query rate per client, each computed from the query log lines selected by their fixed target (`| json`, then filtering and `topk(…)` over `count_over_time` or `rate`). A text panel at the top of the section says these need the query log enabled and shows the setting.

### D4. Drift test
A test parses the dashboard JSON, extracts the metric names from the Prometheus panel expressions and the field names from the Loki expressions, runs a server through the events that produce every metric (blocked, allowed, passed, cached, failed-upstream queries; a changed and an unchanged refresh; a failed refresh), scrapes `/metrics`, and fails if a panel uses a metric the scrape doesn't contain or a log field that isn't in the documented list. It also checks the file is valid JSON with the expected top-level structure and a stable UID and title.

## Risks / Trade-offs

- [Loki panels are empty without the query log] → The section says so, and the overview works without it.
- [Log labelling differs between collectors] → The stream selector is a variable with a default, not hard-coded.
- [Grafana's JSON schema changes between versions] → Keep to long-stable panel types (stat, timeseries, table, bargauge, text) and set `schemaVersion` to a conservative value.
- [Dashboard JSON is verbose and easy to break by hand] → Edited through Grafana and exported, with the drift test as the guard.

## Open Questions

- Whether to provide alert rules alongside the dashboard (list refresh failing, upstream failures, not ready). Out of scope here.
