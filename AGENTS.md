# Agent Guide — hickory-filter

A filtering DNS forwarder: `hickory-server` used as a library, plus a filter
with allow/block rules, several list formats, and scheduled refresh of
blocklists from web servers. See `README.md` for the features and the
planned architecture.

## Status

The DNS server is implemented and its specs are in `openspec/specs/`
(`dns-forwarding`, `filter-rules`, `blocklist-sources`, `blocked-responses`,
`observability`). Still planned, as changes in `openspec/changes/`:
`add-release-packaging` (image, Helm chart, release workflows) and
`add-grafana-dashboard`. Work happens through OpenSpec: implement agreed
changes, and don't expand scope without the user. Use the `openspec-*` skills
(`.claude/skills/` → `.agents/skills/`): propose, explore, update, apply,
archive.

## Hard rules

- **Self-contained docs.** Nothing in this repo (README, AGENTS.md, specs,
  designs, comments, commit messages) references other projects, products or
  repositories. The only exceptions are the libraries the code depends on,
  the build tools and base distributions the packaging uses, and the
  monitoring formats and APIs the integrations speak (Prometheus metrics and
  operator resources, Loki log queries, Grafana dashboards), each named where
  it is used. Describe formats by their syntax (`||x^`,
  `address=/x/#`), not by the tools they come from.
- **Library, not fork.** Use `hickory-server`'s public API (`RequestHandler`,
  `Catalog`, `ForwardZoneHandler`, `Server`). If something we need isn't
  public, raise it with the user. Don't vendor or patch the library.
- **Pin `hickory-server` to an exact release** in `Cargo.toml`, with
  `Cargo.lock` committed. Its API is pre-1.0, so an upgrade is its own change.
- **Allow always wins** over any block rule, from any source.
- **The query path never blocks on I/O or on a refresh.** A refresh builds a
  new rule set and swaps it in atomically. A failed fetch keeps the last good
  copy of that list. It never silently drops the list's rules.
- **Never resolve list hosts through ourselves.** List fetches use the
  configured bootstrap resolver (or the system resolver when none is set).
- **Fail closed at startup, fail open on refresh.** A config error stops
  startup. A refresh failure is logged, counted in metrics, and keeps
  serving the previous rules.
- **Rule storage is reversed-domain FSTs** (the `fst` crate): immutable, built
  whole on each refresh and swapped atomically, behind the `NameIndex` trait.
  Hash sets stay only as a test oracle.
- **Container-friendly by default:** runs as non-root, binds ports ≥ 1024
  unless configured otherwise, needs no writable root filesystem (the
  on-disk list cache is optional and points at a configured directory), and
  logs to stderr.
- **Rust:** stable toolchain pinned in `rust-toolchain.toml`. `cargo fmt`,
  `cargo clippy -- -D warnings` and `cargo test` pass on every change. No
  `unsafe`. rustls, not OpenSSL.
- **Releases are tags** (`vX.Y.Z`) on an immutable commit, created only by
  merging the release-please `chore: release X.Y.Z` PR. Never bump versions
  or push release tags by hand. The release builds and publishes the image
  and the Helm chart (OCI) to `ghcr.io/backend-bunny`. See the
  `add-release-packaging` change for details.
- **Conventional Commits.** PR titles and commit subjects are
  `type(scope)!: subject`; CI enforces it because they set the version.
- **Never push to `main` directly.** Branch, PR, `ci-ok` green. Agents may
  merge their own PRs through GitHub (squash, keeping the Conventional
  Commit title) once every required check has passed and the change meets
  its spec. Release PRs (`chore: release X.Y.Z`) are the exception: a human
  merges those, because merging one publishes.
- **Pin everything.** Actions by full SHA with a version comment, the
  toolchain in `devbox.json` + `devbox.lock`, crates in `Cargo.lock`. CI and
  local tasks call the same `hack/` scripts.
- **Packaging:** melange + apko only, **no Dockerfiles**. The binary is
  **statically linked against musl**: built in an Alpine melange environment
  (Wolfi's Rust targets glibc only), with a distroless Wolfi runtime image
  holding the binary and CA certificates, and no libc. It runs as uid/gid
  65532.
- **Throwaway melange keys:** generate one per build, sign, hand the public
  half to apko (`--keyring-append`), discard both. Never commit or store a
  package signing key.
- **Keyless signing only:** cosign with the CI's OIDC identity. Sign and
  attest the image **index** digest and the chart digest (SBOM, provenance,
  notices). No long-lived signing keys.
- **Licenses travel with the binary:** third-party notices are generated
  from the vendored crates, shipped in the image and attested. A license
  outside `deny.toml`'s allowlist fails CI.
- **Lockfiles change only by review:** `Cargo.lock` and `apko.lock.json` are
  committed. Builds use `--locked` / `--lockfile`, and nothing relocks
  silently in CI.
- **License: `MIT OR Apache-2.0`.** Keep `LICENSE-MIT` and `LICENSE-APACHE`
  and the SPDX expression in `Cargo.toml` and the image annotation in step.
- **Config is one TOML file** (`serde`, `deny_unknown_fields`), with rules
  written as lines (`block wildcard *.x`) inside it.
- Don't name private consumers, clusters or networks in this repo's docs.

## Open decisions

None at the moment. Record new ones here with a proposal, and remove them once
the answer is in the relevant spec or design.
