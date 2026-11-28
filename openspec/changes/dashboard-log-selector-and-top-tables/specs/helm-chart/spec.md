# Spec Delta

## MODIFIED Requirements

### Requirement: Optional Grafana dashboard resources
The chart SHALL offer, each off by default, a `GrafanaDashboard` custom resource for the Grafana operator (`grafana.integreatly.org`) and a ConfigMap labelled for the Grafana sidecar dashboard loader, each carrying the dashboard file packaged in the chart's `dashboards/` directory. The operator resource SHALL take its instance selector and folder from values, and the ConfigMap's label and namespace SHALL be configurable. When `dashboard.logSelector` is set, both resources SHALL carry the dashboard with that value as the default of its log stream selector; when it is empty, the packaged dashboard SHALL be carried unchanged.

#### Scenario: Operator resource
- **GIVEN** the operator resource is enabled with an instance selector
- **WHEN** the chart is rendered
- **THEN** a `GrafanaDashboard` with that instance selector and the packaged dashboard JSON is included

#### Scenario: Sidecar ConfigMap
- **GIVEN** the dashboard ConfigMap is enabled with the label `grafana_dashboard: "1"`
- **WHEN** the chart is rendered
- **THEN** a ConfigMap with that label holding the packaged dashboard JSON is included

#### Scenario: Off by default
- **WHEN** the chart is rendered with default values
- **THEN** no `GrafanaDashboard` and no dashboard ConfigMap is rendered

#### Scenario: Log stream selector default
- **GIVEN** both dashboard resources are enabled and `dashboard.logSelector` is `{service_name="hickory-filter"}`
- **WHEN** the chart is rendered
- **THEN** in both resources the dashboard's log stream selector defaults to `{service_name="hickory-filter"}`, the packaged default appears nowhere, and the dashboard is valid JSON
