# Changelog

## [0.17.0](https://github.com/prop/navinamp/compare/v0.16.0...v0.17.0) (2026-11-14)


### Features

* **ai:** catalog-anchored local results (Discogs ⇄ library join) ([2ac2c29](https://github.com/prop/navinamp/commit/2ac2c293b5bec0028b89bd1a5ff07a5ef138bd34))
* **ai:** Discogs catalog lookups work without a token ([2a0958a](https://github.com/prop/navinamp/commit/2a0958a51900b633ac5fc457eca65242b6db826e))
* **ai:** Discogs catalog lookups work without a token ([ebc1e3a](https://github.com/prop/navinamp/commit/ebc1e3a710c989943c29ab2a2dc69737c05cbb5f))
* **audio:** split underrun counts by source kind, deeper remote decode-ahead ([b2d6502](https://github.com/prop/navinamp/commit/b2d650284f0bbe7d7d6cf4bb1b4a68016a66a7ef))
* **audio:** split underrun counts by source kind, deeper remote decode-ahead ([3c5716e](https://github.com/prop/navinamp/commit/3c5716eeed95d63947ffc35c9d0e3dd0fec4b5ed))
* **dev:** show the running build identity in the DEV tab ([e0b5d95](https://github.com/prop/navinamp/commit/e0b5d95c10cd0b35609dba267f61c0dff09b7b4e))
* **dev:** show the running build identity in the DEV tab ([1c19e0c](https://github.com/prop/navinamp/commit/1c19e0c439f99bb1a5ae4b1352529feee5d4c206))
* **hotkeys:** global next-track shortcut (Ctrl-Cmd-N / Ctrl-Alt-N) ([510154f](https://github.com/prop/navinamp/commit/510154f5deabb6bfd76ecb138884bfd3a5879c36))
* **hotkeys:** global next-track shortcut (Ctrl-Cmd-N / Ctrl-Alt-N) ([5b291bf](https://github.com/prop/navinamp/commit/5b291bf23a0ad4b3a4ed1e363878ce4b6f5c8fc4))
* **navidrome:** show album label and catalog number ([ef89691](https://github.com/prop/navinamp/commit/ef89691fe61e96628a2f1d33a5d55e03be5c985b))
* **navidrome:** show album label and catalog number ([055ad9c](https://github.com/prop/navinamp/commit/055ad9ceaefb1a323086ad4b0ecc3002b9582e23))
* **notify:** resolve notification album art backend-side ([94a8e25](https://github.com/prop/navinamp/commit/94a8e25ab92e09099f74a9883374c25caae9b890))
* **player:** add "Play next" to playlist and library context menus ([5dcbd12](https://github.com/prop/navinamp/commit/5dcbd12f3d4be559f76ac0b0c6748d43e04c8d0c))
* **player:** add "Play next" to playlist and library context menus ([b52657b](https://github.com/prop/navinamp/commit/b52657b05247640daf06cd06c80b8f81104eb706))
* **player:** resume the current track across restarts ([e42027d](https://github.com/prop/navinamp/commit/e42027ddd70dc21ced88897238472788261ab109))
* **player:** resume the current track across restarts ([9090a3a](https://github.com/prop/navinamp/commit/9090a3a8416f50409636ced6320055282279c342))
* **playlist:** follow the current track and show release metadata ([41d27cc](https://github.com/prop/navinamp/commit/41d27cc3f9dfd0c92032b85608a41b8478c9c28f))
* **playlist:** follow the current track and show release metadata ([c41cac5](https://github.com/prop/navinamp/commit/c41cac5f8d109c1ce10ab6248f9434cf316bac48))
* **queue:** persist the play queue to disk ([1e499aa](https://github.com/prop/navinamp/commit/1e499aaa461d16fad3119a1bfadf390ea6f8995c))
* **queue:** persist the play queue to disk ([8b95df4](https://github.com/prop/navinamp/commit/8b95df4e92db12e147fc31ea8a7829f80c6cf302))
* **skin:** pixel-faithful vector payloads for classic skins ([319d263](https://github.com/prop/navinamp/commit/319d26349252420c87fcfc3c2fd2000df6e15fd4))
* **skin:** pixel-faithful vector payloads for classic skins ([73da6ca](https://github.com/prop/navinamp/commit/73da6cab21fd317c69d494cae2dbda7f07600a33))
* **skin:** prefer smooth system text over classic bitmap fonts ([8cc0d50](https://github.com/prop/navinamp/commit/8cc0d506ea8edbb33be8d44b91bfabe26a075614))
* **skin:** prefer smooth system text over the classic bitmap font ([b011f0e](https://github.com/prop/navinamp/commit/b011f0e748c33427c7a944a21560711414db2351))
* **windows:** glued windows move as one cluster ([1037a67](https://github.com/prop/navinamp/commit/1037a67f5b74029aa9f01ad7ce5228506d9e3af3))
* **windows:** glued windows move as one cluster ([0eb55b0](https://github.com/prop/navinamp/commit/0eb55b0f3c7628c4832f4a64e6854c233549cf10))


### Bug Fixes

* **audio:** bound the device-change window symmetrically ([2b5850a](https://github.com/prop/navinamp/commit/2b5850a0db5f508ae186b52349cf93abf45901ec))
* **audio:** classify all remote variants, flip attribution at ring turnover ([0d3bcef](https://github.com/prop/navinamp/commit/0d3bcef4631b2819cce6110dbbfbe631e6eab6f7))
* **audio:** device-loss follow-ups (worker + actionable message) ([31e475b](https://github.com/prop/navinamp/commit/31e475b94e7d5f0dd6c635ffc0fd0e33a57a857e))
* **audio:** do not resume paused playback on output-device changes ([a42cfc3](https://github.com/prop/navinamp/commit/a42cfc3584d8d1325d1280bba174e466817f668b))
* **audio:** do not resume paused playback on output-device changes ([742946e](https://github.com/prop/navinamp/commit/742946ee7152abe07ed533a24de1697d4684b64b))
* **audio:** repair WAV header lengths the strict reader rejects ([ac7c110](https://github.com/prop/navinamp/commit/ac7c110a59b9b6a609100314c05c10cbf9d663cf))
* **audio:** repair WAV header lengths the strict reader rejects ([bfee523](https://github.com/prop/navinamp/commit/bfee523e2a35b7a29c35bd7cb13bde8b2c114865))
* **audio:** report a lost output device with actionable wording ([5a680df](https://github.com/prop/navinamp/commit/5a680dfdb7ab576503e3042bbaaee54c8cc34827))
* **library:** scale-native chrome via CSS zoom (crisp text at 1.5×) ([ede307d](https://github.com/prop/navinamp/commit/ede307df6feab43d40d5180a8c9929b45b9ca524))
* **library:** search results own the footer duration (0 tracks · 8216:46:34) ([f5276ce](https://github.com/prop/navinamp/commit/f5276ce015eda7a35b23cbc03b82c3931ed0d6da))
* **library:** search results own the footer duration and bulk actions ([53ec826](https://github.com/prop/navinamp/commit/53ec826cfa774a805ffae3bfc2f6fe52cb1c2db7))
* **library:** zoom-agnostic natural-size measurement ([ee15b6a](https://github.com/prop/navinamp/commit/ee15b6acfc30296b44ae91c0040e4e2f505a18f5))
* **library:** zoom-agnostic natural-size measurement (truncated window at 1.5×) ([8accd6f](https://github.com/prop/navinamp/commit/8accd6f7efd6accd3f0c8c087165d89decbbcb04))
* **media:** serialize deferred play commands in one worker ([d016485](https://github.com/prop/navinamp/commit/d0164856848a8cd4d8d48ed6b03505b154e676e6))
* **navidrome:** drop redundant test binding rebind (clippy) ([7fc6462](https://github.com/prop/navinamp/commit/7fc64622b651a476f3d39907939886d1faba3c1d))
* **navidrome:** playlist rows use the album artist + derive album_key ([da0218a](https://github.com/prop/navinamp/commit/da0218a03f827281b98187b2ff5e78ceb3c3ec3d))
* **navidrome:** playlist rows use the album artist + derive album_key ([404396b](https://github.com/prop/navinamp/commit/404396b38bba71790a484a52415353be9ccd2457))
* **navidrome:** read displayAlbumArtist, the actual OpenSubsonic field ([f462052](https://github.com/prop/navinamp/commit/f4620525066bca9865aedc16de364e171e96b51d))
* **notify:** log every now-playing notification decision ([0af635d](https://github.com/prop/navinamp/commit/0af635dda6fdd866d94603a72217ded4c44af8d3))
* **notify:** log every now-playing notification decision (diagnose missing banner) ([ba93f77](https://github.com/prop/navinamp/commit/ba93f77735f43bb78231f8a445f4e841f1d7b5dc))
* **player:** keep the warning bar click-through so skin buttons stay usable ([820c053](https://github.com/prop/navinamp/commit/820c0534a3e966070e61b4b86ed0f6b9b6393cac))
* **player:** keep the warning bar click-through so skin buttons stay usable ([e03c98f](https://github.com/prop/navinamp/commit/e03c98f71fd6759281a51297407b62ed82b70b2c))
* **player:** keep the warning list scrollable after the click-through fix ([fec1089](https://github.com/prop/navinamp/commit/fec10895038a72377080181df99d187e33e1e90a))
* **playlist:** fractional row height + review cleanups ([d8acbca](https://github.com/prop/navinamp/commit/d8acbca76424d675caba0cf4f450493e38bb7226))
* **playlist:** render the skinned playlist list scale-native ([17ba58e](https://github.com/prop/navinamp/commit/17ba58e9a46d431baaaad7baa665f1823fc9704f))
* **playlist:** render the skinned playlist list scale-native ([2d90917](https://github.com/prop/navinamp/commit/2d909174a1278554c7285c73add6f8d96890b1a3))
* **playlist:** scale the folded and unskinned playlist window ([4b93b48](https://github.com/prop/navinamp/commit/4b93b48d2c1edb13faf59137d373b0c89ee6f888))
* **playlist:** scale the folded and unskinned playlist window ([84bc21e](https://github.com/prop/navinamp/commit/84bc21eb76a6b64fb468693bcd0cc32f64b37063))
* **queue:** rebuild the shuffle order when restoring a queue ([3f066c9](https://github.com/prop/navinamp/commit/3f066c9bd5765a2d9b181dad0ee010afa2e471ed))
* **queue:** rebuild the shuffle order when restoring a queue (next dead after restart) ([439fb3a](https://github.com/prop/navinamp/commit/439fb3ab198a7a02ca7543c45a72e79721567286))
* **skin:** keep bitmap-font glyph strips raster ([14d92f2](https://github.com/prop/navinamp/commit/14d92f26501b455e8dc4bc3ee517c12d7ce942fa))
* **skin:** keep bitmap-font glyph strips raster ([8d94763](https://github.com/prop/navinamp/commit/8d94763899474ffd0147d10fa68bb07bfdb77af9))
* **skin:** keep classic time on the bitmap digits ([277865a](https://github.com/prop/navinamp/commit/277865af5a1459dc087ddae65b3694a6cce9a220))
* **skin:** keep classic time on the bitmap digits (timestamp follow-up) ([8d6f16e](https://github.com/prop/navinamp/commit/8d6f16e9f6e9e28496c3ffe879766366d77f4e98))
* **skin:** keep slider bars opaque behind the track/thumb ([ab7db97](https://github.com/prop/navinamp/commit/ab7db978ab4a196942981e8c4bc0a3d80d4d015e))
* **skin:** keep slider bars opaque behind the track/thumb (seek trail) ([a0f8b50](https://github.com/prop/navinamp/commit/a0f8b50f5d7354d111c60eb43ccfaab998354e14))
* **skin:** keep the classic font for Unicode typography in titles ([289f5c8](https://github.com/prop/navinamp/commit/289f5c8308c886a4d179f870173dd31c26ecaf15))
* **skin:** keep the classic ticker font for Unicode typography ([57f52a1](https://github.com/prop/navinamp/commit/57f52a116fcec44fb2849a85544b0dc6950af316))
* **skin:** rasterize vector payloads nearest-neighbour at integer scales ([e5deea7](https://github.com/prop/navinamp/commit/e5deea74c876c8d438878a76b6499dbf0896174c))
* **skin:** rasterize vector payloads nearest-neighbour at integer scales ([6dc5175](https://github.com/prop/navinamp/commit/6dc51752b3278abaeb2cb2aa22b10dc84f874039))
* **skin:** review follow-ups — clamp the band crop, honest size bound ([6006131](https://github.com/prop/navinamp/commit/6006131a4c5869232633db52a4b119532fcb466d))
* **skin:** review follow-ups — cover more typography, document asymmetry ([2e3d410](https://github.com/prop/navinamp/commit/2e3d41065a972a27b97915d5f1974d736da5748a))
* **skin:** smooth raster art at fractional UI scales ([57fb1a0](https://github.com/prop/navinamp/commit/57fb1a00202e7beda209d6eb46a25469f58faaf9))
* **skin:** smooth raster art at fractional UI scales (seek-bar trail) ([73e6a4e](https://github.com/prop/navinamp/commit/73e6a4e86c33946241851d2c6e077856c1be6fd2))
* **skin:** vectorize the player titlebar band (Winamp wordmark) ([5ca9ce9](https://github.com/prop/navinamp/commit/5ca9ce940a1109452adf33908ac947749571773b))
* **ui:** scale-native library chrome + vector player titlebar band ([dc02787](https://github.com/prop/navinamp/commit/dc02787262bd9010cbf6a8d71c76e87bea69fefd))
* **windows:** drop unused fixture var in the dock-follow dedupe test ([c4a59d1](https://github.com/prop/navinamp/commit/c4a59d1fe349c0b598137b0c0096aa82312ed582))

## [0.16.0](https://github.com/prop/navinamp/compare/v0.15.1...v0.16.0) (2026-11-09)


### Features

* **skin:** bundle the Winamp 2.91 base skin ([f8b6d30](https://github.com/prop/navinamp/commit/f8b6d303781bce7bc9c28c612c1287ef3fa56b32))
* **skin:** bundle the Winamp 2.91 base skin ([d3e1f6f](https://github.com/prop/navinamp/commit/d3e1f6f14945e29932cdcb76289919b2b43caa59))
* **skin:** use the Winamp Skin Museum base-2.91 and make it the default ([93418c4](https://github.com/prop/navinamp/commit/93418c4055fd30775cd839f6f0c4b8d1788646b4))

## [0.15.1](https://github.com/prop/navinamp/compare/v0.15.0...v0.15.1) (2026-11-07)


### Bug Fixes

* **release:** add Applications link to the macOS dmg ([12c48da](https://github.com/prop/navinamp/commit/12c48dabc460b3768fe82bbf2ac8893a0fe7c9e8))
* **release:** add Applications link to the macOS dmg ([c95a8e5](https://github.com/prop/navinamp/commit/c95a8e544cad638e2b8ee4841dd401122b9a39fa))

## [0.15.0](https://github.com/prop/navinamp/compare/v0.14.0...v0.15.0) (2026-11-07)


### Features

* **brand:** use supplied wine glass and Navidrome logo ([cdd09ee](https://github.com/prop/navinamp/commit/cdd09eecb03f3e9ead53db90425b9abfef08894d))
* **brand:** use supplied wine glass and Navidrome logo ([e7114ea](https://github.com/prop/navinamp/commit/e7114ea8abf56d4f7fe9bbd43955f4ccef505904))


### Bug Fixes

* **brand:** replace ambiguous monogram with a wine glass mark ([22195bf](https://github.com/prop/navinamp/commit/22195bf7513bcf5ce8cb1f36b941508c26864a7c))

## [0.14.0](https://github.com/prop/navinamp/compare/v0.13.0...v0.14.0) (2026-11-06)


### Features

* **audio:** support self-signed remote streams ([1dda52a](https://github.com/prop/navinamp/commit/1dda52a5a11389af85ac522a7ac582ffab93e5f0))
* **brand:** add wine-drop logo and regenerate icons ([97435d8](https://github.com/prop/navinamp/commit/97435d8304f81210b8a76afdc7145581b2259633))
* **brand:** wine logo and README refresh ([9c0cff7](https://github.com/prop/navinamp/commit/9c0cff7a963ef4e18e2185bcc5bc748296f91802))
* **player:** surface playback errors in the warning badge ([1692173](https://github.com/prop/navinamp/commit/1692173a03cb26e280311c52c5bee6356d08a29d))
* **playlist:** skin the editor frame and action bar ([72c6c8d](https://github.com/prop/navinamp/commit/72c6c8dd03ae865d86904c849ae7b49b29c0e27b))
* **playlist:** skin the editor frame and action bar ([ba7cba5](https://github.com/prop/navinamp/commit/ba7cba5dbf0048f4baef003d7cdc49827c200822))
* **ui:** add a global UI scale selector ([8766d24](https://github.com/prop/navinamp/commit/8766d2425781872db28c31d8e2f937bdddbe8170))
* **ui:** add a global UI scale selector ([3c97f2c](https://github.com/prop/navinamp/commit/3c97f2c410a5503bdb100c4114d47d3f437d01c4))
* **ui:** close dialogs and secondary windows with Escape ([5c70b91](https://github.com/prop/navinamp/commit/5c70b91a924ff44ebd48d35af326b3223c6a1fb6))
* **ui:** close dialogs and secondary windows with Escape ([81ef0ad](https://github.com/prop/navinamp/commit/81ef0ad2fa9aea3912689419c2ccbd99660877f1))


### Bug Fixes

* **library:** natural path order for unpadded track numbers ([abd353b](https://github.com/prop/navinamp/commit/abd353b905ede3ba01b6028bf63d9d580ef42cd3))
* **library:** order paged views and bulk adds by file path ([f7e9481](https://github.com/prop/navinamp/commit/f7e94818a4ccf361d8caa8d23b31b959eca49ce9))
* **library:** order paged views and bulk adds by file path ([4b0352e](https://github.com/prop/navinamp/commit/4b0352e1766b2442aa89b865b38c66fef8bffaa0))
* **navidrome:** play self-signed profiles through the insecure source ([51ed615](https://github.com/prop/navinamp/commit/51ed61563e75d424716e875e33d22ce8af9613e9))
* **navidrome:** play streams from self-signed profiles ([1c4c7d4](https://github.com/prop/navinamp/commit/1c4c7d4ad9f1ce0f64bbf4a9af9c92fa75657d20))
* **notify:** consume art_path on Linux (cfg cleanup made it unused there) ([1973607](https://github.com/prop/navinamp/commit/197360721ae6735692b101dc584f06e2bef63d26))
* **notify:** deliver via NSUserNotificationCenter so dev runs work ([9cbc801](https://github.com/prop/navinamp/commit/9cbc8018924e4020551e73df5a42e1965a709f94))
* **notify:** deliver via NSUserNotificationCenter so dev runs work ([2a7e3e0](https://github.com/prop/navinamp/commit/2a7e3e073715f22067881d743c08dea9781af920))
* **notify:** review follow-ups — DND note, macos temp cleanup, pin ([6154f0d](https://github.com/prop/navinamp/commit/6154f0ddb93c4159b3933be85360f9c0050af2f6))
* **player:** close the playback warning panel when the error clears ([77eaa00](https://github.com/prop/navinamp/commit/77eaa00522af8927678835b8ba42d1c9c860856a))
* **playlist:** address review follow-ups ([681d4f7](https://github.com/prop/navinamp/commit/681d4f701871217727c036608d575c85e226b23f))
* **playlist:** expose the queue summary to assistive tech ([509fff4](https://github.com/prop/navinamp/commit/509fff4c82b5b6bc729f989e5910c451e3268fec))
* **playlist:** filter only the fallback skin's sample placeholders ([db2e870](https://github.com/prop/navinamp/commit/db2e8708a40d464caadbd7294364cb744a3a0b11))
* **playlist:** keep a generic font fallback after the skin font ([ee793d5](https://github.com/prop/navinamp/commit/ee793d55829cee7fff193800c9e013d0cf2455e0))
* **playlist:** keep footer actions inside the narrow window ([469a964](https://github.com/prop/navinamp/commit/469a964a7f389e7b7c61c37d396a8f06e381a7a5))
* **playlist:** keep footer actions inside the narrow window ([4958cc7](https://github.com/prop/navinamp/commit/4958cc78d1092d50639ee5c29b915fd9fe3a63fb))
* **playlist:** keep open menus above the titlebar drag layer ([4c58e59](https://github.com/prop/navinamp/commit/4c58e59943461049445f1b72f32b7da101ddf50e))
* **playlist:** keep row menus aligned inside the scaled skin overlay ([8fd3451](https://github.com/prop/navinamp/commit/8fd3451dcc219296615636096f41ee7020aff300))
* **skin:** make the bitmap fallback color reactive and cover time ([9f82d03](https://github.com/prop/navinamp/commit/9f82d03ac48048979306075a69a2d3d9de8ede58))
* **skin:** match bitmap glyph color for missing-glyph fallback ([648956b](https://github.com/prop/navinamp/commit/648956b634c9c4fbdafe59897af49b30f3b5417b))
* **skin:** match bitmap glyph color for missing-glyph fallback ([27ad4cb](https://github.com/prop/navinamp/commit/27ad4cb72e35f05ebd199a5909759ebd81d7a8db))
* **ui:** guard Alt+S against non-element key targets ([d476016](https://github.com/prop/navinamp/commit/d4760169db6a3850b76a6892753724e933431a1f))
* **ui:** leave Alt+S alone while typing in a field ([11951ab](https://github.com/prop/navinamp/commit/11951ab316eb9ab789a59be62cf7e3864b87c508))
* **ui:** migrate AddUrlDialog to the shared Escape stack ([6130327](https://github.com/prop/navinamp/commit/613032771a4ebd9602294f9dc0c02cc970a0d636))
* **ui:** retry failed window-size requests ([a80e9ef](https://github.com/prop/navinamp/commit/a80e9ef80bd38258c17d0dfb7efc2ea7c0a3ccc2))

## [0.13.0](https://github.com/prop/navinamp/compare/v0.12.0...v0.13.0) (2026-11-05)


### Features

* **navidrome:** per-server self-signed TLS certificate support ([92a3e11](https://github.com/prop/navinamp/commit/92a3e11f8924af7baf3963bfa183482d5e76b7e3))
* **navidrome:** per-server self-signed TLS certificate support ([a3f4780](https://github.com/prop/navinamp/commit/a3f4780f17ae1ed4ce66e980a4ebb047aa0df863))
* **playlist:** add URL streams and opt-in YouTube audio ([ac93c0c](https://github.com/prop/navinamp/commit/ac93c0c7dad00d8963d868c5bbe3c653c599cd0c))
* **playlist:** add URL streams and opt-in YouTube audio ([56561ba](https://github.com/prop/navinamp/commit/56561bac3d1554370da964ae39a28ff91296d265))
* **search:** add embedding-based semantic track matching ([34958df](https://github.com/prop/navinamp/commit/34958df783bd3ab99c38fd032d3f492cc9cf8448))
* **search:** add embedding-based semantic track matching ([50df8a2](https://github.com/prop/navinamp/commit/50df8a27ba7cb251514096671f2c86ae267a24b7))


### Bug Fixes

* **audio:** harden URL-stream redirects and title fallback ([1dc7c5f](https://github.com/prop/navinamp/commit/1dc7c5ffb646dc36fccd9b3966d4fd565548f119))
* **eq:** make the classic close button clickable ([3f57178](https://github.com/prop/navinamp/commit/3f57178b7f93e5ee876088542272098fcede4452))
* **eq:** make the classic close button clickable ([2c7b41a](https://github.com/prop/navinamp/commit/2c7b41a5bf90e93c380fe3ff00807e0fd4c6045c))
* **navidrome:** review follow-ups — scrub whole chain, hydrate TLS flag ([d608a61](https://github.com/prop/navinamp/commit/d608a61c7244e59336fb591539285409a4e3e6fa))
* **playlist:** keep the URL dialog compact for direct streams ([b065622](https://github.com/prop/navinamp/commit/b065622b36352379a96edce9a4e2a82c8b33378f))
* **semantic:** keep index progress accounting accurate ([d2d3526](https://github.com/prop/navinamp/commit/d2d3526fb34f5c11b6a0763569fb2a3b98b84aa5))

## [0.12.0](https://github.com/prop/navinamp/compare/v0.11.0...v0.12.0) (2026-11-04)


### Features

* **audio:** pre-open a lookahead window of upcoming tracks ([4dffc6c](https://github.com/prop/navinamp/commit/4dffc6c3ea2b3c9dff4152cb7a943da8b53cd645))
* **audio:** pre-open a lookahead window of upcoming tracks ([1db3e0e](https://github.com/prop/navinamp/commit/1db3e0e0dbfdddeeeb1215f0bec74ebe4a7c7cc9))
* **library:** fingerprint remote albums for quick rescans ([23d6b6f](https://github.com/prop/navinamp/commit/23d6b6f55d89220cddeb950c1502bff82997afe9))
* **navidrome:** quick and full rescan modes ([524997d](https://github.com/prop/navinamp/commit/524997dd81575980ec8565c912fd8aa4b83220e3))
* **navidrome:** quick and full rescan with concurrent album fetch ([8fdc6e5](https://github.com/prop/navinamp/commit/8fdc6e561b8dcac0a76f7c86dbc717656c7f0a54))
* **navidrome:** show sync progress in tracks instead of albums ([c29ed63](https://github.com/prop/navinamp/commit/c29ed635ba2b15ebde00391704824a57fe98553e))
* **navidrome:** show sync progress in tracks instead of albums ([dbb980d](https://github.com/prop/navinamp/commit/dbb980d42e8589dccb2670820857fd516473b0a1))
* **notify:** album art in desktop notifications + Rust-side sending ([8034d0c](https://github.com/prop/navinamp/commit/8034d0c76a13267a71d82ab4d91595db6be9ba05))
* **notify:** album art in desktop notifications + Rust-side sending ([0d9dec0](https://github.com/prop/navinamp/commit/0d9dec01c2dd1deb35c4713bfdd3b605b6b8a5d4))
* **ui:** add quick and full rescan actions ([aa301f5](https://github.com/prop/navinamp/commit/aa301f5f196681225858f3db3c4d1b2dc5aaf3ed))


### Bug Fixes

* **audio:** keep SetUpcoming from starting idle playback ([6423d07](https://github.com/prop/navinamp/commit/6423d07f0a35152a7da1b49cd26d953a83f4eae7))
* **audio:** open Play targets through the prefetch thread ([f23da89](https://github.com/prop/navinamp/commit/f23da8970c8c5d20c9f941ae6c963d838f357124))
* **audio:** open Play targets through the prefetch thread ([fba8a2f](https://github.com/prop/navinamp/commit/fba8a2f9e64f2bf75479f1173bc27a75e59be6bd))
* **menu:** keep context menu in-window and scrollable ([2773456](https://github.com/prop/navinamp/commit/277345623d1fac476e40780cff006f3d541611be))
* **menu:** keep context menus inside the window and scrollable ([82f012d](https://github.com/prop/navinamp/commit/82f012dab7cb9705dc266a3097b0364f235c5253))
* **menu:** wrap long info rows and copy the real Navidrome path ([1e8c5a1](https://github.com/prop/navinamp/commit/1e8c5a1dc5c31496a9d0162f3de971f4e519e500))
* **navidrome:** align track progress with the deduped album walk ([a13d835](https://github.com/prop/navinamp/commit/a13d835d9d59ed52acf90355b2eca721120769d1))
* **navidrome:** avoid the artist-index call on the album walk ([9fd56ee](https://github.com/prop/navinamp/commit/9fd56ee87f94fe52223e3677ec57d08e133d18fb))
* **notify:** review follow-ups — UN backend, unique temp art, soft failures ([94b782a](https://github.com/prop/navinamp/commit/94b782a4b0efddbace0649bf1b7f286a62cc6a72))

## [0.11.0](https://github.com/prop/navinamp/compare/v0.10.0...v0.11.0) (2026-11-04)


### Features

* **ai:** pair Discogs catalog answers with local keyword rules ([73f3f60](https://github.com/prop/navinamp/commit/73f3f60ea9471bcfaadb65284c3f70e23ff1b82b))
* **ai:** pair Discogs catalog answers with local keyword rules ([7e1cc56](https://github.com/prop/navinamp/commit/7e1cc563500119c876797369a4d679a06bb9257f))
* **playlist:** jump to the current track + detailed row info in the context menu ([bd35010](https://github.com/prop/navinamp/commit/bd350108a99437eab47983378fce3544e678bb21))
* **playlist:** jump to the current track and show details in the row menu ([6db7cec](https://github.com/prop/navinamp/commit/6db7cecaca68260f409c0769d3cbdaaa635794c0))
* **windows:** accept the first mouse click while unfocused ([c80a58a](https://github.com/prop/navinamp/commit/c80a58a376d60c1cfd814e1d61ffba94d5bb0880))
* **windows:** accept the first mouse click while unfocused ([8721f41](https://github.com/prop/navinamp/commit/8721f41562d347c0f549d19d1700908af72fbc00))


### Bug Fixes

* **ai:** carry the Discogs keyword into trimmed-context injections ([c8e05c6](https://github.com/prop/navinamp/commit/c8e05c6f1a05ac74aaad533d2fb7ee3d75744306))
* **audio:** open the next track off the decode thread ([4176509](https://github.com/prop/navinamp/commit/4176509a8118da6c828b8ac1fb5a7b1b539bded2))
* **audio:** open the next track off the decode thread ([18b137c](https://github.com/prop/navinamp/commit/18b137c33361a1448b78423689cd5565044ab264))
* **library:** repair legacy tag text on remote rows and queue restore ([2cf288a](https://github.com/prop/navinamp/commit/2cf288a10077d0a1cb63857c9f2525bf6f56bcf1))
* **library:** repair legacy tag text on remote rows and queue restore ([932f572](https://github.com/prop/navinamp/commit/932f57270c8adbb95bec7aeac258a4e45a0774f0))
* **library:** tighten charset repair and bound the migration ([dd365f3](https://github.com/prop/navinamp/commit/dd365f3fafbdb237b29811871656a84e05e5528f))
* **playlist:** stable info keys, styled menu scrollbar, clearer NOW tooltip ([ce9f646](https://github.com/prop/navinamp/commit/ce9f64665cefcab6d41965bfe18fefe870f73f56))
* **ui:** disable autocorrect in search and rule inputs ([4c10080](https://github.com/prop/navinamp/commit/4c1008053e82b09004bd32d93166d6714d0e77c0))
* **ui:** disable autocorrect in search and rule inputs ([d61dc0a](https://github.com/prop/navinamp/commit/d61dc0a17558e42bba5512c860c2472ba8f24be5))

## [0.10.0](https://github.com/prop/navinamp/compare/v0.9.0...v0.10.0) (2026-11-03)


### Features

* AI search offline fallback — rules parser + local provider presets ([29ebc5a](https://github.com/prop/navinamp/commit/29ebc5a6061432dba6815240692d0d94333ad811))
* **ai:** offline rules fallback and local provider presets ([c565c94](https://github.com/prop/navinamp/commit/c565c94685a058657a90bf8c6047c31e8503b3d2))
* **ai:** refresh provider models and add a model picker ([cd6b771](https://github.com/prop/navinamp/commit/cd6b771c2c24076c50aea3ad1b082de75a2df00b))
* **ai:** refresh provider models and add a model picker ([b5af2e6](https://github.com/prop/navinamp/commit/b5af2e6887808e117495efa20dcdeb16d8bb16e9))
* **ai:** surface Discogs catalog results and structured filters ([c6b2455](https://github.com/prop/navinamp/commit/c6b245516d71d444d1445694570511d066c516ef))
* **ai:** surface Discogs catalog results and structured filters ([b970c15](https://github.com/prop/navinamp/commit/b970c154b6b9fa3c5ae43df3852711bfde95005a))
* curated popular-skins pack (24 classic skins, downloaded on request) ([e877636](https://github.com/prop/navinamp/commit/e87763672b3d298091cd92f970309cb1b1c01192))
* **library:** move Equalizer / Skin / DEV into tabs ([#30](https://github.com/prop/navinamp/issues/30)) ([ea11415](https://github.com/prop/navinamp/commit/ea11415776190b23643943ba50911b971792621a)), closes [#16](https://github.com/prop/navinamp/issues/16)
* **notifications:** global now-playing shortcut ([a697522](https://github.com/prop/navinamp/commit/a6975222d005fc8762577e6bef70cff1bdeba4e8))
* **notifications:** global now-playing shortcut ([646c95f](https://github.com/prop/navinamp/commit/646c95f0edd7259d520bf2312b5245d37f587447))
* **onboarding:** first-run tour, AI demo, and provider wizard ([c6aa962](https://github.com/prop/navinamp/commit/c6aa9629ad22c9c6f67675e2d7ace4a5db8cc6fb))
* **onboarding:** first-run tour, AI demo, and provider wizard ([c2918e6](https://github.com/prop/navinamp/commit/c2918e68b8c39fb09991f20bad3818b49c858808))
* **player:** desktop notifications on track change ([#36](https://github.com/prop/navinamp/issues/36)) ([9703d30](https://github.com/prop/navinamp/commit/9703d30d08e7f282ae0f6102d4192a5398c315e5)), closes [#34](https://github.com/prop/navinamp/issues/34)
* **player:** seek on long press and handle OS media seeks ([1763501](https://github.com/prop/navinamp/commit/17635011c0538c5a6d4d65e65b56dca2a3796141))
* **player:** seek on long press and handle OS media seeks ([3fb0467](https://github.com/prop/navinamp/commit/3fb046736325373593c395100bdb5a9dd953eb3e))
* **skins:** add base-2.91 and Classified-family skins to the pack ([d2858f0](https://github.com/prop/navinamp/commit/d2858f08235e1142dcb506c453aa05eee282b8e7))
* **skins:** curated popular-skins pack with on-request download ([52233aa](https://github.com/prop/navinamp/commit/52233aa90b364d48421fd40830d151fa8539cb06))
* windowshade — double-click fold/unfold for player, EQ, and playlist ([2a729b6](https://github.com/prop/navinamp/commit/2a729b6861a8813a64531512b67e14a373b05e53))


### Bug Fixes

* **ai:** address the Discogs catalog audit ([740e6ee](https://github.com/prop/navinamp/commit/740e6ee2bebeb5f8acde3bcb2715ad0addfa55f7))
* **ai:** address the Discogs catalog audit ([d2004c5](https://github.com/prop/navinamp/commit/d2004c547f85d627baa78c83778debac3567c73d))
* **ai:** enforce a real request budget and normalize Discogs kind ([011d3af](https://github.com/prop/navinamp/commit/011d3afc829e7543fd39055384ff57d14369b352))
* **ai:** read Discogs response pagination fields ([1e35242](https://github.com/prop/navinamp/commit/1e35242c2eb1121cf9dcab02238d4a78ab0e8082))
* **library:** repair legacy CP1251/KOI8-R tag text to UTF-8 ([#42](https://github.com/prop/navinamp/issues/42)) ([93120e4](https://github.com/prop/navinamp/commit/93120e43de546ee735bff620cccc130d20811d8f)), closes [#41](https://github.com/prop/navinamp/issues/41)
* **library:** stop macOS ctrl+click right-click from dropping the selection ([#25](https://github.com/prop/navinamp/issues/25)) ([dc71346](https://github.com/prop/navinamp/commit/dc71346ccee81475e9634fd206dbee32cb41e169)), closes [#24](https://github.com/prop/navinamp/issues/24)
* **notifications:** make the global shortcut's first use reliable ([bb6e016](https://github.com/prop/navinamp/commit/bb6e016a4a8af4a5ba8d4d32aae6dcff21d0a54c))
* **onboarding:** make the spotlight click-through and protect the wizard ([68ed6a9](https://github.com/prop/navinamp/commit/68ed6a901f3d1af48edf11e5526e64354d97a3c0))
* **player:** stop hold-seek when the track changes ([7053b52](https://github.com/prop/navinamp/commit/7053b523003abd40b9aaf6177054061f2088fd41))
* **playlist:** keep folded width fixed and use the playlist track font ([6505bc7](https://github.com/prop/navinamp/commit/6505bc7773e753382ea867d8e524e03d2562f3a7))
* **playlist:** keep shade width fixed and use the playlist track font ([7e3d9fa](https://github.com/prop/navinamp/commit/7e3d9fa6fe59bfcf27fdca1bfca35a8d845c6b97))
* **playlist:** virtualize the editor and coalesce queue broadcasts ([9cad51e](https://github.com/prop/navinamp/commit/9cad51ed96d202a6d94d68b908e48b8129e3bccb))
* **playlist:** virtualize the editor so huge queues don't freeze ([1989344](https://github.com/prop/navinamp/commit/19893449889822056167ca2b71bd52a61808fdb3))
* **queue:** keep saved queues and resolve play-now in the player ([7aa27c2](https://github.com/prop/navinamp/commit/7aa27c23a560005b15b4ceb9e093e46c5a169f60))
* **skin:** correct classic text glyphs and complete the windowshade strips ([2b9f3ef](https://github.com/prop/navinamp/commit/2b9f3ef10649c2e9eb3c11e3551987e043475ec1))
* **ui:** make the AI setup prompt encouraging ([8c2d50c](https://github.com/prop/navinamp/commit/8c2d50c0c5ff6fd12a374761fe268a4f0612feff))
* **ui:** make the AI setup prompt encouraging ([159c41a](https://github.com/prop/navinamp/commit/159c41ada12671a7bcea784e1ad395b68ccbcf95))
* **vis:** adaptive bar density, edge clamp, and classic viscolor palette ([9e0e68c](https://github.com/prop/navinamp/commit/9e0e68ceae78779104e5682d9eb0857feff0843b))
* windowshade follow-ups — clock colon, mini analyzer, folded playlist title ([9d64a21](https://github.com/prop/navinamp/commit/9d64a21e72699c3787840e179ce8a522c80cd1ac))
* **windows:** keep double-click for windowshade; allow 14px shade heights ([103b4d3](https://github.com/prop/navinamp/commit/103b4d37b025ff039057fec79e0649be7be01660))

## [0.9.0](https://github.com/prop/navinamp/compare/v0.8.0...v0.9.0) (2026-11-02)


### Features

* **eq:** show the automatic headroom readout ([447afca](https://github.com/prop/navinamp/commit/447afca78475037b8d3a4ead38e1f161e30ce687))
* **search:** backend for advanced and AI search ([94e1541](https://github.com/prop/navinamp/commit/94e1541c267a4fda8b67344bc778f43027664921))
* **search:** three-mode search UI with AI and Discogs ([3b3effe](https://github.com/prop/navinamp/commit/3b3effef042ef3553870c12d225bcb525b229404))


### Bug Fixes

* **player:** wire skin shuffle/repeat toggles to the queue ([#21](https://github.com/prop/navinamp/issues/21)) ([82f0833](https://github.com/prop/navinamp/commit/82f083304e27e603ab438aaaa0f89269c4b4cfdd)), closes [#20](https://github.com/prop/navinamp/issues/20)

## [0.8.0](https://github.com/prop/navinamp/compare/v0.7.0...v0.8.0) (2026-11-02)


### Features

* **eq:** add a Bass Boost preset ([e38e80d](https://github.com/prop/navinamp/commit/e38e80d8564872dbdcd0a31e66301dbe24af36e6))
* **eq:** add an AUTO curve manager ([f701f46](https://github.com/prop/navinamp/commit/f701f465314d1a087c48bc0f4b0b299e2a49a52b))
* **library:** add A–Z jump rails to the long tree groups ([8446a7f](https://github.com/prop/navinamp/commit/8446a7f5bc41b5ee25ab59b2d64ac434000bd2ad))
* **navidrome:** per-server playlists and publish target ([995ad3f](https://github.com/prop/navinamp/commit/995ad3fd80b5472faa500f4ebf900d65db07a155))


### Bug Fixes

* **eq:** stop presets from clipping the output ([8d5d946](https://github.com/prop/navinamp/commit/8d5d946f4701ab9c20d6aad34f48284fb4744b70))
* **ui:** selection-aware row menus and a global Always on Top ([e6dc7e1](https://github.com/prop/navinamp/commit/e6dc7e18add69ae9072cfeb28a95f4a40137432c))

## [0.7.0](https://github.com/prop/navinamp/compare/v0.6.0...v0.7.0) (2026-11-02)


### Features

* **eq:** genre-based AUTO preset loading ([f7407f3](https://github.com/prop/navinamp/commit/f7407f34b23f47b1955724f71ec1bf461bec9acb))
* **eq:** per-track and per-artist AUTO curves ([34c6e82](https://github.com/prop/navinamp/commit/34c6e826ead994628fb6f24eb64b66bf4a0ed86a))
* **library:** SELECT ALL covers the whole filter ([0596c51](https://github.com/prop/navinamp/commit/0596c513fc9c00fa448e664976292403dd8f6f9d))
* **navidrome:** aggregate multiple server libraries ([bf7cb67](https://github.com/prop/navinamp/commit/bf7cb67ff9760f7d51adc6b706fe65c604d363bb))
* **navidrome:** choose a library for sync ([081317f](https://github.com/prop/navinamp/commit/081317fb7e732d96986e092dbea8dc8e469e378c))
* **navidrome:** make adding a server explicit in settings ([80afbf1](https://github.com/prop/navinamp/commit/80afbf1e87bb3bb951b454efe67511888219049a))
* **navidrome:** multiple server profiles (one active) ([1d2db94](https://github.com/prop/navinamp/commit/1d2db948660cdc5480d9f48e38a17d2e3d9a5ceb))


### Bug Fixes

* **eq:** match curves saved before server-qualified paths ([1f80ee4](https://github.com/prop/navinamp/commit/1f80ee43d773ffde6c5445eddf01fa974b1c519e))
* **navidrome:** accept numeric music-folder ids ([1e882e6](https://github.com/prop/navinamp/commit/1e882e685991a03b8b78bf761f2fd921b9af1504))


### Performance Improvements

* **library:** virtualize the artist/album/genre tree ([8b4423a](https://github.com/prop/navinamp/commit/8b4423a9f9f95582cba17b058425fce6978b8213))
* **library:** virtualize the track table ([8d4f4e9](https://github.com/prop/navinamp/commit/8d4f4e9a5d42fb8ebbc2ea9217d7e3c0778150c6))

## [0.6.0](https://github.com/prop/navinamp/compare/v0.5.0...v0.6.0) (2026-11-01)


### Features

* **audio:** persist volume and EQ across restarts ([05a51ab](https://github.com/prop/navinamp/commit/05a51ab3bf83373a08529be161e1ba18097f734a))
* **engine:** track metadata in audio_state (kbps/kHz/mono-stereo) ([7409152](https://github.com/prop/navinamp/commit/74091525d218c6258598cbaf479e1eeaf6400591))
* **eq:** show the active preset name in the EQ curve panel ([f0c28f4](https://github.com/prop/navinamp/commit/f0c28f430756c4661657ed4ded2143ae3a4c0c19))
* **eq:** user presets and live response curve ([cffcc2a](https://github.com/prop/navinamp/commit/cffcc2a8c20e4b294baf96730e3403a70cbc0f2d))
* **library:** 0-5 star ratings UI with Navidrome sync ([7bee0ea](https://github.com/prop/navinamp/commit/7bee0eaa1f1df67fdc89b634d3f7126883d18c25))
* **library:** show synced artists/albums/genres during a sync ([da1e11a](https://github.com/prop/navinamp/commit/da1e11af63e0f3c07d9a10b5f740caa856a1658e))
* **navidrome:** browse and play server playlists ([9e1637d](https://github.com/prop/navinamp/commit/9e1637da4895c6abcf65ccf27e22b5ae9c4e5fcf))
* **navidrome:** persist favorite (starred) state ([3abf1ec](https://github.com/prop/navinamp/commit/3abf1ece46ece343c2b555e5a2ff37d5ac86f572))
* **navidrome:** publish smart views as server playlists ([507462f](https://github.com/prop/navinamp/commit/507462f3eccfbcff57885a89c3b0a5933e37b897))
* **navidrome:** stream partial sync results into the library ([98912cb](https://github.com/prop/navinamp/commit/98912cb66d3968ddca112e7b872be9d14b1c542c))
* **navidrome:** surface the first library sync ([c5aa92f](https://github.com/prop/navinamp/commit/c5aa92fded07387fe26d115b1d7640ed88ea1d71))
* **skin:** in-app skin switcher ([bef12b8](https://github.com/prop/navinamp/commit/bef12b861e067a2d5455d6c77a722a3f66577b27))
* **ui:** in-app context menu with star and rating actions ([3029a7d](https://github.com/prop/navinamp/commit/3029a7dcb693c9bff6036e9eb5acded3454ea125))
* **ui:** open the library window from the player ([47f4d85](https://github.com/prop/navinamp/commit/47f4d853a5218389299a202c0558a0cd4f71d791))
* **ui:** Window-menu toggles for library, EQ and playlist ([d2a5818](https://github.com/prop/navinamp/commit/d2a581832afba4ca68132963e49fca0136640fd5))


### Bug Fixes

* **eq:** applying a preset auto-enables the EQ stage (was inaudible while disabled) ([9607014](https://github.com/prop/navinamp/commit/9607014dbba07a994885d9af4ed9853a07ba2c12))
* **library:** incremental tree lists (album jumble) ([ec4dc65](https://github.com/prop/navinamp/commit/ec4dc65a053a474763323b67a96cc738f9c71e4f))
* **library:** keep tree rows at full height ([a51ad59](https://github.com/prop/navinamp/commit/a51ad5910be382c4c31712e4442296601cfecc04))
* **navidrome:** accept OpenSubsonic starred timestamps ([918986e](https://github.com/prop/navinamp/commit/918986ede63c380b4b8abdeebb7a451491dbdf86))
* **windows:** include eq in WindowsInfo + serialization test ([8309f55](https://github.com/prop/navinamp/commit/8309f550d0ee23f5652d85bd83d15793d6e00a18))


### Performance Improvements

* **library:** lazy paged table and backend tree lists ([42090e0](https://github.com/prop/navinamp/commit/42090e0efa238e069935595d5a274bb754127c35))
* **library:** stop duplicate 20k-row loads during sync ([d86aaaa](https://github.com/prop/navinamp/commit/d86aaaa889759e41641429aeb9ec2e035beac259))

## [0.5.0](https://github.com/prop/navinamp/compare/v0.4.0...v0.5.0) (2026-10-30)


### Features

* **eq:** skinned EQ window + fallback-skin EQ layout ([3de1dab](https://github.com/prop/navinamp/commit/3de1dab94f727bfa8ff128307845400df4273d72))
* **library:** album art pipeline + covers; fix windows tests for the eq label ([614aea6](https://github.com/prop/navinamp/commit/614aea60d0c65eda0ad5519182dac412e7b14004))
* **windows:** fourth OS window 'eq' (275x116, hidden by default, docked right of playlist) ([3e763d4](https://github.com/prop/navinamp/commit/3e763d439909280d8aef0899ba985c27524d79d3))

## [0.4.0](https://github.com/prop/navinamp/compare/v0.3.0...v0.4.0) (2026-10-30)


### Features

* **ui:** native-app feel — arrow cursor everywhere, no text selection ([53b093d](https://github.com/prop/navinamp/commit/53b093de2a2b89ddde195fec602247a983c9b35e))
* **ui:** pixel-perfect fallback time display + pixel-font chrome ([be777f9](https://github.com/prop/navinamp/commit/be777f9420f30e6f197ebd3c2e88b8a84985e318))


### Bug Fixes

* **skin:** serialize params as a real property over IPC ([7d25206](https://github.com/prop/navinamp/commit/7d252063da7e2b97df1692e15567138ccfe7f038))
* **ui:** playlist editor + time display typography ([a76ca6e](https://github.com/prop/navinamp/commit/a76ca6eb073a6a100b36ee3c4ae62823f7010168))

## [0.3.0](https://github.com/prop/navinamp/compare/v0.2.0...v0.3.0) (2026-10-29)


### Features

* M4A/AAC playback + bitmap fonts + classic playlist theming ([60e40c0](https://github.com/prop/navinamp/commit/60e40c03610b29b175e62d1eade80a89b957c8e0))
* **ui:** always-on-top toggles for player + playlist (library DEV section) ([e8e3751](https://github.com/prop/navinamp/commit/e8e375103d54343e21b86388400797c5786840c9))

## [0.2.0](https://github.com/prop/navinamp/compare/v0.1.0...v0.2.0) (2026-10-29)


### Features

* **skin:** classic Winamp 2 (.wsz) skin support ([bef66c2](https://github.com/prop/navinamp/commit/bef66c2b9ee7031e564c2045100109b65f3acf98))
* **windows:** keep-player-on-top option ([760da30](https://github.com/prop/navinamp/commit/760da30580af1598b8548d4c3517fc5154d9a5e9))
