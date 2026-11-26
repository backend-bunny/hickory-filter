# Spec Delta

## Purpose

Provides a Helm chart that deploys hickory-filter to Kubernetes with secure defaults, published as a versioned OCI artifact that pins the image it was released with.

## ADDED Requirements

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
- **THEN** the container image is `<registry>/<owner>/hickory-filter@<digest of release X.Y.Z>`

### Requirement: Chart versioning
The chart's `version` and `appVersion` SHALL both equal the release version `X.Y.Z`.

#### Scenario: Versions match
- **WHEN** the chart for release `v1.2.3` is inspected
- **THEN** `version` and `appVersion` are both `1.2.3`

### Requirement: Published as an OCI artifact
The packaged chart SHALL be pushed to an OCI registry at `oci://<registry>/<owner>/charts/hickory-filter`, installable with `helm install` from that reference and version.

#### Scenario: Install from OCI
- **WHEN** a user runs `helm install dns oci://<registry>/<owner>/charts/hickory-filter --version X.Y.Z`
- **THEN** the release installs with the pinned image

### Requirement: Chart validated in CI
Every change SHALL lint the chart and validate its rendered manifests against the Kubernetes API schemas, for the default values and for a values file that enables every optional resource.

#### Scenario: Invalid manifest
- **WHEN** a template change renders an invalid field
- **THEN** CI fails before anything is published
