# Contributing to NaVinaMP

Every change goes through a pull request from a topic branch and is merged
after CI passes. [AGENTS.md](AGENTS.md) has the rules.

## Building from source

None of this is needed to run a released build; see
[Download](README.md#download) in the README.

You need Node.js 20.19+ or 22.12+ with npm, Rust stable, and the
[Tauri 2 prerequisites][tauri-prereq].

```sh
npm install
npm run tauri dev      # development
npm run tauri build    # installers for the current platform
```

CI runs these on every pull request:

```sh
npm run build
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

There is no JavaScript test runner or type-check step.

[tauri-prereq]: https://v2.tauri.app/start/prerequisites/

## Releases

Releases are generated with [release-please] from
[Conventional Commits] on `master`:

| Commit message | Version bump |
|---|---|
| `fix: …` | patch |
| `feat: …` | minor |
| `feat!: …` / `BREAKING CHANGE:` | major |
| `chore:`, `ci:`, `docs:`, `refactor:` | none |

Merging the release PR bumps the version, updates `CHANGELOG.md`, and
creates the tag and the GitHub release. Two follow-ups are needed each
time: the tag is created with `GITHUB_TOKEN`, which starts no workflow, and
release-please does not touch `Cargo.lock`.

1. Run the **CI** workflow on the new tag (Actions → CI → Run workflow). It
   builds and attaches the installers.
2. Run `cargo update -w` and commit the result, otherwise `--locked` builds
   fail.

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
scripts/                 CI runner scripts, cache cleaning, skin curation
.github/                 CI workflows, release-please, version stamping
```
