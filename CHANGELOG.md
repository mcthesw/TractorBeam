# Changelog

## [0.1.1](https://github.com/mcthesw/Basement-Bridge/compare/v0.1.0...v0.1.1) (2026-06-30)


### Features

* add closed-test relay controls ([5093d72](https://github.com/mcthesw/Basement-Bridge/commit/5093d726da7d2d99b8ae5fb32d46fc628726aeb5))
* add readiness probe matrix ([f389f01](https://github.com/mcthesw/Basement-Bridge/commit/f389f010197c6dffd2a20b85d5ead98d5482efe6))
* add relay TCP egress telemetry ([3e548d4](https://github.com/mcthesw/Basement-Bridge/commit/3e548d47e433169ab6beac25c7a9a8c4697772ca))
* **client:** collect session runtime health ([6f37e85](https://github.com/mcthesw/Basement-Bridge/commit/6f37e85d74b2cc8c1293ed9f3455d3a6c4ded84a))
* **core:** add async bridge transport runtime ([f455f91](https://github.com/mcthesw/Basement-Bridge/commit/f455f91e217855e50cfe7b81c15b4ce34fc7d48f))
* **core:** add bridge runtime and diagnostics ([58e6cb2](https://github.com/mcthesw/Basement-Bridge/commit/58e6cb22fa3e573aabb7d21a397df506179a0c82))
* **core:** add internal-test report packaging and upload ([3cc7587](https://github.com/mcthesw/Basement-Bridge/commit/3cc7587d3c8324270e5dd47497080a2a716ccfcf))
* expose build provenance ([b2c3405](https://github.com/mcthesw/Basement-Bridge/commit/b2c340561922a4e768f77fdef6a0f6658d17800f))
* **gui:** add player session controls ([67950f4](https://github.com/mcthesw/Basement-Bridge/commit/67950f4ed498700ed2d60c1a57cf2d8101317a35))
* **gui:** add self-serve internal test page ([fc4edd4](https://github.com/mcthesw/Basement-Bridge/commit/fc4edd474918a85862dbcdce6c9fd3fa0ed695e2))
* **gui:** show session quality summary ([7ecca6b](https://github.com/mcthesw/Basement-Bridge/commit/7ecca6b73f255196b8f62984b34306809099fdf4))
* **native-hook:** add Rust Steam hook and injector ([7c5c9d8](https://github.com/mcthesw/Basement-Bridge/commit/7c5c9d8d6c23c01196e9d611e7096bdd42322072))
* **protocol:** add relay health ping pong ([c3550b4](https://github.com/mcthesw/Basement-Bridge/commit/c3550b4e8df034aff1c62f85ad86ce9fd376dc3f))
* **relay:** add deployable UDP relay ([dd99b4f](https://github.com/mcthesw/Basement-Bridge/commit/dd99b4ff2153c208e42cb1f96f0cb16c722af6fe))
* **relay:** add room-level observability metrics ([69fa629](https://github.com/mcthesw/Basement-Bridge/commit/69fa629627a6be625d5b04e37e8b43430aa35308))
* **relay:** support tcp datagram transport ([4f20dd4](https://github.com/mcthesw/Basement-Bridge/commit/4f20dd4d5d93877710cd67fd852523fddace27dc))
* scaffold bridge workspace ([d908e2b](https://github.com/mcthesw/Basement-Bridge/commit/d908e2bcae248e64dad97aef86b07cc7ecf37643))
* write GUI client logs with tracing ([aa3e8a9](https://github.com/mcthesw/Basement-Bridge/commit/aa3e8a93ecf77e911fdcf73773bcae7683e429eb))


### Bug Fixes

* avoid non-windows injector warning ([44b01b1](https://github.com/mcthesw/Basement-Bridge/commit/44b01b1739b26bb494799d28168330295e9527ea))
* report injector failure steps ([77ebf4f](https://github.com/mcthesw/Basement-Bridge/commit/77ebf4f06b546a90cb17928b6b877bcd21bf279a))


### Code Refactoring

* clarify client runtime boundaries ([6903f86](https://github.com/mcthesw/Basement-Bridge/commit/6903f863c16a1c218d87694deb7e0b7cf9070bb3))
* **config:** drop startup room defaults and prefer tcp ([4ddae03](https://github.com/mcthesw/Basement-Bridge/commit/4ddae03eb292ca2a1da5da3d5d07a09a6f877298))
* **workspace:** consolidate bridge crate layout ([db9eec2](https://github.com/mcthesw/Basement-Bridge/commit/db9eec2f24e5616640c83f318f0921fa8236d0bd))


### Documentation

* align Phase 1 roadmap and relay guidance ([87e7ffc](https://github.com/mcthesw/Basement-Bridge/commit/87e7ffc367c95383f554cb1e1251d0127ad9301d))
* clarify relay transport terminology ([4632615](https://github.com/mcthesw/Basement-Bridge/commit/46326150214db9d7cd18c1dd35b10c48faa03538))
