# Tasks

## 1. Project rules and toolchain

- [ ] 1.1 Check AGENTS.md packaging rules (added with this proposal) against the final implementation and update any detail that changed; verify the self-contained-docs grep in 6.3 passes
- [ ] 1.2 Add the pinned toolchain manifest (melange, apko, cosign, helm, schema validator, cargo-deny, bubblewrap, python3, jq, dev Rust) and a Taskfile with build/test/lock/chart/publish-dry-run tasks calling `hack/`; verify `task --list` and each tool's `--version` run through the manifest
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

- [ ] 4.1 Add `deny.toml` with the license allowlist and a `cargo deny check licenses` step in CI; verify CI fails on a scratch dependency licensed GPL-3.0-only
- [ ] 4.2 Implement `hack/notices.py` per D5 (normal deps on the musl target, license and notice files from the vendored tree, pinned SPDX fallback texts, musl entry, reviewed-exception list) writing `notices.json` and `THIRD-PARTY-NOTICES.md`; verify unit tests for a crate with NOTICE, a crate with no file (canonical fallback), a build-only crate (excluded) and a crate with nothing (fails)
- [ ] 4.3 Ship `THIRD-PARTY-NOTICES.md` and `LICENSE` in `/usr/share/doc/hickory-filter/` via the melange source dir; verify a test comparing the crate set in the image's notices, `notices.json` and the binary's cargo-auditable data

## 5. Helm chart

- [ ] 5.1 Create `charts/hickory-filter/` (Chart.yaml with version = appVersion, `values.yaml` with `image.digest: ""`, `values.schema.json`, Deployment, Service 53→1053 UDP/TCP, ConfigMap rendered as TOML with checksum annotation, ServiceAccount); verify `helm lint` and schema validation of the default render pass
- [ ] 5.2 Add the restricted security context, `httpGet` probes on `/health` and `/ready`, the `/tmp` emptyDir and the optional cache volume (emptyDir/PVC); verify the default render passes a `restricted` Pod Security check (e.g. a server-side dry run against a namespace labelled `restricted` in a local test cluster, or a policy linter)
- [ ] 5.3 Add opt-in PDB, NetworkPolicy and ServiceMonitor, plus `ci/all-on-values.yaml`; verify lint and schema validation pass for that file too
- [ ] 5.4 Implement `hack/chart-publish.sh` (temp copy, fill `image.digest`, `helm package`, `helm push` to `oci://<registry>/<owner>/charts`, write the chart digest; `DRY_RUN=1` packages only); verify a dry run produces a package whose rendered image is `…@<given digest>`
- [ ] 5.5 Write `charts/hickory-filter/README.md` (install from OCI, values reference, verification commands); verify every value named in it exists in `values.schema.json`

## 6. Release pipeline

- [ ] 6.1 Implement `hack/check-version.sh` (tag = Cargo.toml = melange = Chart version/appVersion = `--version` output); verify it fails on each kind of mismatch
- [ ] 6.2 Implement `hack/publish.sh` (`apko publish` with tags `X.Y.Z` and `sha-<short>`, resolve and write the index digest; `DRY_RUN=1` prints the tags); verify the dry run lists exactly those two tags
- [ ] 6.3 Write CI for pull requests (`contents: read` only: Rust checks, cargo-deny, chart checks, build, smoke test, publish dry run, a pinned-action check, a self-contained-docs grep); verify a PR run passes and has no write or OIDC permission
- [ ] 6.4 Write the release workflow per D4 (sign the index, attest SBOM + notices + provenance, publish chart, sign + provenance for the chart, append digests and verify commands to the release notes) with permissions only on that job; verify with a first pre-release tag that `cosign verify`, `cosign verify-attestation --type spdxjson`, the notices type and the provenance verification all succeed for both digests
- [ ] 6.5 Document verification and the runtime contract in README.md (image, chart, attestation types, pin by digest); verify the documented commands succeed against the pre-release from 6.4

## Workflow follow-up

- Decide repo hosting (registry/owner paths) and the release PR tool before 6.4.
- Archive after the first release verifies, and check the archived specs in `openspec/specs/`.
