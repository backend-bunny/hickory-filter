# Changelog

## [0.2.0](https://github.com/backend-bunny/hickory-filter/compare/v0.1.1...v0.2.0) (2026-11-27)


### Features

* **recursion:** add a recursive resolution mode ([#52](https://github.com/backend-bunny/hickory-filter/issues/52)) ([1fe1aac](https://github.com/backend-bunny/hickory-filter/commit/1fe1aac3464c5fa464327aa219332b9daeb6109a))

## [0.1.1](https://github.com/backend-bunny/hickory-filter/compare/v0.1.0...v0.1.1) (2026-11-27)


### Bug Fixes

* **deps:** update build toolchain ([#42](https://github.com/backend-bunny/hickory-filter/issues/42)) ([407fa12](https://github.com/backend-bunny/hickory-filter/commit/407fa12f67eb243dfc2b81f344f1fbf70e866eb5))
* **image:** attest an SBOM that lists the image's packages and the binary's crates ([#45](https://github.com/backend-bunny/hickory-filter/issues/45)) ([0deeace](https://github.com/backend-bunny/hickory-filter/commit/0deeace3119c824e8efd4dd3e9d9f0b9094a1a8b))

## 0.1.0 (2026-11-27)


### Features

* add the Cargo project skeleton with --version ([#4](https://github.com/backend-bunny/hickory-filter/issues/4)) ([9c5d273](https://github.com/backend-bunny/hickory-filter/commit/9c5d273933dfb4bc752c779fddb51e7e5a053ddd))
* add the immutable rule set with allow-first matching and an atomic swap ([#10](https://github.com/backend-bunny/hickory-filter/issues/10)) ([34675da](https://github.com/backend-bunny/hickory-filter/commit/34675da917509a3737667fd3d406a6270e565e55))
* add the request-level filter handler spike for hickory-server 0.26.4 ([#5](https://github.com/backend-bunny/hickory-filter/issues/5)) ([7f958c4](https://github.com/backend-bunny/hickory-filter/commit/7f958c4b80b011665d37cef2177b7fb70bf5ab91))
* answer blocked queries per the configured response mode ([#14](https://github.com/backend-bunny/hickory-filter/issues/14)) ([820234f](https://github.com/backend-bunny/hickory-filter/commit/820234f2eec6b0da38b0b885ea9c8fdd2bffee4a))
* cache fetched lists on disk and use the cache at startup ([#19](https://github.com/backend-bunny/hickory-filter/issues/19)) ([b33f542](https://github.com/backend-bunny/hickory-filter/commit/b33f542007c7c0844003f876f8e1c7c5b65bd900))
* **chart:** add the Helm chart with tests and a CI job ([#33](https://github.com/backend-bunny/hickory-filter/issues/33)) ([245ec06](https://github.com/backend-bunny/hickory-filter/commit/245ec069c3cbaa8bbcbd11772f5efcce9b457c9b))
* **dashboard:** add a Grafana dashboard for the metrics and query log ([#31](https://github.com/backend-bunny/hickory-filter/issues/31)) ([4c71779](https://github.com/backend-bunny/hickory-filter/commit/4c71779d9e7671bc68f2c808dde70f91c83d8625))
* expose query, rule set, list, upstream and cache metrics ([#24](https://github.com/backend-bunny/hickory-filter/issues/24)) ([3c1a03a](https://github.com/backend-bunny/hickory-filter/commit/3c1a03a672c9d95fa08f8782a81cde7a58741611))
* fetch remote lists over HTTP(S) with rustls ([#16](https://github.com/backend-bunny/hickory-filter/issues/16)) ([13e96b3](https://github.com/backend-bunny/hickory-filter/commit/13e96b3ca6c1d098c57863ac38ba8ac798360323))
* forward unblocked queries to the configured upstreams ([#20](https://github.com/backend-bunny/hickory-filter/issues/20)) ([c7e17b2](https://github.com/backend-bunny/hickory-filter/commit/c7e17b245465723dd6a3839a1d0e90761524477f))
* load and validate the TOML configuration with --config and --check ([#7](https://github.com/backend-bunny/hickory-filter/issues/7)) ([66e0234](https://github.com/backend-bunny/hickory-filter/commit/66e02340f79380f2a8e06458c2cb9dbe5113ccd6))
* load file lists and keep each list's last good state ([#15](https://github.com/backend-bunny/hickory-filter/issues/15)) ([4011e77](https://github.com/backend-bunny/hickory-filter/commit/4011e7762675284084403686c70c08ba03fc83e2))
* parse rule lines and list files, and validate rules at config load ([#9](https://github.com/backend-bunny/hickory-filter/issues/9)) ([7443997](https://github.com/backend-bunny/hickory-filter/commit/744399710db9d453d0a7dfd221f9c444cea10195))
* refresh lists on a schedule and swap the rule set once per cycle ([#18](https://github.com/backend-bunny/hickory-filter/issues/18)) ([29974f8](https://github.com/backend-bunny/hickory-filter/commit/29974f8fe2a0c9961a08e71bde685f07bc70241e))
* resolve list hosts only through the bootstrap resolver ([#17](https://github.com/backend-bunny/hickory-filter/issues/17)) ([eaae626](https://github.com/backend-bunny/hickory-filter/commit/eaae62660179bf8151e21cdcbfa1ef0b8af8c093))
* serve /health, /ready and /metrics on an HTTP endpoint ([#23](https://github.com/backend-bunny/hickory-filter/issues/23)) ([0f97f8b](https://github.com/backend-bunny/hickory-filter/commit/0f97f8b78c7c3a2fc51edb99876f5f69125ed444))
* serve DNS on UDP and TCP and shut down gracefully ([#21](https://github.com/backend-bunny/hickory-filter/issues/21)) ([8270f91](https://github.com/backend-bunny/hickory-filter/commit/8270f9140251e87310737976d08929ec75cd2739))
* store rule names in reversed-domain FSTs ([#13](https://github.com/backend-bunny/hickory-filter/issues/13)) ([be257ae](https://github.com/backend-bunny/hickory-filter/commit/be257aee79991ef110894d9646f398f290209e0d))
* structured logging with an opt-in query log ([#25](https://github.com/backend-bunny/hickory-filter/issues/25)) ([313c01c](https://github.com/backend-bunny/hickory-filter/commit/313c01c74958c94abf7fd59ecc3ceede1a23d752))


### Build and Image

* add pinned devbox toolchain, Taskfile and shared hack scripts ([#2](https://github.com/backend-bunny/hickory-filter/issues/2)) ([82130ab](https://github.com/backend-bunny/hickory-filter/commit/82130ab93a9d6a0e466bb77c9cdfa75f5e3c8ddc))
* **image:** add the apko runtime image, lock handling and smoke test ([#35](https://github.com/backend-bunny/hickory-filter/issues/35)) ([4c9c9fb](https://github.com/backend-bunny/hickory-filter/commit/4c9c9fb526477ec09bd084ff10ea55362e381dc2))
* **image:** generate and ship the third-party notices ([#36](https://github.com/backend-bunny/hickory-filter/issues/36)) ([59eab5a](https://github.com/backend-bunny/hickory-filter/commit/59eab5aab4e7f9e29037d4a98db7d4f9d88c03c0))
* **package:** build a static musl APK with melange ([#34](https://github.com/backend-bunny/hickory-filter/issues/34)) ([3ea1c8f](https://github.com/backend-bunny/hickory-filter/commit/3ea1c8f2d552d009eb931c30fad29ea3da0c6a05))
* require the release compiler's Rust version and update the agent guide ([#40](https://github.com/backend-bunny/hickory-filter/issues/40)) ([22d9742](https://github.com/backend-bunny/hickory-filter/commit/22d974253abcb9090f19774dc7c49bad740baf61))
