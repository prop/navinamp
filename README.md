# NaVinaMP

A Winamp3-style desktop music player built with Tauri 2 + Svelte 5 + Rust.

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
