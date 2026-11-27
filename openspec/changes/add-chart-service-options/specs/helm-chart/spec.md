# Spec Delta

## ADDED Requirements

### Requirement: Service options are opt-in
Every Service option below SHALL be omitted from the rendered Service unless its value is set, so a render without them is unchanged.

#### Scenario: Default render unchanged
- **WHEN** the chart is rendered with default values
- **THEN** the Service has none of the fields named in the Service option requirements

### Requirement: Dual-stack Service
The chart SHALL take `service.ipFamilyPolicy` (`SingleStack`, `PreferDualStack`, `RequireDualStack`) and `service.ipFamilies` (`IPv4` and `IPv6`, at most two, in order) and render them in the Service spec.

#### Scenario: Dual-stack
- **GIVEN** `service.ipFamilyPolicy` is `PreferDualStack` and `service.ipFamilies` is `[IPv4, IPv6]`
- **WHEN** the chart is rendered
- **THEN** the Service spec carries that policy and those families, in that order

### Requirement: Traffic routing options
The chart SHALL take `service.trafficDistribution` (`PreferClose`, `PreferSameZone`, `PreferSameNode`), `service.externalTrafficPolicy` and `service.internalTrafficPolicy` (`Cluster`, `Local`), and `service.sessionAffinity` (`None`, `ClientIP`) with `service.sessionAffinityTimeoutSeconds`, and render them in the Service spec.

#### Scenario: Traffic fields
- **GIVEN** `service.trafficDistribution` is `PreferClose` and `service.internalTrafficPolicy` is `Local`
- **WHEN** the chart is rendered
- **THEN** the Service spec carries both

#### Scenario: Session affinity
- **GIVEN** `service.sessionAffinity` is `ClientIP` and `service.sessionAffinityTimeoutSeconds` is 600
- **WHEN** the chart is rendered
- **THEN** the Service spec has client-IP affinity with a 600 second timeout

### Requirement: Address and load balancer options
The chart SHALL take `service.clusterIP`, `service.loadBalancerClass`, `service.loadBalancerSourceRanges` and `service.loadBalancerIP`, and render them in the Service spec.

#### Scenario: Client addresses behind a load balancer
- **GIVEN** `service.type` is `LoadBalancer`, `service.externalTrafficPolicy` is `Local`, and a load balancer class and source ranges are set
- **WHEN** the chart is rendered
- **THEN** the Service spec carries the policy, the class and the source ranges

#### Scenario: Pinned cluster IP
- **GIVEN** `service.clusterIP` is `10.96.0.53`
- **WHEN** the chart is rendered
- **THEN** the Service spec has that cluster IP

### Requirement: Service option values are checked
The values schema SHALL reject a Service option value outside the allowed ones listed in the Service option requirements, and a source range that isn't a CIDR.

#### Scenario: Unknown value
- **WHEN** `service.trafficDistribution` is set to `Closest`
- **THEN** rendering fails because the value isn't one of the allowed ones

### Requirement: Service option combinations are checked
Rendering SHALL fail, with a message that names the values, when `service.externalTrafficPolicy` is set and `service.type` is neither `NodePort` nor `LoadBalancer`; when a load balancer option is set and `service.type` is not `LoadBalancer`; when two `service.ipFamilies` are given with the policy `SingleStack`; and when `service.sessionAffinityTimeoutSeconds` is set without `service.sessionAffinity` being `ClientIP`.

#### Scenario: External traffic policy on a cluster-internal Service
- **WHEN** `service.externalTrafficPolicy` is `Local` and `service.type` is `ClusterIP`
- **THEN** rendering fails with a message that names `service.externalTrafficPolicy` and the types it needs

#### Scenario: Load balancer option on another type
- **WHEN** `service.loadBalancerSourceRanges` is set and `service.type` is `ClusterIP`
- **THEN** rendering fails with a message that names `service.loadBalancerSourceRanges` and the type `LoadBalancer`

#### Scenario: Two IP families with a single-stack policy
- **WHEN** `service.ipFamilies` has two entries and `service.ipFamilyPolicy` is `SingleStack`
- **THEN** rendering fails with a message that names both values

#### Scenario: Timeout without client-IP affinity
- **WHEN** `service.sessionAffinityTimeoutSeconds` is set and `service.sessionAffinity` is unset
- **THEN** rendering fails with a message that names both values
