<p align="center">
  <img src="assets/brand/logo.svg" alt="NaVinaMP logo" width="180">
</p>

<h1 align="center">NaVinaMP</h1>

<p align="center">
  A Winamp-inspired desktop music player for local libraries and Navidrome.
</p>

---

NaVinaMP is a desktop music player built with **Tauri 2**, **Svelte 5**, and
**Rust**. It combines a classic four-window player layout with a modern
library: local folders, Navidrome servers, smart views, skins, an equalizer,
gapless playback, and AI-assisted search.

## Download

macOS builds are attached to GitHub Releases:

- <https://github.com/prop/NaVinaMP/releases>

Download the latest `NaVinaMP_*_x64.dmg`, open it, and copy `NaVinaMP.app`
to `/Applications`.

Windows and Linux can be built from source with the Tauri CLI.

## Highlights

- **Four-window Winamp-style interface**
  - Player, playlist editor, media library, and equalizer
  - Windowshade mode and always-on-top controls
- **Local music library**
  - Incremental scanning by folder
  - Artists, albums, genres, ratings, favorites, and smart views
- **Navidrome / Subsonic support**
  - Multiple saved servers
  - Unified local + remote library
  - Quick and full remote rescans
  - Server playlists and smart-view publishing
  - Ratings and favorites synced to the server
- **Playback engine**
  - Gapless playback
  - ReplayGain
  - 10-band equalizer with user presets and auto-headroom
  - Spectrum analyzer
  - Prefetched upcoming tracks
- **Skins**
  - Classic Winamp 2 `.wsz` support
  - Bundled fallback skin
  - Curated popular-skins pack
- **Search**
  - Full-text search
  - Rule-based smart views
  - AI-assisted search with optional Discogs catalog support
- **Desktop integration**
  - Media keys
  - Desktop notifications
  - Global now-playing shortcut

## Getting started

### Requirements

- Node.js 20+
- Rust stable
- Tauri 2 system dependencies

macOS needs Xcode command-line tools. Linux needs the usual WebKit/GTK
development packages. Windows needs the MSVC toolchain and WebView2.

### Development

```sh
npm install
npm run tauri dev
```

### Production build

```sh
npm install
npm run tauri build
```

### Tests

```sh
cargo test --workspace
npm run build
```

The Rust code is also checked with:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Navidrome

Open the **Media Library** window, choose **NAVIDROME → SERVER STATUS**, and
add a server. NaVinaMP supports multiple saved Navidrome/Subsonic servers;
their libraries are synced into one combined local database.

### Sync modes

- **Quick rescan** pages the album index and fetches only new or changed
  albums.
- **Full rescan** re-fetches every album and is useful after tag edits that do
  not change the album index fingerprint.

### Publishing and scrobbling

Remote tracks support:

- Now-playing and submission scrobbles
- Star/unstar
- Rating sync
- Publishing smart views as Navidrome playlists

## Security notes

- Navidrome credentials are stored **in plaintext** at:
  `<app_data_dir>/navidrome.json`
- Stream URLs contain Subsonic authentication tokens. NaVinaMP does not log
  them and scrubs them from user-facing errors, but do not paste stream URLs
  publicly.
- Servers using self-signed certificates must enable
  **allow self-signed certificates** for that profile. This applies to both
  library sync and streaming.

## Skins

NaVinaMP can load classic Winamp 2 `.wsz` skins. The Media Library’s **SKIN**
tab lists installed skins and a curated popular-skins pack. A fallback skin is
bundled for minimal setups.

Skin files and the selected skin are stored under:

```text
<app_data_dir>/skins
```

## Releases

Releases are generated with [release-please] from
[Conventional Commits] on `master`:

| Commit message | Version bump |
|---|---|
| `fix: …` | patch |
| `feat: …` | minor |
| `feat!: …` / `BREAKING CHANGE:` | major |
| `chore:`, `ci:`, `docs:`, `refactor:` | none |

Release-please opens a release PR. Merging it creates the version tag, and CI
builds and attaches the macOS `.dmg` to the GitHub release.

[release-please]: https://github.com/googleapis/release-please
[Conventional Commits]: https://www.conventionalcommits.org/

## Project layout

```text
src/                     Svelte 5 frontend and window views
src-tauri/               Tauri shell, IPC commands, audio bridge
crates/navinamp-audio    Playback engine: decode, DSP, EQ, output
crates/navinamp-library  Scanner, SQLite library, queries, remote sync
crates/navinamp-subsonic Subsonic/OpenSubsonic API client
crates/navinamp-skin     Winamp skin parsing and validation
crates/navinamp-llm      LLM adapters for AI search
assets/                  Brand assets and bundled skin sources
```

## License

[MIT](LICENSE)
