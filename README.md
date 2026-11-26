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

Installers for macOS, Windows, and Linux are attached to GitHub Releases:

- <https://github.com/prop/navinamp/releases>

| Platform | Files |
|---|---|
| macOS | `NaVinaMP_<version>_x64.dmg` |
| Windows | `NaVinaMP_<version>_x64-setup.exe`, `NaVinaMP_<version>_x64_en-US.msi` |
| Linux | `NaVinaMP_<version>_amd64.deb`, `NaVinaMP-<version>-1.x86_64.rpm`, `NaVinaMP_<version>_amd64.AppImage` |

On macOS, open the `.dmg` and copy `NaVinaMP.app` to `/Applications`.

All installers are x86-64 builds; on Apple Silicon Macs the app runs through
Rosetta 2. At present none of them is code-signed with a developer
certificate. The macOS app is ad-hoc signed and not notarized (CI signs and
notarizes it only when Apple credentials are configured), so Gatekeeper does
not accept it: if macOS refuses to open it, allow the app under
**System Settings → Privacy & Security** (**System Preferences → Security &
Privacy** on macOS 12 or earlier).

## Highlights

- **Four-window Winamp-style interface**
  - Player, playlist editor, media library, and equalizer
  - Windowshade mode and always-on-top controls
  - Edge snapping: docked windows move together until a quick pull separates
    them
- **Local music library**
  - Incremental scanning by folder
  - Artists, albums, genres, ratings, favorites, and smart views
- **Navidrome / Subsonic support**
  - Multiple saved servers
  - Unified local + remote library
  - Quick and full remote rescans
  - Server playlists and smart-view publishing
  - Ratings and favorites synced to the server
  - Album and track label and catalog number from Navidrome's own API
- **Playlist**
  - Follows the current track (on by default)
  - "Play next" in the row menus of the playlist and the library
  - URL streams through **Add URL**; YouTube links need your own `yt-dlp`
    install, which is never bundled
- **Playback engine**
  - Gapless playback
  - ReplayGain
  - 10-band equalizer with user presets and auto-headroom
  - Spectrum analyzer
  - Prefetched upcoming tracks; remote streams reconnect instead of skipping
    the track
  - Queue and playback position restored on the next launch
- **Skins**
  - Classic Winamp 2 `.wsz` support
  - Winamp 2.91 base skin bundled and selected by default
  - Built-in fallback skin
  - Curated popular-skins pack
- **Search**
  - SIMPLE (full-text) and ADVANCED (field rules) modes
  - SEMANTIC search by meaning, with embeddings from an OpenAI-compatible
    provider (Anthropic has no embeddings API)
  - AI search: describe the music and the model writes the rules
  - Rule-based smart views and optional Discogs catalog lookups
- **Desktop integration**
  - Media keys and Now Playing metadata
  - Desktop notifications
  - Global now-playing and next-track shortcuts

Two playback preferences are checkable items of the **Window** menu: **Notify
on Track Change** (on by default) and **Resume Playback on Launch** (off by
default). The queue, its shuffle and repeat settings, and the position of the
current track are restored on the next launch, and Play continues from there.
With resume on, a track that was playing when the app quit also starts again.

## Keyboard shortcuts

| Where | macOS | Windows / Linux | Action |
|---|---|---|---|
| Window menu | ⌘⇧L | Ctrl+Shift+L | Media Library |
| Window menu | ⌘⇧E | Ctrl+Shift+E | Equalizer |
| Window menu | ⌘⇧P | Ctrl+Shift+P | Playlist Editor |
| Window menu | ⌘⇧T | Ctrl+Shift+T | Always on Top |
| Player window | Alt+L | Alt+L | Toggle the Media Library |
| Any window | Alt+S | Alt+S | Cycle the UI scale |
| Playlist window | ⌘U | Ctrl+U | Add URL |
| Playlist list | ↑ / ↓ | ↑ / ↓ | Move the selection |
| Playlist list | Delete / Backspace | Delete / Backspace | Remove the selected row |
| Any window | Esc | Esc | Close the top dialog or menu; hide a secondary window |
| Global | Ctrl+⌘O | Ctrl+Alt+O | Show now playing |
| Global | Ctrl+⌘N | Ctrl+Alt+N | Next track |

