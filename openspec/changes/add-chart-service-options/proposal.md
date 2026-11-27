# Proposal

## Why

The chart's Service sets only its type, port and annotations, so a deployment can't ask for dual-stack addressing, keep client source addresses behind a load balancer, prefer nearby replicas, pin the cluster IP or restrict a load balancer. Each of those needs a field of the Service's `spec` that the chart never renders, and the values schema closes the `service` block, so there is no way to set them short of patching the rendered manifests. For a DNS service some of these matter beyond routing: with the default external traffic policy, clients behind a load balancer all appear as a node address, which makes the query log's per-client views and the dashboard's busiest-clients panels meaningless.

## What Changes

- New optional `service` values, each off by default so a render without them is identical to today's: `clusterIP`, `ipFamilyPolicy` and `ipFamilies` (dual-stack), `trafficDistribution`, `externalTrafficPolicy`, `internalTrafficPolicy`, `loadBalancerClass`, `loadBalancerSourceRanges`, `loadBalancerIP` (deprecated upstream, kept for clusters that still honour it), and `sessionAffinity` with `sessionAffinityTimeoutSeconds`.
- Explicit values with schema validation, not a free-form passthrough, so a typo or a bad value fails at render time.
- Render-time checks, with messages that name the value, for combinations the API server would reject: external traffic policy on a type that has no external traffic, load balancer fields on any other type, two IP families with a single-stack policy, a session affinity timeout without client-IP affinity.
- The chart README documents every value and shows a dual-stack example and the client-address example.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `helm-chart`: the Service takes the options above, and the chart validates them.

## Impact

- Files: `charts/hickory-filter/templates/service.yaml`, `values.yaml`, `values.schema.json`, the chart README, golden renders and fail cases under `charts/hickory-filter/tests/`, and `ci/all-on-values.yaml`. No Rust code and no image change.
- Behaviour: none by default; every field is omitted unless set.
- Operations: fields the cluster doesn't know (`trafficDistribution` on older Kubernetes versions) are rejected by that cluster, not by the chart; the README says which fields need which Kubernetes version. Some fields are immutable on an existing Service (`clusterIP`, and the primary IP family), so changing them on upgrade can fail in the cluster; the README says so.
