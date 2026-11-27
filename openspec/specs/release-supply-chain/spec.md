# release-supply-chain Specification

## Purpose
Makes every published image and chart verifiable: who built it, from which commit, and what it contains, using keyless signatures and attestations bound to the artifact digests.

## Requirements

### Requirement: Releases come from version tags
A release SHALL be built from a `vX.Y.Z` tag, and only from it. The tag version SHALL equal the version in `Cargo.toml`, the melange package version and the chart version, and `hickory-filter --version` SHALL print `hickory-filter X.Y.Z`. Any mismatch SHALL fail the release before publishing.

#### Scenario: Version mismatch
- **WHEN** tag `v1.2.3` points at a commit whose `Cargo.toml` says `1.2.2`
- **THEN** the release fails without publishing anything

### Requirement: Release PR is the human gate
Releases SHALL be started by merging a release pull request, kept up to date automatically from the Conventional Commits merged since the last release. Merging it SHALL create the `vX.Y.Z` tag and the GitHub Release and then publish, in the same workflow run. Pushing a tag by hand SHALL NOT publish anything.

#### Scenario: Merge publishes
- **WHEN** the release PR for `1.2.3` is merged
- **THEN** tag `v1.2.3` and its GitHub Release are created and the publish workflow runs for that tag

#### Scenario: Hand-pushed tag
- **WHEN** someone pushes a `vX.Y.Z` tag without merging a release PR
- **THEN** no image or chart is published

### Requirement: Release PR carries the version bump
The release PR SHALL bump `Cargo.toml`, `Cargo.lock`, the melange package version and the chart's `version` and `appVersion` together, and update `CHANGELOG.md`. It SHALL run the same CI checks as any other PR before it can merge.

#### Scenario: Versions bumped together
- **WHEN** the release PR for `1.2.3` is opened
- **THEN** `Cargo.toml`, `Cargo.lock`, the melange version and the chart `version` and `appVersion` all say `1.2.3`

#### Scenario: No releasable commits
- **WHEN** the only commits since the last release are of hidden types (docs, ci, chore, test, refactor, style)
- **THEN** no release PR proposes a new version

### Requirement: Conventional Commits enforced
Pull request titles and every commit subject SHALL follow `type(scope)!: subject`, and CI SHALL fail otherwise, because they decide the next version and the changelog.

#### Scenario: Bad title
- **WHEN** a pull request is titled "update stuff"
- **THEN** the `conventional-commits` check fails

### Requirement: Single required check
CI SHALL expose one aggregate check, `ci-ok`, that fails unless every other CI job succeeded. Branch protection on `main` SHALL require it, and direct pushes to `main` are not part of the workflow.

#### Scenario: One job fails
- **WHEN** the chart job fails and the others pass
- **THEN** `ci-ok` fails and the PR can't merge

### Requirement: Dependency updates
Dependencies SHALL be kept current by an automated update tool that pins CI actions by digest and groups crate, toolchain and action bumps. Its commit types SHALL be set so crate and toolchain bumps produce a patch release and action bumps produce none. Bumping the Alpine branch of the build environment SHALL require explicit approval.

#### Scenario: Action bump
- **WHEN** the tool bumps a pinned action
- **THEN** the commit type is `ci(deps)` and no release is proposed for it

### Requirement: Image tags
A released image SHALL be tagged `X.Y.Z` and `sha-<short commit>`. No floating tag such as `latest` SHALL be published. Documentation SHALL tell consumers to pin by digest.

#### Scenario: No latest
- **WHEN** release `v1.2.3` is published
- **THEN** the registry has tags `1.2.3` and `sha-<commit>` for it, and no `latest`

### Requirement: Keyless signing
The image index digest and the chart's OCI digest SHALL each be signed with cosign, using the CI provider's OIDC identity (keyless, with a public transparency log). No long-lived signing key SHALL exist.

#### Scenario: Verify image signature
- **WHEN** a consumer runs `cosign verify <image>@<digest>` with the release workflow's certificate identity and OIDC issuer
- **THEN** verification succeeds

#### Scenario: Verify chart signature
- **WHEN** a consumer runs `cosign verify ghcr.io/backend-bunny/charts/hickory-filter@<chart digest>` with the same identity constraints
- **THEN** verification succeeds

### Requirement: Sign what tags point to
Signatures and attestations SHALL be attached to the image **index** digest that the tags resolve to, not only to per-architecture manifests.

#### Scenario: Tag digest verifies
- **WHEN** a consumer resolves tag `1.2.3` to its digest and verifies it
- **THEN** the signature and every attestation are found on that digest

### Requirement: SBOM attestation
The image index digest SHALL carry an SPDX SBOM attestation listing every package in the image and the Rust crates in the binary, with versions.

#### Scenario: SBOM lists crates
- **WHEN** a consumer downloads and verifies the image's SPDX attestation
- **THEN** it includes the `hickory-filter` package and entries for its Rust crates (for example `hickory-server`)

### Requirement: Build provenance
The image index digest and the chart digest SHALL each carry a SLSA build provenance attestation naming the source repository, the commit, the workflow and the build run.

#### Scenario: Provenance of the chart
- **WHEN** a consumer verifies the chart digest's provenance attestation
- **THEN** it names the release tag's commit and the release workflow

### Requirement: Publish only tested artifacts
The release SHALL publish only after the Rust checks, the chart validation and the image smoke test have passed on the tagged commit. Pull request builds SHALL have no permission to publish, sign or obtain OIDC tokens.

#### Scenario: Pull request build
- **WHEN** CI runs for a pull request
- **THEN** it builds and tests the image and chart but has no registry write or OIDC permission

### Requirement: Least-privilege CI
CI workflows SHALL default to read-only repository permissions, and grant package write, OIDC and attestation permissions only to the release job. Third-party CI actions SHALL be pinned by full commit SHA.

#### Scenario: Unpinned action
- **WHEN** a workflow references an action by tag only
- **THEN** the repository's checks fail

### Requirement: Pinned build tools
melange, apko, cosign, helm and the other build tools SHALL be pinned to exact versions in one committed manifest, used by both CI and local builds.

#### Scenario: Same tools locally and in CI
- **WHEN** a developer runs the build locally through the pinned toolchain
- **THEN** it uses the same tool versions as CI

### Requirement: One implementation per step
Local tasks and CI jobs SHALL call the same scripts for building, testing, locking, notices and publishing. CI SHALL NOT carry its own copy of a step's logic.

#### Scenario: Local run matches CI
- **WHEN** a developer runs the Rust checks task locally
- **THEN** it runs the script the CI `rust` job runs

### Requirement: Release record
The release notes SHALL list the image digest, the chart reference and digest, and copy-pasteable verification commands for both.

#### Scenario: Release notes
- **WHEN** release `v1.2.3` completes
- **THEN** its notes contain the image and chart digests and the `cosign verify` commands
