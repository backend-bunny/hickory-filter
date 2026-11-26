# Tasks

## 1. Project rules and toolchain

- [ ] 1.1 Check AGENTS.md packaging rules (added with this proposal) against the final implementation and update any detail that changed; verify the self-contained-docs grep in 6.3 passes
- [x] 1.2 Add the pinned toolchain (`devbox.json` + `devbox.lock`: melange, apko, cosign, helm, schema validator, cargo-deny, bubblewrap, go-task, python, jq, dev Rust), `.envrc`, `.gitignore` (build outputs, vendored crates, melange keys, devbox state), and a `Taskfile.yml` with build/test/lock/chart/publish-dry-run tasks calling `hack/`; verify `task --list` and each tool's `--version` run through the manifest
- [x] 1.2a Add the `.github/actions/setup-tools` composite action (devbox install, cache restore keyed per D7); verify a CI run restores caches on the second run
- [ ] 1.3 Set `rust-version` in `Cargo.toml` to the pinned Alpine branch's rust and add a CI job that builds with that compiler through melange; verify the job fails on a deliberately newer-only API in a scratch commit

## 2. Static musl package (melange)

- [ ] 2.1 Write `melange.yaml`: Alpine build environment per D1, `RUSTFLAGS=+crt-static`, the `*-alpine-linux-musl` target, `cargo auditable build --locked --release --offline`, installing `/usr/bin/hickory-filter`; verify `hack/build.sh` produces the APK offline (network disabled for the compile step)
- [ ] 2.2 Implement `hack/build.sh` vendoring (committed files → temp dir → `cargo vendor --locked --versioned-dirs` → `--source-dir`) and the throwaway key flow; verify that after a build no private key exists in the workspace or temp dirs
- [ ] 2.3 Add melange's test pipeline in a Wolfi environment: `--version` matches the package version, `readelf` shows no interpreter and no `NEEDED`; verify the test fails when `+crt-static` is removed

## 3. Runtime image (apko)

- [ ] 3.1 Write `apko.yaml` (Wolfi + `@local`, `hickory-filter@local`, `ca-certificates-bundle`, uid/gid 65532, entrypoint, `/etc/hickory-filter`, `/tmp`, OCI annotations, x86_64) and `hack/lock.sh`; generate and commit `apko.lock.json`; verify `apko build --lockfile` succeeds and the image has no `/bin/sh`
- [ ] 3.2 Implement `hack/lockfile.py merge` (adds `@local` packages, fails when the config or the resolved Wolfi set drifts from the committed lock); verify unit tests for an unchanged lock, an added package without relock, and a dropped package
- [ ] 3.3 Write `hack/test.sh` (the image smoke test: user 65532, annotations, no shell, static binary, DNS answers for blocked and unblocked names against a local upstream, `/health` and `/ready`, all caps dropped, read-only rootfs); verify it passes on a fresh build and fails on an image missing an annotation

## 4. License notices

- [x] 4.1 Add `deny.toml` with the license allowlist and a `cargo deny check licenses` step in CI; verify CI fails on a scratch dependency licensed GPL-3.0-only
- [ ] 4.2 Implement `hack/notices.py` per D5 (normal deps on the musl target, license and notice files from the vendored tree, pinned SPDX fallback texts, musl entry, reviewed-exception list) writing `notices.json` and `THIRD-PARTY-NOTICES.md`; verify unit tests for a crate with NOTICE, a crate with no file (canonical fallback), a build-only crate (excluded) and a crate with nothing (fails)
- [ ] 4.3 Ship `THIRD-PARTY-NOTICES.md`, `LICENSE-MIT` and `LICENSE-APACHE` in `/usr/share/doc/hickory-filter/` via the melange source dir; verify a test comparing the crate set in the image's notices, `notices.json` and the binary's cargo-auditable data

## 5. Helm chart

