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
| `service.clusterIP` | unset | Pin the Service's cluster IP; `None` makes it headless |
| `service.ipFamilyPolicy` | unset | `SingleStack`, `PreferDualStack` or `RequireDualStack` |
| `service.ipFamilies` | unset | `IPv4` and `IPv6`, at most two, in order; two need a dual-stack policy |
| `service.trafficDistribution` | unset | `PreferClose`, `PreferSameZone` or `PreferSameNode` |
| `service.externalTrafficPolicy` | unset | `Cluster` or `Local`; only for `NodePort` and `LoadBalancer` |
| `service.internalTrafficPolicy` | unset | `Cluster` or `Local` |
| `service.loadBalancerClass` | unset | Which load balancer implementation to use; `LoadBalancer` only |
| `service.loadBalancerSourceRanges` | unset | CIDRs allowed to reach the load balancer; `LoadBalancer` only |
| `service.loadBalancerIP` | unset | Requested load balancer address; `LoadBalancer` only, deprecated upstream |
| `service.sessionAffinity` | unset | `None` or `ClientIP` |
| `service.sessionAffinityTimeoutSeconds` | unset | Client-IP affinity timeout (1 to 86400); needs `sessionAffinity: ClientIP` |
| `podLabels` | `{}` | Extra pod labels |
| `podAnnotations` | `{}` | Extra pod annotations; they win over generated ones |
| `podDisruptionBudget.enabled` | `false` | Render a PodDisruptionBudget |
| `networkPolicy.enabled` | `false` | Render a NetworkPolicy for DNS and the HTTP port |
| `scrapeAnnotations.enabled` | `false` | `prometheus.io/*` pod annotations, port from `config.http` |
| `serviceMonitor.enabled` | `false` | Prometheus Operator ServiceMonitor |
| `podMonitor.enabled` | `false` | Prometheus Operator PodMonitor |
| `dashboard.logSelector` | `""` | Default Loki stream selector of the dashboard's usage panels; empty keeps `{app_kubernetes_io_name="hickory-filter"}` |
| `grafanaDashboard.enabled` | `false` | `GrafanaDashboard` for the Grafana operator |
| `grafanaDashboard.instanceSelector` | `{}` | Required when enabled |
| `dashboardConfigMap.enabled` | `false` | Dashboard ConfigMap for the Grafana sidecar |
| `dashboardConfigMap.labels` | `grafana_dashboard: "1"` | The sidecar's label |

The monitors share `interval`, `scrapeTimeout`, `labels`, `relabelings` and
`metricRelabelings`. Enabling a monitor or the scrape annotations while `config.http`
is `false` fails the render.

## Service options

Every Service field above is left out of the Service unless you set it, so the defaults
render exactly what older versions of the chart did. A combination the API server would
reject fails when the chart renders, with a message that names the values (for example
`externalTrafficPolicy` on a `ClusterIP` Service, a load balancer option on another type,
two IP families without a dual-stack policy).

Dual-stack (the server already listens on IPv4 and IPv6; the cluster needs dual-stack pod
networking):

```yaml
service:
  ipFamilyPolicy: PreferDualStack
  ipFamilies: [IPv4, IPv6]
```

Keep client addresses behind a load balancer. With the default external traffic policy every
client looks like a node address, which makes the query log's `client` field and the
dashboard's busiest-clients panels useless; `Local` keeps the real source address, at the
price of sending traffic only to nodes that have a pod:

```yaml
service:
  type: LoadBalancer
  externalTrafficPolicy: Local
  loadBalancerSourceRanges: [198.51.100.0/24]
```

Things to know:

- `clusterIP` and the primary IP family cannot be changed on an existing Service, so changing
  them in an upgrade fails in the cluster, not in the chart.
- `trafficDistribution` only matters with more than one replica, needs a Kubernetes version that
  has the field (it is recent, and `PreferSameZone` and `PreferSameNode` arrived after
  `PreferClose`), and the zone-aware values need nodes labelled with their zone. The cluster, not
  the chart, rejects a field it doesn't know.
- `loadBalancerIP` is deprecated upstream and some clusters ignore it; prefer
  `loadBalancerClass` or your provider's annotations (`service.annotations`).

## Recursive mode

By default the server forwards to `config.upstreams`. With `mode: recursive` it resolves
names itself, from the root servers, so no third-party resolver sees your queries. The chart
leaves `upstreams` out of the rendered configuration in that mode, so this is enough:

```yaml
config:
  mode: recursive
```

The pods then need to reach arbitrary hosts on UDP and TCP port 53 (and TCP port 853, where
name servers accept TLS); the chart restricts no egress, and an enabled NetworkPolicy
selects ingress only. A pod becomes ready once a root server answers, so a network that
blocks DNS shows up as a pod that never becomes ready. DNSSEC validation, QNAME minimization
and opportunistic encryption are on by default (see the project README for what each does and
doesn't give); change them under `config.recursion`. To keep what was learned about TLS
support across restarts, enable the cache volume and point the state file into it:

```yaml
cache:
  enabled: true
config:
  mode: recursive
  recursion:
    state_file: /var/cache/hickory-filter/tls-state.toml
```

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

The usage panels find the server's log lines through the dashboard's **Log stream
selector**, by default `{app_kubernetes_io_name="hickory-filter"}`. Log collectors
label container streams differently; if yours indexes them by another label, set the
default that matches (it applies to both dashboard resources):

```yaml
dashboard:
  logSelector: '{service_name="hickory-filter"}'
```