The Window-menu accelerators work from any NaVinaMP window. The UI scale steps
through 1×, 1.25×, 1.5× and 2×; the same choice is in the Media Library's
**DEV** tab.

## Getting started

### Requirements

- Node.js 20.19+ or 22.12+ (the minimum for Vite 8; CI uses Node 22) and npm
- Rust stable (`rust-toolchain.toml` selects the channel, so rustup installs
  it automatically)
- Tauri 2 system dependencies

macOS needs Xcode command-line tools. Windows needs the MSVC toolchain and
WebView2. Linux needs WebKitGTK and a few more packages; on Ubuntu 24.04, CI
installs:

```sh
sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev \
  libasound2-dev patchelf
```

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

`tauri build` produces the bundles configured in `src-tauri/tauri.conf.json`
for the platform it runs on.

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

CI runs these four steps on every pull request. There is no JavaScript test
runner or type-check step, so `npm run build` is the only frontend check.

## Navidrome

Open the **Media Library** window, choose **NAVIDROME → SERVER STATUS**, and
add a server. NaVinaMP supports multiple saved Navidrome/Subsonic servers;
their libraries are synced into one combined local database.

### Sync modes

- **Quick rescan** pages the album index and fetches only new or changed
  albums.
- **Full rescan** re-fetches every album and is useful after tag edits that do
  not change the album index fingerprint.

### Release metadata

Albums in the library tree and the track card at the top of a row's context
menu show the label and catalog number when the server provides them. They
come from Navidrome's own API rather than from Subsonic, so other Subsonic
servers do not have them. The same API supplies the real file path that
**Copy path** puts on the clipboard.

### Publishing and scrobbling

Remote tracks support:

- Now-playing and submission scrobbles
- Star/unstar
- Rating sync
- Publishing smart views as Navidrome playlists

## Data and settings files

NaVinaMP keeps its files in two directories named after the bundle
identifier, `com.prop.navinamp`. With the current Tauri and `dirs` versions
they are the same directory on macOS and Windows, and separate directories on
Linux:

| | macOS | Linux | Windows |
|---|---|---|---|
| `<app_config_dir>` | `~/Library/Application Support/com.prop.navinamp` | `$XDG_CONFIG_HOME/com.prop.navinamp` (default `~/.config/…`) | `%APPDATA%\com.prop.navinamp` |
| `<app_data_dir>` | same directory | `$XDG_DATA_HOME/com.prop.navinamp` (default `~/.local/share/…`) | same directory |

`<app_data_dir>` holds:

- `navinamp.db`: the library database
- `library-roots.json`: the configured scan roots
- `art-cache/`: cached album art
- `navidrome.json`: saved servers, credentials, and streaming preferences
- `queue.json` and `playback.json`: the play queue and the current playback
  position, restored on the next launch

`<app_config_dir>` holds:

- `audio.json`: volume, equalizer state, user presets, and the notification
  and resume preferences
- `windows.json`: position, size, visibility, windowshade layout, and
  always-on-top state of the windows
- `ai.json`: AI search provider settings, including API keys
- `streams.json`: the path of the `yt-dlp` executable, written when you set
  one in the Add URL dialog
- `skins/`: installed skin archives

A few interface preferences, such as the selected skin, the UI scale, and the
playlist's follow mode, are kept in the webview's local storage instead of
these files.

## Security notes

- Navidrome credentials are stored **in plaintext** at:
  `<app_data_dir>/navidrome.json`
- AI search keys and the optional Discogs token are stored **in plaintext**
  at `<app_config_dir>/ai.json`.
