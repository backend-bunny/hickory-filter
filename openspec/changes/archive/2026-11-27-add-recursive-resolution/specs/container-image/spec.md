# Spec Delta

## ADDED Requirements

### Requirement: Root server hints in the image
The image SHALL contain the root server hints at `/usr/share/hickory-filter/root.hints`, the default of `recursion.roots`. The file SHALL NOT be under `/etc/hickory-filter`, which deployments mount their configuration over. Recursive mode SHALL start in the restricted container (all capabilities dropped, read-only root filesystem) with the shipped hints.

#### Scenario: Hints present
- **WHEN** the image filesystem is inspected
- **THEN** `/usr/share/hickory-filter/root.hints` exists and lists root server addresses

#### Scenario: Recursive mode in the restricted container
- **GIVEN** the image runs as uid 65532 with all capabilities dropped and a read-only root filesystem
- **WHEN** it starts with `mode = "recursive"` and no other recursion settings
- **THEN** it loads the hints, binds its listeners and `/health` returns 200
