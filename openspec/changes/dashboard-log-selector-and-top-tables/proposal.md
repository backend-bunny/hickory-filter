# Proposal

## Why

Two things keep the dashboard's usage section from working out of the box.

The usage panels find the server's log lines through the **Log stream selector** variable, whose packaged default is `{app_kubernetes_io_name="hickory-filter"}`. Log collectors label container streams differently: some index them by a pod label, others by the workload's service name or container. Where the default label doesn't exist, every usage panel is empty, and the chart has no way to change the default short of patching the dashboard JSON inside the rendered resource.

The three rankings (most requested blocked names, most requested permitted names, busiest clients) are bar gauges at a fixed height. With ten series they don't fit, so a viewer sees a single bar and a number instead of a ranked list of names, and long domain names are cut.

## What Changes

- The three rankings become **tables**: one row per name or client with its query count, sorted by count, the count drawn as a gauge cell. The panels are taller so ten rows fit, and the panels below move down by the added height.
- New chart value `dashboard.logSelector` (empty by default): when set, it replaces the packaged default of the Log stream selector in the dashboard carried by both the operator resource and the sidecar ConfigMap. A render without it is unchanged.
- The render fails, naming the value, if the value is set but the packaged dashboard no longer contains the default it replaces, so a dashboard edit can't silently drop the override.
- READMEs document the value and the tables.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `grafana-dashboard`: the usage rankings are tables of names or clients with their counts.
- `helm-chart`: the dashboard resources take a default log stream selector from values.

## Impact

- Files: `charts/hickory-filter/dashboards/hickory-filter.json`, `templates/_helpers.tpl`, `templates/grafanadashboard.yaml`, `templates/dashboard-configmap.yaml`, `values.yaml`, `values.schema.json`, the chart README and the project README, golden renders and a new case under `charts/hickory-filter/tests/`, and `tests/dashboard.rs`. No server code and no image change.
- Behaviour: the dashboard shows tables in place of the three bar gauges; with default values the rendered resources differ only in that dashboard JSON.
- Operations: a dashboard already imported keeps its layout until it is re-imported or the resource is updated; the operator and the sidecar loader update it from the new release.
