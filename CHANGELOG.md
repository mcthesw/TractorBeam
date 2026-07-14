# Changelog

## [0.2.2](https://github.com/mcthesw/TractorBeam/compare/v0.2.1...v0.2.2) (2026-07-14)


### Features

* **client:** add input delay control API ([46a7984](https://github.com/mcthesw/TractorBeam/commit/46a7984c5b2746710fb0de5b712c2e1c6a5aade0))
* **client:** add Relay v2 transport recovery ([443a1ec](https://github.com/mcthesw/TractorBeam/commit/443a1ece5fa520f951819e03cd4ca23da1d53bdb))
* **client:** expose actionable smoothness evidence ([1967457](https://github.com/mcthesw/TractorBeam/commit/1967457bdd0ca44f809f1d27238bf944b969e431))
* **client:** simplify rooms and troubleshooting workflows ([0dfdce5](https://github.com/mcthesw/TractorBeam/commit/0dfdce5cec6aeec1f64d894fab8e567a5b195267))
* **gui:** add input delay settings controls ([75e24be](https://github.com/mcthesw/TractorBeam/commit/75e24be69c9123d063da5f08ab181051d2232085))
* **native-hook:** add input delay control endpoint ([3d5ef06](https://github.com/mcthesw/TractorBeam/commit/3d5ef06ef72066d72861acea4dcd8c6e71379e73))
* **network:** add peer room path quality ([2db4699](https://github.com/mcthesw/TractorBeam/commit/2db46990434ba9928ccb00e3797cbe3cd18c6212))
* **protocol:** define the Relay Protocol v2 wire contract ([986a13a](https://github.com/mcthesw/TractorBeam/commit/986a13ac350e9155c352da9cb8160388a6e3f79d))
* **relay:** export OpenTelemetry signals ([d2e03c0](https://github.com/mcthesw/TractorBeam/commit/d2e03c057e5f152c3d06e634c28eb0c40c8a7071))
* **relay:** implement resumable Relay Protocol v2 sessions ([330acaa](https://github.com/mcthesw/TractorBeam/commit/330acaa74eb57e88294cffe3d74ad75373d2a6e6))
* **relay:** make traffic limits configurable ([98e0a8b](https://github.com/mcthesw/TractorBeam/commit/98e0a8b5e7c0195a0eae4972ba0b47ed5781505c))
* **relay:** support structured JSON logs ([1a2750d](https://github.com/mcthesw/TractorBeam/commit/1a2750d2a1fa5ac9d32d4e8ac5ed7a9c559ecc8a))


### Bug Fixes

* **native-hook:** use fixed input delay offset ([1c1008b](https://github.com/mcthesw/TractorBeam/commit/1c1008bc8b67ce7c675cdae20e6e1ee120616229))


### Code Refactoring

* **client:** isolate application runtime and local IPC adapter ([d29cc81](https://github.com/mcthesw/TractorBeam/commit/d29cc81087cf646cfc8133a39443e72f6668ca68))
* **client:** move config tests into a focused module ([63ca3f8](https://github.com/mcthesw/TractorBeam/commit/63ca3f86da2fde7500cebdeb42fd4b4704ba41e0))
* **gui:** present smoothness through focused application modules ([419703a](https://github.com/mcthesw/TractorBeam/commit/419703a17a8efcb87b2cf1b6b620895e46ef95a3))
* **injector:** isolate Windows elevation helpers ([4f1d0e3](https://github.com/mcthesw/TractorBeam/commit/4f1d0e38304b0a3d74f29d48cb054aece11fbeb8))
* **ipc:** split codec and connection responsibilities ([e1b66c7](https://github.com/mcthesw/TractorBeam/commit/e1b66c72b3ad2ca3e2cb7d8811f5dd0f5109a5fd))
* **native-hook:** move local IPC to owned worker ([97ffcf5](https://github.com/mcthesw/TractorBeam/commit/97ffcf5bd41e62dd55500763e7de30d9e7d96cfe))
* **protocol:** remove Relay Protocol v1 ([13946fc](https://github.com/mcthesw/TractorBeam/commit/13946fc37853446eb5614294c0963a0b359502c7))
* **relay:** isolate protocol v1 and add size metrics ([d616d0d](https://github.com/mcthesw/TractorBeam/commit/d616d0d302b8b07c51af184be99932717abda00c))
* **relay:** remove v1 runtime and simplify observability ([d9a95b9](https://github.com/mcthesw/TractorBeam/commit/d9a95b9840e4f518dc9e510a0e49283438d537e2))


### Documentation

* define input delay terminology ([77f8848](https://github.com/mcthesw/TractorBeam/commit/77f8848c95245dece8c91d4426b53d1bfadb0938))
* document relay and client quality contracts ([af469f4](https://github.com/mcthesw/TractorBeam/commit/af469f42a5160bf467982bb004e672ec21a1bb03))
* document runtime and protocol boundaries ([34e6040](https://github.com/mcthesw/TractorBeam/commit/34e60409a5b05f8fa934d7ea6bc051c5e3465869))
* **protocol:** record the Relay Protocol v2 decisions ([1fe0e9f](https://github.com/mcthesw/TractorBeam/commit/1fe0e9ffc4e9f0cb602d6e0d6a131165f36327d9))
* **protocol:** sync Relay v2 architecture and contracts ([d7f1710](https://github.com/mcthesw/TractorBeam/commit/d7f17101d95e8f5a38031873b56cd01a03f24d0a))
* **relay:** document observability operations ([9858c63](https://github.com/mcthesw/TractorBeam/commit/9858c6381678a08ce573a7e4788d50da4d6bbcac))
* **relay:** document structured log collection ([09338b6](https://github.com/mcthesw/TractorBeam/commit/09338b6f67c5d486cab844460a9d488000bf107f))
* **spec:** codify the Relay Protocol v2 contract ([c640129](https://github.com/mcthesw/TractorBeam/commit/c6401296fe9dfc3b534fe0af516eb71d49b0d543))

## [0.2.1](https://github.com/mcthesw/TractorBeam/compare/v0.2.0...v0.2.1) (2026-07-05)


### Features

* **protocol:** add relay admission metadata ([254e5a0](https://github.com/mcthesw/TractorBeam/commit/254e5a0232b88e715066252a68c034122d0f002c))
* **relay:** add public relay config defaults ([0d71766](https://github.com/mcthesw/TractorBeam/commit/0d71766be6c0e04e7b2695bc3a3549a9f17bf4fc))
* **relay:** add public traffic safety limits ([3bcc3f4](https://github.com/mcthesw/TractorBeam/commit/3bcc3f4099554267cf2224d3a09688147cf48f27))
* **relay:** enforce client admission metadata ([f7badeb](https://github.com/mcthesw/TractorBeam/commit/f7badebba9b8a335adfc0dcf7bc01e9f3005bb6b))
* **relay:** require proof of work for joins ([44093e0](https://github.com/mcthesw/TractorBeam/commit/44093e0a032bf9bd2d59704f62b294183e674ffb))
* **relay:** require room admission material ([e7ab4e3](https://github.com/mcthesw/TractorBeam/commit/e7ab4e3d7d833f3a34cd27f9492bead44a9c5bcb))


### Bug Fixes

* **core:** report initial room peers ([f105d92](https://github.com/mcthesw/TractorBeam/commit/f105d92c209ed3cb8aff864b730ddcbcc6f8d7a4))
* **gui:** keep startup responsive during relay init ([f5b1e9d](https://github.com/mcthesw/TractorBeam/commit/f5b1e9dbe4e3aeff53b634cb29c9007e1782d73a))
* **relay:** prune stale public peer state ([6243a46](https://github.com/mcthesw/TractorBeam/commit/6243a465bf6b28f480e82867844d83e850970231))

## [0.2.0](https://github.com/mcthesw/TractorBeam/compare/v0.1.1...v0.2.0) (2026-07-04)


### ⚠ BREAKING CHANGES

* removes the internal-test cargo feature and all code gated behind it, including the BB1. join code, self-test workflow, report packaging/upload, and InternalTestConfig. The closed-test report flow is no longer needed; a new join code (relay + room only) will replace it in subsequent commits.
* remove UDP FEC transport profile

### Features

* **core:** add lightweight relay ping probes ([18781de](https://github.com/mcthesw/TractorBeam/commit/18781deb96b9fdbbb83c57df954ef169ab98ad76))
* **core:** add shareable join codes ([62361aa](https://github.com/mcthesw/TractorBeam/commit/62361aaaca136751a18626cff0577454ff87bec1))
* **core:** persist room and selected steam identity ([5446f5c](https://github.com/mcthesw/TractorBeam/commit/5446f5cd18324c0534f3656156573dd4f5357c03))
* **core:** track room peer view from relay updates ([4f71491](https://github.com/mcthesw/TractorBeam/commit/4f714912c38f77f4b7ba6715a6ccdde951b9ab57))
* **gui:** default to recent Steam identity ([68aa268](https://github.com/mcthesw/TractorBeam/commit/68aa268332b498d6f0dc23a314a9795db59a1e86))
* **gui:** hide release console window ([b57cbc4](https://github.com/mcthesw/TractorBeam/commit/b57cbc427e7551e7a043b8d048c41f9a18ea0c14))
* **gui:** move UI translations to rust-i18n YAML ([ba94b87](https://github.com/mcthesw/TractorBeam/commit/ba94b8735afe9f5a873e51e93b11b58bd50894b7))
* **gui:** remove redundant name field ([254c768](https://github.com/mcthesw/TractorBeam/commit/254c7689c13b04ed65fe5e5c6508c73de273b535))
* **gui:** rewrite to five-page shell with home, settings, stats, log, about ([082c108](https://github.com/mcthesw/TractorBeam/commit/082c108f72e579cec56fa7c9321593d57bffa44c))
* **gui:** show relay latency in selection ([f3d1731](https://github.com/mcthesw/TractorBeam/commit/f3d17315673487d6931f5c7aff959e5d395b4f3f))
* **injector:** retry native hook injection with elevation on access denied ([9b24c4a](https://github.com/mcthesw/TractorBeam/commit/9b24c4aff619973ed2cf4ac2b6ca17b354dc1070))
* **protocol:** add peer metadata and room update control messages ([3f05a32](https://github.com/mcthesw/TractorBeam/commit/3f05a32c58100c18fcc1bb305fcf8ed3b2d177f2))
* **relay:** broadcast room peer updates ([dbd06b0](https://github.com/mcthesw/TractorBeam/commit/dbd06b0a29ddc16b01756eca1289d0ee0f0a0bf1))
* remove UDP FEC transport profile ([f367c23](https://github.com/mcthesw/TractorBeam/commit/f367c237a82345b39d19ac010e0a90f62a25e4b8))


### Bug Fixes

* **client:** make native hook startup diagnosable ([e581c88](https://github.com/mcthesw/TractorBeam/commit/e581c88d9ee97459ad411297078a071e0e2a1881))


### Code Refactoring

* **gui:** call rust-i18n translations directly ([5047390](https://github.com/mcthesw/TractorBeam/commit/5047390b7089e28a1ebc0953d009c538d69655b0))
* **injector:** remove prototype fallback artifacts ([985ee14](https://github.com/mcthesw/TractorBeam/commit/985ee149460e05e00622904c1089adcce442e0c3))


### Miscellaneous

* remove internal-test feature and closed-test report flow ([4299b46](https://github.com/mcthesw/TractorBeam/commit/4299b46e49380f333eeee0e92be838a8d731bae8))

## [0.1.1](https://github.com/mcthesw/TractorBeam/compare/v0.1.0...v0.1.1) (2026-06-30)


### Features

* add closed-test relay controls ([5093d72](https://github.com/mcthesw/TractorBeam/commit/5093d726da7d2d99b8ae5fb32d46fc628726aeb5))
* add readiness probe matrix ([f389f01](https://github.com/mcthesw/TractorBeam/commit/f389f010197c6dffd2a20b85d5ead98d5482efe6))
* add relay TCP egress telemetry ([3e548d4](https://github.com/mcthesw/TractorBeam/commit/3e548d47e433169ab6beac25c7a9a8c4697772ca))
* **client:** collect session runtime health ([6f37e85](https://github.com/mcthesw/TractorBeam/commit/6f37e85d74b2cc8c1293ed9f3455d3a6c4ded84a))
* **core:** add async bridge transport runtime ([f455f91](https://github.com/mcthesw/TractorBeam/commit/f455f91e217855e50cfe7b81c15b4ce34fc7d48f))
* **core:** add bridge runtime and diagnostics ([58e6cb2](https://github.com/mcthesw/TractorBeam/commit/58e6cb22fa3e573aabb7d21a397df506179a0c82))
* **core:** add internal-test report packaging and upload ([3cc7587](https://github.com/mcthesw/TractorBeam/commit/3cc7587d3c8324270e5dd47497080a2a716ccfcf))
* expose build provenance ([b2c3405](https://github.com/mcthesw/TractorBeam/commit/b2c340561922a4e768f77fdef6a0f6658d17800f))
* **gui:** add player session controls ([67950f4](https://github.com/mcthesw/TractorBeam/commit/67950f4ed498700ed2d60c1a57cf2d8101317a35))
* **gui:** add self-serve internal test page ([fc4edd4](https://github.com/mcthesw/TractorBeam/commit/fc4edd474918a85862dbcdce6c9fd3fa0ed695e2))
* **gui:** show session quality summary ([7ecca6b](https://github.com/mcthesw/TractorBeam/commit/7ecca6b73f255196b8f62984b34306809099fdf4))
* **native-hook:** add Rust Steam hook and injector ([7c5c9d8](https://github.com/mcthesw/TractorBeam/commit/7c5c9d8d6c23c01196e9d611e7096bdd42322072))
* **protocol:** add relay health ping pong ([c3550b4](https://github.com/mcthesw/TractorBeam/commit/c3550b4e8df034aff1c62f85ad86ce9fd376dc3f))
* **relay:** add deployable UDP relay ([dd99b4f](https://github.com/mcthesw/TractorBeam/commit/dd99b4ff2153c208e42cb1f96f0cb16c722af6fe))
* **relay:** add room-level observability metrics ([69fa629](https://github.com/mcthesw/TractorBeam/commit/69fa629627a6be625d5b04e37e8b43430aa35308))
* **relay:** support tcp datagram transport ([4f20dd4](https://github.com/mcthesw/TractorBeam/commit/4f20dd4d5d93877710cd67fd852523fddace27dc))
* scaffold bridge workspace ([d908e2b](https://github.com/mcthesw/TractorBeam/commit/d908e2bcae248e64dad97aef86b07cc7ecf37643))
* write GUI client logs with tracing ([aa3e8a9](https://github.com/mcthesw/TractorBeam/commit/aa3e8a93ecf77e911fdcf73773bcae7683e429eb))


### Bug Fixes

* avoid non-windows injector warning ([44b01b1](https://github.com/mcthesw/TractorBeam/commit/44b01b1739b26bb494799d28168330295e9527ea))
* report injector failure steps ([77ebf4f](https://github.com/mcthesw/TractorBeam/commit/77ebf4f06b546a90cb17928b6b877bcd21bf279a))


### Code Refactoring

* clarify client runtime boundaries ([6903f86](https://github.com/mcthesw/TractorBeam/commit/6903f863c16a1c218d87694deb7e0b7cf9070bb3))
* **config:** drop startup room defaults and prefer tcp ([4ddae03](https://github.com/mcthesw/TractorBeam/commit/4ddae03eb292ca2a1da5da3d5d07a09a6f877298))
* **workspace:** consolidate bridge crate layout ([db9eec2](https://github.com/mcthesw/TractorBeam/commit/db9eec2f24e5616640c83f318f0921fa8236d0bd))


### Documentation

* align Phase 1 roadmap and relay guidance ([87e7ffc](https://github.com/mcthesw/TractorBeam/commit/87e7ffc367c95383f554cb1e1251d0127ad9301d))
* clarify relay transport terminology ([4632615](https://github.com/mcthesw/TractorBeam/commit/46326150214db9d7cd18c1dd35b10c48faa03538))
