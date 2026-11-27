# helm-chart Specification

## Purpose
Provides a Helm chart that deploys hickory-filter to Kubernetes with secure defaults, published as a versioned OCI artifact that pins the image it was released with.

## Requirements

### Requirement: Chart resources
The chart SHALL render a Deployment, a Service exposing DNS on port 53 (UDP and TCP) mapped to the container's port 1053, a ConfigMap with the hickory-filter configuration, and a ServiceAccount. A PodDisruptionBudget, a NetworkPolicy and a Prometheus `ServiceMonitor` SHALL be available as opt-in values.

#### Scenario: Default render
- **WHEN** the chart is rendered with default values
- **THEN** the output contains the Deployment, Service (53/UDP and 53/TCP → 1053), ConfigMap and ServiceAccount, and no ServiceMonitor

#### Scenario: ServiceMonitor opt-in
- **WHEN** the chart is rendered with the ServiceMonitor enabled
- **THEN** a ServiceMonitor scraping `/metrics` on the HTTP port is included

### Requirement: Restricted Pod Security by default
With default values, the pod SHALL satisfy the Kubernetes `restricted` Pod Security Standard: `runAsNonRoot`, numeric `runAsUser`/`runAsGroup` 65532, `allowPrivilegeEscalation: false`, all capabilities dropped, `seccompProfile: RuntimeDefault`, and `readOnlyRootFilesystem: true`.

#### Scenario: Admitted under restricted
- **WHEN** the default render is applied to a namespace enforcing `restricted`
- **THEN** the pods are admitted and become ready

### Requirement: Probes use the HTTP endpoints
The Deployment SHALL use an `httpGet` liveness probe on `/health` and an `httpGet` readiness probe on `/ready`. No probe SHALL rely on a shell.

#### Scenario: Not ready until lists load
- **GIVEN** a large remote list still being fetched at startup
- **WHEN** the pod starts
- **THEN** it isn't added to the Service endpoints until `/ready` returns 200

### Requirement: Configuration through values
The hickory-filter configuration SHALL be settable from values, either as structured values rendered into the ConfigMap or as an existing ConfigMap name. A change to the rendered configuration SHALL roll the pods.

#### Scenario: Config change rolls pods
- **WHEN** an upgrade changes a rule in the values
- **THEN** the pod template's config checksum annotation changes and the Deployment rolls out

### Requirement: Optional list cache volume
When the list cache is enabled, the chart SHALL mount a writable volume at the configured cache directory: an `emptyDir` by default, or an existing PersistentVolumeClaim. With the cache disabled, no writable volume except `/tmp` SHALL be mounted.

#### Scenario: Cache enabled
- **WHEN** the cache is enabled with defaults
- **THEN** an `emptyDir` is mounted at the cache directory and the config's `cache_dir` points at it

### Requirement: Released chart pins the image digest
A released chart's default image reference SHALL be the digest published in the same release (`repository@sha256:…`). Users SHALL be able to override repository, tag and digest. When the digest is set, it SHALL take precedence over the tag.

#### Scenario: Default image of a release
- **WHEN** the released chart version `X.Y.Z` is rendered with default values
- **THEN** the container image is `ghcr.io/backend-bunny/hickory-filter@<digest of release X.Y.Z>`

### Requirement: Chart versioning
The chart's `version` and `appVersion` SHALL both equal the release version `X.Y.Z`.

#### Scenario: Versions match
- **WHEN** the chart for release `v1.2.3` is inspected
- **THEN** `version` and `appVersion` are both `1.2.3`

### Requirement: Published as an OCI artifact
The packaged chart SHALL be pushed to an OCI registry at `oci://ghcr.io/backend-bunny/charts/hickory-filter`, installable with `helm install` from that reference and version.

#### Scenario: Install from OCI
- **WHEN** a user runs `helm install dns oci://ghcr.io/backend-bunny/charts/hickory-filter --version X.Y.Z`
- **THEN** the release installs with the pinned image

### Requirement: Standard labels and extra metadata
Every resource the chart renders SHALL carry the recommended Kubernetes labels (`app.kubernetes.io/name`, `app.kubernetes.io/instance`, `app.kubernetes.io/version` and `app.kubernetes.io/managed-by`). Values SHALL allow extra labels and annotations on the pods (`podLabels`, `podAnnotations`) and annotations on the Service, so that scraping, log and telemetry collectors that discover workloads by label or annotation can find them.

#### Scenario: Recommended labels
- **WHEN** the chart is rendered with default values
- **THEN** every resource has the four recommended labels, with the chart's app version as `version`

#### Scenario: Pod annotations for a collector
- **GIVEN** `podAnnotations` and `podLabels` are set
- **WHEN** the chart is rendered
- **THEN** the pod template carries them, next to the config checksum annotation, which is unchanged

