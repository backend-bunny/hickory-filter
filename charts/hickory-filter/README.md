# hickory-filter Helm chart

Deploys the filtering DNS forwarder. The pod meets the `restricted` Pod
Security Standard: it runs as uid/gid 65532, drops all capabilities, has a
read-only root filesystem, and listens on port 1053 (the Service maps port 53
to it, UDP and TCP).

## Install

```sh
helm install dns oci://ghcr.io/backend-bunny/charts/hickory-filter --version X.Y.Z \
  --set 'config.upstreams={192.0.2.53}'
```

A released chart's default image is `ghcr.io/backend-bunny/hickory-filter@sha256:…`,
the digest published in the same release. `image.digest` wins over `image.tag`.

## Verify

The chart and the image are signed keylessly with the release workflow's
identity. Check the chart digest before installing:

```sh
cosign verify ghcr.io/backend-bunny/charts/hickory-filter@sha256:<digest> \
  --certificate-identity-regexp '^https://github.com/backend-bunny/hickory-filter/\.github/workflows/' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

## Configuration

`config` is the configuration file as values and is rendered to TOML, so any key of the
file's reference works there (the schema rejects unknown ones). A change rolls the pods
through a checksum annotation. `existingConfigMap` uses a ConfigMap you manage instead;
`config.http` must then still match the HTTP address in it.

| Value | Default | Meaning |
|---|---|---|
| `replicaCount` | `1` | Number of pods |
| `image.repository` | `ghcr.io/backend-bunny/hickory-filter` | Image repository |
| `image.digest` | `""` | Image digest; takes precedence over the tag |
| `image.tag` | `""` | Image tag; the chart's app version when empty |
| `image.pullPolicy` | `IfNotPresent` | Pull policy |
| `config` | upstreams `1.1.1.1`, `9.9.9.9`; HTTP on 8080 | The configuration as values |
| `existingConfigMap` | `""` | Use this ConfigMap for the configuration |
| `existingConfigMapKey` | `config.toml` | Its key |
| `cache.enabled` | `false` | Mount a writable volume and set `filter.cache_dir` |
| `cache.existingClaim` | `""` | Use this PersistentVolumeClaim instead of an emptyDir |
| `service.type` | `ClusterIP` | Service type |
| `service.port` | `53` | DNS port of the Service |
| `service.annotations` | `{}` | Service annotations |
| `podLabels` | `{}` | Extra pod labels |
| `podAnnotations` | `{}` | Extra pod annotations; they win over generated ones |
| `podDisruptionBudget.enabled` | `false` | Render a PodDisruptionBudget |
| `networkPolicy.enabled` | `false` | Render a NetworkPolicy for DNS and the HTTP port |
| `scrapeAnnotations.enabled` | `false` | `prometheus.io/*` pod annotations, port from `config.http` |
| `serviceMonitor.enabled` | `false` | Prometheus Operator ServiceMonitor |
| `podMonitor.enabled` | `false` | Prometheus Operator PodMonitor |
| `grafanaDashboard.enabled` | `false` | `GrafanaDashboard` for the Grafana operator |
| `grafanaDashboard.instanceSelector` | `{}` | Required when enabled |
| `dashboardConfigMap.enabled` | `false` | Dashboard ConfigMap for the Grafana sidecar |
| `dashboardConfigMap.labels` | `grafana_dashboard: "1"` | The sidecar's label |

The monitors share `interval`, `scrapeTimeout`, `labels`, `relabelings` and
`metricRelabelings`. Enabling a monitor or the scrape annotations while `config.http`
is `false` fails the render.

## Examples

Scrape through a Prometheus Operator monitor (the labels match the operator's selector):

```yaml
serviceMonitor:
  enabled: true
  interval: 30s
  labels:
    release: monitoring
```

Scrape through annotations, for a telemetry collector that discovers pods by them. Do not
enable this together with a monitor, or a collector honouring both scrapes the pod twice:

```yaml
scrapeAnnotations:
  enabled: true
```

Load the dashboard through the Grafana operator, or through the sidecar:

```yaml
grafanaDashboard:
  enabled: true
  instanceSelector:
    matchLabels:
      dashboards: grafana
# or
dashboardConfigMap:
  enabled: true
```

The usage panels need the query log: set `config.log.query_log: true`.
