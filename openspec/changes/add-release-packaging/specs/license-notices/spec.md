# Spec Delta

## Purpose

Meets the license obligations of shipping a statically linked binary: every bundled dependency's license and notice files travel with each release, and licenses outside the allowlist can't slip in.

## ADDED Requirements

### Requirement: Notices cover everything linked
The notices SHALL list every crate linked into the released binary on its release target: normal dependencies, not build-only or dev-only ones. Each entry SHALL give name, version, source, declared SPDX license expression, and the full text of every license and notice file the crate ships. The C library linked into the binary SHALL be included.

#### Scenario: Crate with a NOTICE file
- **GIVEN** a linked crate that ships `LICENSE-APACHE` and `NOTICE`
- **WHEN** the notices are generated
- **THEN** its entry contains both files' full text

#### Scenario: Build-only crate excluded
- **GIVEN** a crate used only by a build script
- **WHEN** the notices are generated
- **THEN** it has no entry

### Requirement: Canonical license text fallback
When a crate ships no license file, its entry SHALL include the canonical text of each SPDX license in its declared expression. Generation SHALL fail for a crate with neither a license file nor a declared license, unless that exact name@version is listed as reviewed, with a note.

#### Scenario: No license at all
- **GIVEN** a linked crate with no license file and no `license` field
- **WHEN** the notices are generated
- **THEN** generation fails and names the crate

### Requirement: License allowlist
CI SHALL fail when any linked crate's license expression can't be satisfied by an allowlist of licenses kept in the repository. Adding a license to the allowlist SHALL be a reviewed change.

#### Scenario: Copyleft crate added
- **WHEN** a dependency update pulls in a crate licensed only under GPL-3.0
- **THEN** CI fails and names the crate and its license

### Requirement: Notices shipped in the image
The image SHALL contain the notices as a human-readable file at `/usr/share/doc/hickory-filter/THIRD-PARTY-NOTICES.md`, together with the project's own license file.

#### Scenario: Notices in the image
- **WHEN** the image filesystem is inspected
- **THEN** both files exist and list the same crates as the attestation

### Requirement: Notices attestation
The image index digest SHALL carry the notices as a signed attestation of a project-defined predicate type, in machine-readable form (JSON), with the same content as the file in the image.

#### Scenario: Verify notices
- **WHEN** a consumer verifies the notices attestation of an image digest
- **THEN** the predicate lists every crate in the SBOM with its license texts

### Requirement: Image license annotation
The image's `org.opencontainers.image.licenses` annotation SHALL be the project's own license expression. The bundled dependencies' licenses SHALL be given in the notices and SBOM, not in the annotation.

#### Scenario: Annotation
- **WHEN** the image is inspected
- **THEN** `org.opencontainers.image.licenses` equals the project's SPDX expression

### Requirement: Notices generated from the release inputs
The notices SHALL be generated from the same `Cargo.lock` and vendored sources the binary is compiled from, in the same build, so they can't describe a different dependency set.

#### Scenario: Dependency bump
- **WHEN** a commit bumps a crate version
- **THEN** that commit's image and attestation list the new version