### Requirement: Metrics scraping resources
The chart SHALL name the HTTP port on the pod and the Service. It SHALL offer, each off by default, a Prometheus Operator `ServiceMonitor` and, as an alternative that needs no Service, a `PodMonitor`, each scraping `/metrics` with a configurable interval and scrape timeout, extra labels (so an operator's selector matches the resource) and relabelings and metric relabelings. Enabling either with the HTTP endpoint disabled in the configuration SHALL fail the render with a message saying so.

#### Scenario: PodMonitor opt-in
- **WHEN** the chart is rendered with the PodMonitor enabled and an interval of 30 seconds
- **THEN** a PodMonitor selecting the chart's pods and scraping `/metrics` on the named HTTP port every 30 seconds is included

#### Scenario: Monitor labels
- **GIVEN** `serviceMonitor.labels` sets `release: monitoring`
- **WHEN** the chart is rendered with the ServiceMonitor enabled
- **THEN** the ServiceMonitor carries that label

#### Scenario: Endpoint disabled
- **WHEN** a monitor is enabled and the configuration disables the HTTP endpoint
- **THEN** rendering fails with a message that the metrics endpoint is disabled

### Requirement: Scrape annotations for annotation-based discovery
The chart SHALL offer, off by default, the conventional scrape annotations on the pod template (`prometheus.io/scrape: "true"`, `prometheus.io/port` and `prometheus.io/path: "/metrics"`) for collectors that discover pods by annotation. The port SHALL be derived from the HTTP address in the chart's configuration, so it can't drift from the port the server listens on.

#### Scenario: Default port
- **WHEN** the chart is rendered with the scrape annotations enabled and the default HTTP address
- **THEN** the pod template's annotations include `prometheus.io/scrape: "true"`, `prometheus.io/port: "8080"` and `prometheus.io/path: "/metrics"`

#### Scenario: Port follows the configuration
- **GIVEN** the configuration sets the HTTP address to port 9090
- **WHEN** the chart is rendered with the scrape annotations enabled
- **THEN** the annotation `prometheus.io/port` is `"9090"`

#### Scenario: Off by default
- **WHEN** the chart is rendered with default values
- **THEN** the pod template has no `prometheus.io/*` annotations

### Requirement: Scrape annotations can be overridden and need the endpoint
An annotation set explicitly in `podAnnotations` SHALL take precedence over the generated one, and enabling the scrape annotations with the HTTP endpoint disabled in the configuration SHALL fail the render with a message saying so.

#### Scenario: Explicit annotation wins
- **GIVEN** the scrape annotations are enabled and `podAnnotations` sets `prometheus.io/path` to `/custom`
- **WHEN** the chart is rendered
- **THEN** the pod template has `prometheus.io/path: /custom` and the other two generated annotations

#### Scenario: Endpoint disabled
- **WHEN** the scrape annotations are enabled and the configuration disables the HTTP endpoint
- **THEN** rendering fails with a message that the metrics endpoint is disabled

### Requirement: Optional Grafana dashboard resources
The chart SHALL offer, each off by default, a `GrafanaDashboard` custom resource for the Grafana operator (`grafana.integreatly.org`) and a ConfigMap labelled for the Grafana sidecar dashboard loader, each carrying the dashboard file packaged in the chart's `dashboards/` directory. The operator resource SHALL take its instance selector and folder from values, and the ConfigMap's label and namespace SHALL be configurable.

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

### Requirement: Chart validated in CI
Every change SHALL lint the chart and validate its rendered manifests against the Kubernetes API schemas, for the default values and for a values file that enables every optional resource.

#### Scenario: Invalid manifest
- **WHEN** a template change renders an invalid field
- **THEN** CI fails before anything is published

### Requirement: Recursive mode values
The chart's values schema SHALL accept the configuration's `mode` and `recursion` settings, with forward mode and its upstreams as the default. A render with `mode` set to `recursive` and no upstreams SHALL be valid. An enabled NetworkPolicy SHALL NOT restrict the pods' egress in recursive mode, since the server must reach arbitrary hosts on UDP and TCP port 53 and TCP port 853.

#### Scenario: Default stays forward
- **WHEN** the chart is rendered with default values
- **THEN** the configuration has upstreams and no `mode` other than `forward`

#### Scenario: Recursive values
- **GIVEN** values with `config.mode` set to `recursive` and no upstreams
- **WHEN** the chart is rendered
- **THEN** the render is valid and the ConfigMap holds `mode = "recursive"`

#### Scenario: NetworkPolicy and egress
- **WHEN** the chart is rendered with the NetworkPolicy enabled and recursive mode
- **THEN** the NetworkPolicy has no egress rules and doesn't select the egress policy type
