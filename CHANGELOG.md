# Changelog

## [0.3.1](https://github.com/mcthesw/TractorBeam/compare/v0.3.0...v0.3.1) (2026-07-19)


### Code Refactoring

* **config:** use a single portable client config ([84c0033](https://github.com/mcthesw/TractorBeam/commit/84c0033416d6972c1f3fc385da874265f2618e2f))
* **logging:** unify portable logs and diagnostics ([afd435c](https://github.com/mcthesw/TractorBeam/commit/afd435c01e35fd7ce3b02957700673fb9a33d816))
* reduce infrastructure maintenance surface ([df7e5f2](https://github.com/mcthesw/TractorBeam/commit/df7e5f2bd38829b508841a9cb4cd48590f3b991a))


### Documentation

* unify diagnostics and testing terminology ([7d4dfda](https://github.com/mcthesw/TractorBeam/commit/7d4dfdac845489df3b6f6799ded9944e78961c3d))

## [0.3.0](https://github.com/mcthesw/TractorBeam/compare/v0.2.1...v0.3.0) (2026-07-15)


### ⚠ BREAKING CHANGES

* removes the internal-test cargo feature and all code gated behind it, including the BB1. join code, self-test workflow, report packaging/upload, and InternalTestConfig. The closed-test report flow is no longer needed; a new join code (relay + room only) will replace it in subsequent commits.
* remove UDP FEC transport profile

### Features

* add closed-test relay controls ([fef77b2](https://github.com/mcthesw/TractorBeam/commit/fef77b22fb83f96a3944753041578cf00c7ba7a1))
* add readiness probe matrix ([a8d3377](https://github.com/mcthesw/TractorBeam/commit/a8d3377f670d7329cc035470639d32e3401e9db9))
* add relay TCP egress telemetry ([a90a90d](https://github.com/mcthesw/TractorBeam/commit/a90a90d0b3f0e0fcb4951b33fd87e8b65499ee2f))
* **client:** add direct LAN join codes ([d99932f](https://github.com/mcthesw/TractorBeam/commit/d99932f49a304f429844ed09bd379371c8bd9477))
* **client:** add direct peer path nomination ([92e8986](https://github.com/mcthesw/TractorBeam/commit/92e8986ab8d4761bed35c3c142e7f4387e7cdfb5))
* **client:** add input delay control API ([b5f470c](https://github.com/mcthesw/TractorBeam/commit/b5f470c8298b40cd127da4be2563902a60d0cfef))
* **client:** add LAN adapter and invitation lifecycle ([d2aa308](https://github.com/mcthesw/TractorBeam/commit/d2aa3081c832f18cea2658cdaa5871a8d6f50953))
* **client:** add peer membership convergence ([b5ada19](https://github.com/mcthesw/TractorBeam/commit/b5ada19fb3d75c7cece2e02f5a75dc54aa62eb49))
* **client:** add Relay v2 transport recovery ([a9eb631](https://github.com/mcthesw/TractorBeam/commit/a9eb6311d40f8c7c02193fa093e187dd23253716))
* **client:** collect session runtime health ([8fac325](https://github.com/mcthesw/TractorBeam/commit/8fac325f0c1d316c379ad5c89a085733292e7ed3))
* **client:** expose actionable smoothness evidence ([edb7241](https://github.com/mcthesw/TractorBeam/commit/edb72413846af2a2848b83f436f7ba4b3c70b6c4))
* **client:** expose direct path diagnostics ([ef5693c](https://github.com/mcthesw/TractorBeam/commit/ef5693caefbe6ca977242d6867d0cb7613595ca5))
* **client:** route gameplay over direct UDP paths ([68e9263](https://github.com/mcthesw/TractorBeam/commit/68e92630f0b28bedc0622b834896f467d204b7dd))
* **client:** simplify rooms and troubleshooting workflows ([423e3b1](https://github.com/mcthesw/TractorBeam/commit/423e3b182f8835818793fc658f7569d9be9a6781))
* **core:** add async bridge transport runtime ([9b14e44](https://github.com/mcthesw/TractorBeam/commit/9b14e4437c46f717a900a57405563a21c067e576))
* **core:** add bridge runtime and diagnostics ([dd35f1e](https://github.com/mcthesw/TractorBeam/commit/dd35f1e34c9e33965667cb3c4338f442a9e3581d))
* **core:** add internal-test report packaging and upload ([c5dfc0b](https://github.com/mcthesw/TractorBeam/commit/c5dfc0bb734b8e154670acb7d3347a6f1918fb3a))
* **core:** add lightweight relay ping probes ([d2dc9d9](https://github.com/mcthesw/TractorBeam/commit/d2dc9d904b7cb3f846054d38597e5856322f3317))
* **core:** add shareable join codes ([b314f94](https://github.com/mcthesw/TractorBeam/commit/b314f9488d88387b766bdbb4837b72019c9aa5a5))
* **core:** persist room and selected steam identity ([4a30013](https://github.com/mcthesw/TractorBeam/commit/4a30013e6b3cc1f85838c6ac2c76786caf063734))
* **core:** track room peer view from relay updates ([b22d2cf](https://github.com/mcthesw/TractorBeam/commit/b22d2cf27bfb86c86baa7677c82e7f72789571ab))
* expose build provenance ([84d967d](https://github.com/mcthesw/TractorBeam/commit/84d967d057415d5fd0d55d01962d280fadd24706))
* **gui:** add input delay settings controls ([03aab39](https://github.com/mcthesw/TractorBeam/commit/03aab3935aab7015bfd8f65df9becd7f7aeecbb7))
* **gui:** add LAN invite and join flows ([8e3b9d8](https://github.com/mcthesw/TractorBeam/commit/8e3b9d8d5040eb7037103d86ab08f8300c50befe))
* **gui:** add player session controls ([a404983](https://github.com/mcthesw/TractorBeam/commit/a404983424e5044e0e28588933b46624be83b52f))
* **gui:** add self-serve internal test page ([d494a76](https://github.com/mcthesw/TractorBeam/commit/d494a7676bafbcef804872aef25c81135a79aadd))
* **gui:** default to recent Steam identity ([47406a2](https://github.com/mcthesw/TractorBeam/commit/47406a23793af1af8f57f7e9d350e92760fa2681))
* **gui:** hide release console window ([6b6942f](https://github.com/mcthesw/TractorBeam/commit/6b6942f1422a0af428db25ee50b8ebcd4cf2242a))
* **gui:** move UI translations to rust-i18n YAML ([da056c7](https://github.com/mcthesw/TractorBeam/commit/da056c76c41597f924a085010585eb4505b198d7))
* **gui:** polish acknowledgements and copy ([07d5e2a](https://github.com/mcthesw/TractorBeam/commit/07d5e2aeba6521eeec8a3814737c34570546d841))
* **gui:** remove redundant name field ([68cad04](https://github.com/mcthesw/TractorBeam/commit/68cad0495a48b4fc9d3c5a227bac992e2e534bf0))
* **gui:** rewrite to five-page shell with home, settings, stats, log, about ([9b7c097](https://github.com/mcthesw/TractorBeam/commit/9b7c0977b3db50c5f7a96d1a727395acaf32d93e))
* **gui:** show LAN adapter addresses ([4986f22](https://github.com/mcthesw/TractorBeam/commit/4986f226b6ce3e77a24bf6a8c241444bbfecd2b4))
* **gui:** show relay latency in selection ([bb71710](https://github.com/mcthesw/TractorBeam/commit/bb7171013eaeb13f3c6f102a8c05f112e7b89a8a))
* **gui:** show session quality summary ([68a911e](https://github.com/mcthesw/TractorBeam/commit/68a911e1acd34040c5b48c51f40a0bd2546a0e65))
* **injector:** retry native hook injection with elevation on access denied ([c0ee927](https://github.com/mcthesw/TractorBeam/commit/c0ee9270a233349d18d44314d66e6bba37db15d0))
* **native-hook:** add input delay control endpoint ([5fee93d](https://github.com/mcthesw/TractorBeam/commit/5fee93d102c305857314c707c91474cdb62cc00b))
* **native-hook:** add Rust Steam hook and injector ([cef9681](https://github.com/mcthesw/TractorBeam/commit/cef9681a2443bd72c83162ac911bdf90b15e0bdb))
* **network:** add peer room path quality ([260d36a](https://github.com/mcthesw/TractorBeam/commit/260d36acc232b26e3216aa1f6b811390e0800b03))
* **protocol:** add bounded direct peer wire contracts ([2cc8c25](https://github.com/mcthesw/TractorBeam/commit/2cc8c258d019b88b632cdf2c3ce8c3db96be6c0a))
* **protocol:** add peer metadata and room update control messages ([0bd20bb](https://github.com/mcthesw/TractorBeam/commit/0bd20bbbd147dc90b4a1ac04d606e5de48b66e60))
* **protocol:** add relay admission metadata ([f34ecd3](https://github.com/mcthesw/TractorBeam/commit/f34ecd3ee73e73a019928699e61a3c8ab3e16840))
* **protocol:** add relay health ping pong ([21fb7fa](https://github.com/mcthesw/TractorBeam/commit/21fb7fa4ce952aaabc18642750966a506aa49ec6))
* **protocol:** define the Relay Protocol v2 wire contract ([94fdc9a](https://github.com/mcthesw/TractorBeam/commit/94fdc9a26e5cb1a9c5281cce568c07379f225995))
* **relay:** add deployable UDP relay ([4ebdd4c](https://github.com/mcthesw/TractorBeam/commit/4ebdd4c811b843372e8b8b912a845b4704534e78))
* **relay:** add public relay config defaults ([74143f0](https://github.com/mcthesw/TractorBeam/commit/74143f045ca4e4a55d5fee148048f554a260c973))
* **relay:** add public traffic safety limits ([41d88ff](https://github.com/mcthesw/TractorBeam/commit/41d88ff1daee3da75670ab93318ffc930fe70cbe))
* **relay:** add room-level observability metrics ([267a957](https://github.com/mcthesw/TractorBeam/commit/267a9578f8cfc460f8e9dc4960fd4f946888aa95))
* **relay:** broadcast room peer updates ([f85a0ac](https://github.com/mcthesw/TractorBeam/commit/f85a0ac7bd7522ffcadcad133bb41415b656794a))
* **relay:** enforce client admission metadata ([37bb2b8](https://github.com/mcthesw/TractorBeam/commit/37bb2b8edbb9cb562e20da7e1418a1a74edc9d95))
* **relay:** export OpenTelemetry signals ([4769d4a](https://github.com/mcthesw/TractorBeam/commit/4769d4ab6508f34783e54131c8c89ef989c1f242))
* **relay:** implement resumable Relay Protocol v2 sessions ([320e4d8](https://github.com/mcthesw/TractorBeam/commit/320e4d8d45cfb6950df5dced28e3e178c0923b37))
* **relay:** make traffic limits configurable ([01c4cbf](https://github.com/mcthesw/TractorBeam/commit/01c4cbf3b5e5575765f1b61785143e5ff4dc0091))
* **relay:** require proof of work for joins ([6bc200d](https://github.com/mcthesw/TractorBeam/commit/6bc200ddcf1e9bab873e8c1278b78e160e1b9bfd))
* **relay:** require room admission material ([a4d69ab](https://github.com/mcthesw/TractorBeam/commit/a4d69abe6bb09a787bb7cc2d35c15ed1708baec6))
* **relay:** support structured JSON logs ([fc108f7](https://github.com/mcthesw/TractorBeam/commit/fc108f7bdde8c333479312b4956876512064f646))
* **relay:** support tcp datagram transport ([c09509b](https://github.com/mcthesw/TractorBeam/commit/c09509b08a2cee32eacb576c3e0026a6590ec6a6))
* remove UDP FEC transport profile ([ec5a0bb](https://github.com/mcthesw/TractorBeam/commit/ec5a0bbc268380e0dc88d0930c0eedc4e605e659))
* scaffold bridge workspace ([55ea43c](https://github.com/mcthesw/TractorBeam/commit/55ea43cbd9f4ba047619bb9289ca68b93fa71671))
* write GUI client logs with tracing ([8fb3305](https://github.com/mcthesw/TractorBeam/commit/8fb3305eb17f2bc19cd76437518de681699e0a1f))


### Bug Fixes

* avoid non-windows injector warning ([86453f7](https://github.com/mcthesw/TractorBeam/commit/86453f7af8e036912164240f9d3236d98006f93d))
* **client:** bound relay probes to protocol payload limits ([230236d](https://github.com/mcthesw/TractorBeam/commit/230236d65a7ca668f4c0a6a4c58435b99e3935ba))
* **client:** improve LAN session observability ([c8a75c5](https://github.com/mcthesw/TractorBeam/commit/c8a75c57a0508fcde52379a096e9cd702a388164))
* **client:** make native hook startup diagnosable ([3c1d41d](https://github.com/mcthesw/TractorBeam/commit/3c1d41d96d9985cd1ede6e303ddbe15ad86e48e4))
* **core:** report initial room peers ([7b23ad8](https://github.com/mcthesw/TractorBeam/commit/7b23ad8b1501fd63c425a3a74f9f3b9131f6a005))
* **gui:** keep startup responsive during relay init ([408dd33](https://github.com/mcthesw/TractorBeam/commit/408dd33edc3edc96e19b6e8abde5100abeba3816))
* **ipc:** preserve full-duplex hook traffic under backpressure ([3906b00](https://github.com/mcthesw/TractorBeam/commit/3906b00539c6d6d4b3e04c8c53ce0eef8ffe191c))
* **native-hook:** use fixed input delay offset ([3e6a38b](https://github.com/mcthesw/TractorBeam/commit/3e6a38b0a8becc6f3437763ac399d1780e9a88a1))
* **relay:** prune stale public peer state ([6f1e0e2](https://github.com/mcthesw/TractorBeam/commit/6f1e0e2160aa260f702f9449fb309924cc7a8497))
* report injector failure steps ([33671fd](https://github.com/mcthesw/TractorBeam/commit/33671fd3b95cdc8ee45c49ee2218d8ed551495d5))


### Code Refactoring

* clarify client runtime boundaries ([161bd6a](https://github.com/mcthesw/TractorBeam/commit/161bd6a439f1655ed748c626e5915214be45c09a))
* **client:** introduce explicit session route config ([b89b3d1](https://github.com/mcthesw/TractorBeam/commit/b89b3d1fa262422d185e7f6984e32ede56c4bf31))
* **client:** isolate application runtime and local IPC adapter ([aa629d0](https://github.com/mcthesw/TractorBeam/commit/aa629d05b3942925f1347ff4e254adea102573ea))
* **client:** make packet runtime route neutral ([5e156dc](https://github.com/mcthesw/TractorBeam/commit/5e156dc04f2219b95fb6047fcf4435663edd19d7))
* **client:** move config tests into a focused module ([16ce6d2](https://github.com/mcthesw/TractorBeam/commit/16ce6d2c71ed5a2cfb0962bb92118f178893ea6b))
* **config:** drop startup room defaults and prefer tcp ([80ef0de](https://github.com/mcthesw/TractorBeam/commit/80ef0decae054eb7d3de4e49969836030471c93e))
* **gui:** call rust-i18n translations directly ([c1a76e8](https://github.com/mcthesw/TractorBeam/commit/c1a76e8b5a539e24aaa3810a675f95bbc4d03cb4))
* **gui:** present smoothness through focused application modules ([2622b3e](https://github.com/mcthesw/TractorBeam/commit/2622b3e45e45362b9c74501b23752522d80ad339))
* **injector:** isolate Windows elevation helpers ([4c45b30](https://github.com/mcthesw/TractorBeam/commit/4c45b3023606559c17b5f985dd5872138b95fb0e))
* **injector:** remove prototype fallback artifacts ([9e95475](https://github.com/mcthesw/TractorBeam/commit/9e9547530dc8f5726347eb06eed3dd6667c7b285))
* **ipc:** split codec and connection responsibilities ([3610215](https://github.com/mcthesw/TractorBeam/commit/3610215e6b45f42fc30f896cfa9e0c22ceb92f1e))
* **native-hook:** move local IPC to owned worker ([a5adb0a](https://github.com/mcthesw/TractorBeam/commit/a5adb0a3f0e73738a75af8d7c3309774944c20dd))
* **protocol:** remove Relay Protocol v1 ([ebfb58f](https://github.com/mcthesw/TractorBeam/commit/ebfb58fa6639a2c20d43407bab7874998731f52e))
* **relay:** isolate protocol v1 and add size metrics ([1f75d2f](https://github.com/mcthesw/TractorBeam/commit/1f75d2f8da0c38d7babc5df13dee9c3f5ba904d5))
* **relay:** remove v1 runtime and simplify observability ([b7b3c7c](https://github.com/mcthesw/TractorBeam/commit/b7b3c7cea85ac4984ccfad803368ef0f302559cb))
* **workspace:** consolidate bridge crate layout ([ab40e91](https://github.com/mcthesw/TractorBeam/commit/ab40e919c5bb263e396c42c8ffaa4c74c4ade60c))


### Documentation

* align Phase 1 roadmap and relay guidance ([0012e49](https://github.com/mcthesw/TractorBeam/commit/0012e49bff876a32aeb8cd39b43d470a7d5d4a15))
* clarify relay transport terminology ([58213eb](https://github.com/mcthesw/TractorBeam/commit/58213eb79716ec5fb47cc919688c48fadc2f0597))
* define input delay terminology ([20a223c](https://github.com/mcthesw/TractorBeam/commit/20a223cc7134a51b21e4555c8a10086c8f717602))
* document direct LAN sessions ([541df8d](https://github.com/mcthesw/TractorBeam/commit/541df8d9464e78c9d18862285194ff3f51fa152c))
* document relay and client quality contracts ([c108d41](https://github.com/mcthesw/TractorBeam/commit/c108d413440201afc9fa6aa19bf8cf302eaa767d))
* document runtime and protocol boundaries ([022420d](https://github.com/mcthesw/TractorBeam/commit/022420dbe7a10fd2f497b8d6aeaf2085c79afd92))
* **protocol:** record the Relay Protocol v2 decisions ([1b55ff0](https://github.com/mcthesw/TractorBeam/commit/1b55ff0c3745c4aacf4b73ca98d89ec56766c830))
* **protocol:** sync Relay v2 architecture and contracts ([a6a389e](https://github.com/mcthesw/TractorBeam/commit/a6a389e00b280a7599bcd1ae4bc52a746b58d31d))
* **relay:** document observability operations ([066d6af](https://github.com/mcthesw/TractorBeam/commit/066d6affd0650778f19d4b19be6853c27a92728c))
* **relay:** document structured log collection ([32f0a22](https://github.com/mcthesw/TractorBeam/commit/32f0a229dd2591ba1235839a859106fb3697d592))


### Miscellaneous

* remove internal-test feature and closed-test report flow ([4683256](https://github.com/mcthesw/TractorBeam/commit/4683256016e9514d968928b67863ac49023e22ce))

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
