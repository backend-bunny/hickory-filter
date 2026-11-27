# Spec Delta

## Purpose

Defines how the hickory-filter container image is built and what it contains, so it is minimal, reproducible from a commit, and runs unprivileged.

## ADDED Requirements

### Requirement: Built without a Dockerfile
The image SHALL be built by melange (source → APK) and apko (APKs → OCI image) only. The repository SHALL NOT contain a Dockerfile or Containerfile, or a `docker build` step.

#### Scenario: Image build
- **WHEN** the image build runs for a commit
- **THEN** it produces an OCI image layout via melange and apko, without invoking a Dockerfile-based builder

### Requirement: Statically linked musl binary
The packaged `hickory-filter` binary SHALL be statically linked against musl, with no ELF program interpreter and no dynamic library dependencies. The build SHALL fail when the binary is dynamically linked.

#### Scenario: Static check
- **WHEN** the package test inspects `/usr/bin/hickory-filter`
- **THEN** it finds no program interpreter and no `NEEDED` entries, otherwise the build fails

#### Scenario: Runs without libc
- **GIVEN** an environment with no musl or glibc installed
- **WHEN** `hickory-filter --version` runs
- **THEN** it prints the version and exits 0

### Requirement: Offline, locked build
The binary SHALL be compiled with `Cargo.lock` enforced (`--locked`), from vendored crate sources, with no network access during compilation.

#### Scenario: Lockfile drift
- **WHEN** `Cargo.toml` changes without a matching `Cargo.lock` update
- **THEN** the build fails instead of resolving new versions

### Requirement: Crate list embedded in the binary
The binary SHALL embed the list of crates it was built from (name, version, source), readable by SBOM generators and vulnerability scanners.

#### Scenario: Scanner sees crates
- **WHEN** an SBOM generator scans the image
- **THEN** its output lists the Rust crates in the binary with their versions

### Requirement: Distroless contents
The runtime image SHALL contain only the hickory-filter package, CA certificates, and the minimal filesystem base they need. It SHALL NOT contain a shell, a package manager or a C library.

#### Scenario: No shell
- **WHEN** a container is started from the image with entrypoint `/bin/sh`
- **THEN** it fails because no shell exists

### Requirement: Runtime package set locked
The runtime image's package set SHALL come from a committed apko lockfile. A build SHALL fail when the apko configuration and the lockfile disagree. The lockfile SHALL only change through a reviewed commit.

#### Scenario: Config changed without relock
- **WHEN** a package is added to the apko configuration without regenerating the lockfile
- **THEN** the build fails and names the mismatch

### Requirement: Throwaway package signing key
The APKs SHALL be signed with a key pair generated inside the build job, used only to hand the APKs to apko, and then discarded. No package signing key SHALL be committed, stored as a secret or reused.

#### Scenario: Key lifetime
- **WHEN** a build finishes
- **THEN** no private package signing key remains in the workspace, the repository or the CI secrets

### Requirement: Runtime contract
The image SHALL run as uid/gid 65532 (numeric) with entrypoint `/usr/bin/hickory-filter` and a default config path under `/etc/hickory-filter/`. It SHALL work with all capabilities dropped and a read-only root filesystem. The image SHALL be built for x86_64, with further architectures added only as a separate change.

#### Scenario: Restricted run
- **WHEN** the image runs as uid 65532, with all capabilities dropped and a read-only root filesystem, using a mounted config with listen port 1053
- **THEN** it serves DNS and answers `/ready`

### Requirement: Standard OCI annotations
The image index and manifest SHALL carry `org.opencontainers.image.{title,description,licenses,vendor,url,documentation,version,revision,source,created}`. `licenses` SHALL be the SPDX expression for the project's license, and `version` the release version.

#### Scenario: Annotations present
- **WHEN** the image is inspected
- **THEN** every listed annotation is present and non-empty, otherwise the image test fails

### Requirement: Image smoke test
Every image build SHALL be tested by running the image: version output, the static and no-shell checks, a DNS query answered for a blocked and an unblocked name against a local upstream, and `/health` and `/ready`.

#### Scenario: Smoke test gates publishing
- **WHEN** the smoke test fails
- **THEN** nothing is published