- The stored Navidrome password is also used to sign in to Navidrome's own
  API, for label, catalog number, and file path lookups. That sign-in sends
  the password itself rather than a salted token, so prefer an `https://`
  server URL. The sync skips it for `http://` servers, but **Copy path** and
  the playlist row menu do not.
- Stream URLs contain Subsonic authentication tokens. NaVinaMP does not log
  them and scrubs them from user-facing errors, but do not paste stream URLs
  publicly.
- Servers using self-signed certificates must enable
  **allow self-signed certificates** for that profile. This applies to both
  library sync and streaming.
- With AI features enabled, library metadata goes to the configured provider.
  AI search sends a compact digest: counts, genres, and sampled artist names,
  plus for each saved server its display name (`user@host` unless you set a
  label), its id, and its track count. SEMANTIC indexing sends each track's
  title, artist, album, genre, and year, or the file name without its
  extension when all of those are empty.

## Skins

NaVinaMP can load classic Winamp 2 `.wsz` skins. The original Winamp 2.91
base skin ships with the app and is selected by default; a minimal fallback
skin is built in as a last resort. Skins are rendered by the player and
equalizer windows.

The Media Library's **SKIN** tab lists the installed skins, with **APPLY** to
switch and a rescan button for new files. **INSTALL POPULAR SKINS** downloads
a curated pack from the Winamp Skin Museum on archive.org. The app ships only
that pack's manifest; skins are downloaded only when you ask.

To add a skin, copy its `.wsz` file into the skins folder and rescan:

```text
<app_config_dir>/skins
```

The selected skin is remembered by the webview (local storage), not in that
folder.

## Releases

Releases are generated with [release-please] from
[Conventional Commits] on `master`:

| Commit message | Version bump |
|---|---|
| `fix: …` | patch |
| `feat: …` | minor |
| `feat!: …` / `BREAKING CHANGE:` | major |
| `chore:`, `ci:`, `docs:`, `refactor:` | none |

Release-please keeps a release PR open. Merging it bumps the version, updates
`CHANGELOG.md`, and creates the tag and the GitHub release.

The tag is created with the default `GITHUB_TOKEN`, which does not start a
workflow, so the release has no installers at that point. Start the **CI**
workflow on the new tag (Actions → CI → Run workflow, then pick the tag). The
workflow then builds and attaches the installers listed under
[Download](#download): the macOS build runs on a self-hosted runner, the
others on GitHub-hosted runners. A final job fails the run if any of the six
files is missing.

Release-please does not update `Cargo.lock`. After merging, run
`cargo update -w`, which updates the workspace members only, and commit the
changed version lines in a separate PR; otherwise `--locked` builds fail.

[release-please]: https://github.com/googleapis/release-please
[Conventional Commits]: https://www.conventionalcommits.org/

## Project layout

```text
src/                     Svelte 5 frontend: window views, components, player queue
src-tauri/               Tauri shell: IPC commands, audio bridge, windows, menus
src-tauri/resources/     Bundled base skin and the popular-skins manifest
crates/navinamp-audio    Playback engine: decode, DSP, EQ, output, remote streams
crates/navinamp-library  Scanner, SQLite library, queries, remote sync
crates/navinamp-subsonic Subsonic/OpenSubsonic client and Navidrome's native API
crates/navinamp-skin     Winamp skin parsing and validation
crates/navinamp-llm      LLM adapters for AI search
assets/                  Brand assets and the fallback skin's sources
scripts/                 CI runner bootstrap, cache cleaning, skin curation
.github/                 CI (tests and release builds), release-please, and the
                         release version-stamping action
```

## Contributing

Every change goes through a pull request from a topic branch and is merged
after CI passes. [AGENTS.md](AGENTS.md) has the workflow;
[CHANGELOG.md](CHANGELOG.md) lists what changed in each release.

## License

[MIT](LICENSE)
