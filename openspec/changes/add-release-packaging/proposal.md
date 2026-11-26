# Proposal

## Why

hickory-filter is meant to run on Kubernetes. Operators need an image and a Helm chart they can verify before running them: who built them, from which commit, what's inside, and under which licenses. A statically linked binary bundles every Rust crate it depends on, so a release also carries those crates' license obligations, and has to ship their notices.

## What Changes

- **Container image** built from source with melange (binary → APK) and apko (APK → image), with no Dockerfile. The binary is **statically linked against musl** and the runtime image is distroless: the binary, CA certificates and nothing else (no shell, no libc, no package manager). It runs as non-root uid/gid 65532.
- **Reproducible inputs**: the build is offline from vendored crates (`Cargo.lock`, `--locked`), the runtime package set comes from a committed apko lockfile, build tools are pinned, and melange signing keys are throwaway per build.
- **Helm chart** for Kubernetes, published as an **OCI artifact**. A released chart pins the image digest it was released with, and its defaults pass the `restricted` Pod Security Standard.
- **Supply chain**: the image index and the chart are both **signed keylessly** with cosign and carry **SLSA build provenance**. The image also carries an **SPDX SBOM** (including the Rust crates, via cargo-auditable) and a **license notices** attestation.
- **License notices** for every crate linked into the binary, also shipped as a file inside the image. CI fails on a crate whose license isn't on an allowlist.
- Release flow from a version tag: build → test → publish image → sign/attest → package chart with the digest → push → sign/attest chart.
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

- New files: `melange.yaml`, `apko.yaml`, `apko.lock.json`, `charts/hickory-filter/`, build/release scripts, CI and release workflows, a pinned toolchain manifest, and `deny.toml` (the license allowlist).
- New build-time tools: melange, apko, cosign, helm, cargo-auditable, a chart linter/validator, bubblewrap (the melange runner).
- Needs a registry and an OIDC-capable CI for keyless signing. Repo hosting is still an open decision (AGENTS.md), so registry paths use a placeholder `<registry>/<owner>`.
- Depends on `add-filtering-dns-forwarder` for the binary, its `--version` output, the health/readiness endpoints and the non-root defaults.
