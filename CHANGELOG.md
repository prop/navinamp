# Changelog

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
