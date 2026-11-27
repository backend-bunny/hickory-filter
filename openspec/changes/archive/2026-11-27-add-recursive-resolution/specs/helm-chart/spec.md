# Spec Delta

## ADDED Requirements

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
