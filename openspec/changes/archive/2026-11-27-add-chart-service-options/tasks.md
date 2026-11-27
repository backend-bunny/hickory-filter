# Tasks

## 1. Values and schema

- [x] 1.1 Add the new `service` values to `values.yaml` (all unset by default, each with a comment) and to `values.schema.json`: enums for `ipFamilyPolicy`, `ipFamilies` items, `trafficDistribution`, `externalTrafficPolicy`, `internalTrafficPolicy` and `sessionAffinity`, at most two `ipFamilies`, CIDR-shaped `loadBalancerSourceRanges` items, a bounded `sessionAffinityTimeoutSeconds`; verify `helm lint` and the schema validation of the default render pass and that a fail case with an unknown `trafficDistribution` is rejected with the schema's message

## 2. Template and render-time checks

- [x] 2.1 Render each field in `templates/service.yaml` only when set; verify the default render is unchanged (the existing golden files pass untouched) and that each field appears in a golden render when set
- [x] 2.2 Fail the render, with messages that name the values, for external traffic policy on a type without external traffic, load balancer fields on a type other than `LoadBalancer`, two IP families with `SingleStack`, and a session affinity timeout without client-IP affinity; verify one fail case per check in `tests/fail`, each with the expected message

## 3. Tests

- [x] 3.1 Add golden render cases for dual-stack, the traffic fields, a load balancer with `Local` external traffic policy, class and source ranges, and client-IP session affinity; extend `ci/all-on-values.yaml` with the fields that fit a `ClusterIP` Service; verify `hack/chart-test.sh` passes, including the kubeconform validation of the all-on render, and that removing a field from the template fails its golden

## 4. Documentation

- [x] 4.1 Add a row per value to the chart README's values table and a short section with the dual-stack example and the client-address example, naming the Kubernetes versions the fields need, that `clusterIP` and the primary IP family are immutable, and that `trafficDistribution` matters only with more than one replica; verify the README check (every value named in it exists in the schema) passes and the policy check passes

## Workflow follow-up

- Released through the release PR (a `feat(chart)` commit makes a minor release); verify the published chart renders the new fields.
- Archive after the release verifies, and check the main `helm-chart` spec.
