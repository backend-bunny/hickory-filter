# Proposal

## Why

hickory-filter is meant to run on Kubernetes. Operators need an image and a Helm chart they can verify before running them: who built them, from which commit, what's inside, and under which licenses. A statically linked binary bundles every Rust crate it depends on, so a release also carries those crates' license obligations, and has to ship their notices.

## What Changes

- **Container image** built from source with melange (binary → APK) and apko (APK → image), with no Dockerfile. The binary is **statically linked against musl** and the runtime image is distroless: the binary, CA certificates and nothing else (no shell, no libc, no package manager). It runs as non-root uid/gid 65532.
- **Reproducible inputs**: the build is offline from vendored crates (`Cargo.lock`, `--locked`), the runtime package set comes from a committed apko lockfile, build tools are pinned, and melange signing keys are throwaway per build.
- **Helm chart** for Kubernetes, published as an **OCI artifact**. A released chart pins the image digest it was released with, and its defaults pass the `restricted` Pod Security Standard.
- **Supply chain**: the image index and the chart are both **signed keylessly** with cosign and carry **SLSA build provenance**. The image also carries an **SPDX SBOM** (including the Rust crates, via cargo-auditable) and a **license notices** attestation.
- **License notices** for every crate linked into the binary, also shipped as a file inside the image. CI fails on a crate whose license isn't on an allowlist.
- **Reproducible dev environment**: one pinned toolchain manifest (devbox) shared by local work and CI, and a Taskfile whose tasks call the same `hack/` scripts CI runs.
- **CI**: pull-request workflow with read-only permissions (Conventional Commits check, Rust checks, license check, chart checks, image build and smoke test, publish dry run) gated by a single required check, `ci-ok`.
- **Automated releases**: release-please in manifest mode keeps a `chore: release X.Y.Z` PR open, derived from Conventional Commits, bumping `Cargo.toml`, `Cargo.lock`, `melange.yaml` and the chart together and writing `CHANGELOG.md`. Merging that PR is the human gate: it creates the `vX.Y.Z` tag and GitHub Release, then the same workflow run calls the publish workflow. Pushing a tag by hand publishes nothing.
- **Dependency updates**: Renovate keeps actions pinned by digest and bumps crates and the toolchain, with commit types chosen so crate and toolchain bumps release as patches.
- Publish flow: build → test → publish image → sign/attest → package chart with the digest → push → sign/attest chart.
- **Hosting decided**: GitHub, `backend-bunny/hickory-filter`; image `ghcr.io/backend-bunny/hickory-filter`, chart `oci://ghcr.io/backend-bunny/charts/hickory-filter`.
- **Project rule change** (already applied in AGENTS.md): images and the chart are built and published from this repo. Dockerfiles are still not allowed.

## Capabilities

### New Capabilities

- `container-image`: how the image is built (static musl binary, melange + apko, lockfile), what it contains, and its runtime contract.
- `helm-chart`: the chart's resources, values, security defaults, and OCI packaging pinned to the image digest.
- `release-supply-chain`: versioning, keyless signing, SBOM and provenance attestations for the image and chart, and how a consumer verifies them.
- `license-notices`: collecting, checking and shipping the licenses and notice files of everything linked into the binary.

### Modified Capabilities

None. `dns-forwarding`'s non-root operation requirement (from `add-filtering-dns-forwarder`) is what the image and chart rely on. It doesn't change.

## Impact

- New files: `melange.yaml`, `apko.yaml`, `apko.lock.json`, `charts/hickory-filter/`, `hack/` scripts, `Taskfile.yml`, `devbox.json` + `devbox.lock`, `.envrc`, `.gitignore`, `.github/` workflows and the tool-setup action, `release-please-config.json`, `.release-please-manifest.json`, `CHANGELOG.md`, `renovate.json`, and `deny.toml` (the license allowlist).
- New build-time tools: melange, apko, cosign, helm, cargo-auditable, a chart linter/validator, bubblewrap (the melange runner), go-task.
- Needs GitHub Actions with OIDC (keyless signing) and GHCR. One-time repository settings: allow Actions to create pull requests (release-please), and `ci-ok` as the required check on `main`.
- Depends on `add-filtering-dns-forwarder` for the binary, its `--version` output, the health/readiness endpoints and the non-root defaults.
