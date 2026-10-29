# NaVinaMP

A Winamp3-style desktop music player built with Tauri 2 + Svelte 5 + Rust.
Whip the llama's ass — locally and over Navidrome.

## Run

```
npm install && npm run tauri dev
```

## Releases

Releases are automated from **Conventional Commits** on `master`:

| Commit message | Version bump |
|---|---|
| `fix: …` | patch (v0.1.**1**) |
| `feat: …` | minor (v0.**2**.0) |
| `feat!:` or `BREAKING CHANGE:` footer | major (v**1**.0.0) |
| `chore:` / `ci:` / `docs:` / `refactor:` … | no release |

[release-please](https://github.com/googleapis/release-please) maintains
`CHANGELOG.md` and opens a **Release PR**; merging it tags `vX.Y.Z`, and CI
builds the macOS `.dmg` on the Mac mini and attaches it to the GitHub
Release (under [Releases](https://github.com/prop/NaVinaMP/releases) — no
sign-in needed to download).

## Navidrome (M5)

Remote servers connect via the Media Library (`navidrome_*` commands).
Credentials and streaming preferences are stored **in plaintext** at
`<app_data_dir>/navidrome.json` (v1 — a keyring backend is planned); treat
the file like a password. Stream URLs carry Subsonic auth tokens in the
query string: the app never logs them and scrubs them from surfaced
errors — don't paste them anywhere either.

## Run

```
npm install && npm run tauri dev
```
