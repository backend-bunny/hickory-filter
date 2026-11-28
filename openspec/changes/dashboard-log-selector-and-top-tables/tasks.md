# Tasks

## 1. Dashboard

- [x] 1.1 Turn the three usage rankings into tables (hide `Time`, rename the label column and `Value #A`, sort by count, gauge cell, height 12) and move the panels below down by the added height; verify no panels overlap and `tests/dashboard.rs` passes
- [x] 1.2 Add a test that each ranking is a table with the name or client column, a `Queries` count, a descending sort and no reduce step; verify it fails against the previous dashboard

## 2. Chart value

- [x] 2.1 Add `dashboard.logSelector` to `values.yaml` (empty, with a comment) and `values.schema.json`; render the dashboard through one helper used by both resources that replaces the packaged default when the value is set and fails, naming the value, when the default isn't found; verify a default render is unchanged and a set value replaces every occurrence in both resources with valid, correctly escaped JSON

## 3. Tests

- [x] 3.1 Add a golden case with both resources and a selector containing quotes and a comma; regenerate the golden files; verify `hack/chart-test.sh` passes and the default golden is untouched

## 4. Documentation

- [x] 4.1 Document `dashboard.logSelector` in the chart README's values table with an example, and the tables and the value in the project README's dashboard section; verify the README check passes

## Workflow follow-up

- Released through the release PR (a `feat` commit makes a minor release); verify the published chart renders the tables and the override.
- Archive after the release verifies, and check the main `grafana-dashboard` and `helm-chart` specs.