- [ ] 5.1 Create `charts/hickory-filter/` (Chart.yaml with version = appVersion, `values.yaml` with `image.digest: ""`, `values.schema.json`, Deployment, Service 53→1053 UDP/TCP, ConfigMap rendered as TOML with checksum annotation, ServiceAccount); verify `helm lint` and schema validation of the default render pass
- [ ] 5.2 Add the restricted security context, `httpGet` probes on `/health` and `/ready`, the `/tmp` emptyDir and the optional cache volume (emptyDir/PVC); verify the default render passes a `restricted` Pod Security check (e.g. a server-side dry run against a namespace labelled `restricted` in a local test cluster, or a policy linter)
- [ ] 5.3 Add opt-in PDB, NetworkPolicy and ServiceMonitor, plus `ci/all-on-values.yaml`; verify lint and schema validation pass for that file too
- [ ] 5.3a Add the recommended labels to every resource, `podLabels`, `podAnnotations` and `service.annotations`, the named `http` port, the shared ServiceMonitor/PodMonitor options, the opt-in PodMonitor and the opt-in `prometheus.io/*` scrape annotations (port derived from the configured HTTP address, explicit `podAnnotations` winning); extend `ci/all-on-values.yaml`; verify golden renders for the labels, the annotations next to the unchanged config checksum, a monitor with custom labels and interval, the scrape annotations on the default port and on port 9090 from the configuration, an explicit annotation overriding a generated one, and that enabling a monitor or the scrape annotations with the HTTP endpoint disabled fails with the message
- [ ] 5.3b Add the opt-in `GrafanaDashboard` resource and labelled dashboard ConfigMap that embed `dashboards/hickory-filter.json` (available once `add-grafana-dashboard` has added it); make the chart validation skip only the `GrafanaDashboard` kind; verify golden renders for each resource and for the default render having neither
- [ ] 5.4 Implement `hack/chart-publish.sh` (temp copy, fill `image.digest`, `helm package`, `helm push` to `oci://ghcr.io/backend-bunny/charts`, write the chart digest; `DRY_RUN=1` packages only); verify a dry run produces a package whose rendered image is `…@<given digest>`
- [ ] 5.5 Write `charts/hickory-filter/README.md` (install from OCI, values reference, verification commands); verify every value named in it exists in `values.schema.json`; include example values for scraping through a Prometheus Operator monitor, through annotations for a telemetry collector, and for loading the dashboard

## 6. Release pipeline

- [ ] 6.1 Implement `hack/check-version.sh [tag]` (tag = Cargo.toml = melange = Chart version/appVersion = `--version` output); verify it fails on each kind of mismatch
- [ ] 6.2 Implement `hack/publish.sh` (`apko publish` with tags `X.Y.Z` and `sha-<short>`, resolve and write the index digest; `DRY_RUN=1` prints the tags); verify the dry run lists exactly those two tags
- [x] 6.3 Implement `hack/check-commits.sh` (PR title and every commit subject match `type(scope)!: subject`); verify it rejects "update stuff" and accepts `fix(deps): bump x`
- [ ] 6.4 Write `ci.yml` per D8 (`contents: read` only; jobs `conventional-commits`, `rust`, `chart`, `image`, and `ci-ok` aggregating them; pinned-action check; self-contained-docs grep); verify a PR run passes, has no write or OIDC permission, and `ci-ok` fails when one job is made to fail
- [ ] 6.5 Add `release-please-config.json`, `.release-please-manifest.json` and the `x-release-please-version` markers in `melange.yaml` and `Chart.yaml`, and write `release-please.yml` (opens the release PR, dispatches `ci.yml` on its branch, and on merge tags, creates the Release and calls `release.yml`); verify the first release PR bumps `Cargo.toml`, `Cargo.lock`, `melange.yaml` and the chart together and `hack/check-version.sh` passes on it
- [ ] 6.6 Write `release.yml` per D4 (`workflow_call` and `workflow_dispatch` with a tag; sign the index, attest SBOM + notices + provenance, publish chart, sign + provenance for the chart, append digests and verify commands to the release notes) with permissions only on the publish job; verify with a first pre-release that `cosign verify`, `cosign verify-attestation --type spdxjson`, the notices type and the provenance verification all succeed for both digests, and that a hand-pushed tag publishes nothing
- [ ] 6.7 Add `renovate.json` per D8 (actions pinned by digest, grouped crate/toolchain/action PRs with the commit types from the design, Alpine branch bumps behind approval, weekly lockfile maintenance); verify the config validates and a sample bump's commit type matches
- [ ] 6.8 Document verification, the runtime contract and the release flow (merge the release PR) in README.md (image, chart, attestation types, pin by digest); verify the documented commands succeed against the pre-release from 6.6

## Workflow follow-up

- One-time repository settings before 6.5: allow Actions to create and approve pull requests, allow OIDC for workflows, require `ci-ok` on `main`. After the first publish, set the GHCR image and chart packages public if intended.
- Staging: if `.github/` can't be written from an agent session, stage the workflow files elsewhere and have a maintainer move them.
- Ship Wolfi package updates with a `task lock` PR before the release PR.
- Archive after the first release verifies, and check the archived specs in `openspec/specs/`.
