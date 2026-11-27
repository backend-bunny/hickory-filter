# Design

## Context

See proposal.md for the motivation. `templates/service.yaml` renders `type`, `selector` and the DNS and HTTP ports; `values.schema.json` closes the `service` object to `type`, `port` and `annotations`. Chart tests are golden renders (`tests/cases` → `tests/golden`), render-must-fail cases (`tests/fail`, each with the message it expects) and a kubeconform validation of the default render and of `ci/all-on-values.yaml`. The server already listens on `0.0.0.0` and `[::]`, so dual-stack needs no change in the binary: only a cluster with dual-stack pod networking and the Service fields.

## Goals / Non-Goals

**Goals:**
- Every field in the proposal can be set from values, validated by the schema, and is omitted unless set.
- The combinations the API server rejects fail at render time with a message that names the values.

**Non-Goals:**
- A free-form passthrough for arbitrary Service fields.
- Other resources' options (Deployment, NetworkPolicy and the rest are untouched).
- Choosing defaults for the user: nothing is turned on, including dual-stack, because the right values depend on the cluster.

## Decisions

### D1. Explicit values, not a passthrough
Each field is its own value with a schema entry: enums for the closed sets, a pattern-checked list for source ranges, bounds for the timeout.
- *Why:* a typo or an unknown enum value fails when the chart renders, with a message from the schema, instead of silently producing a Service the cluster rejects or ignores. The set is small and stable.
- *Alternative:* `service.extraSpec`, merged into the Service. More flexible, and it skips validation entirely; any field the list misses can be added later as one more explicit value.

### D2. Render only what is set
Each field sits behind `with`/`if`, so unset, empty string and empty list all mean "omit". The default render is byte-identical to today's, which the existing golden files check without change.

### D3. Render-time checks mirror the API server
The combinations in the spec are the ones the API server rejects for a Service; the chart fails them early with `fail` and a message that names the values (and, for type-dependent fields, the types they need), so the error appears at `helm template` and not at install. The chart doesn't try to duplicate every API rule (for example the immutability of `clusterIP`), only the ones that are cheap to state and a common mistake.

### D4. `clusterIP: None` is allowed
A headless Service is a legitimate choice, so the value is a plain string and `None` passes through. Whether a headless Service suits a given deployment is the user's call.

### D5. Cluster support is the cluster's to enforce
`trafficDistribution` needs a Kubernetes version that has the field (and zone labels on the nodes for the zone-aware values), and `PreferSameZone` and `PreferSameNode` need newer versions than `PreferClose`. The chart doesn't read the cluster version; the README says which fields need what.

## Risks / Trade-offs

- [`clusterIP` and the primary IP family are immutable on an existing Service, so changing them on upgrade fails in the cluster] → The README says so; the chart can't know the previous values.
- [`externalTrafficPolicy: Local` sends traffic only to nodes that have a pod, so some load-balancer health checks fail on the others] → That is the policy's purpose and the README states the trade (client addresses against even spreading).
- [`loadBalancerIP` is deprecated upstream and some clusters ignore it] → Kept because some clusters still use it; the README points to `loadBalancerClass` and annotations.
- [`trafficDistribution` does nothing with one replica] → The README says it only matters with more than one.
- [Schema checks and render-time checks drift from what the API server accepts as Kubernetes changes] → The kubeconform validation of the all-on render checks the field names against the Kubernetes schemas on every change.

## Migration Plan

Nothing to migrate: every field is off by default. Rolling back is removing the values.
