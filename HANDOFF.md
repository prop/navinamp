# NaVinaMP — current status & next steps

> Handoff note for the next development session. Last updated: 2026-11-01,
> after the v0.5.0 release.

## State

- All six roadmap milestones shipped. Latest release: **v0.5.0**
  (dmg on the Releases page; CI builds it automatically on `v*` tags).
- Test suites: 25+ green across five crates + frontend. Clippy/fmt clean.
- Four windows: player (skinned), playlist editor, media library, EQ.

## Just shipped (v0.5.0 + master since)

- M4A/AAC playback (decoder priming for undeclared channel layouts)
- Classic Winamp 2 `.wsz` skin support (base-2.91 renders)
- Bitmap-glyph fonts (text.bmp/numbers.bmp), pledit.txt colors in the PE
- Queue persistence, recursive folder adds, always-on-top toggles
- Runaway-window guard, native cursor/no-selection
- Semantic releases (release-please) + Mac mini CI runner
- Engine metadata in `audio_state` (bitrate, source sample rate, channels):
  classic kbps/kHz digits and mono/stereo indicators are live
- Ratings UI: 0–5★ in the library table — local DB write for every row,
  plus a Navidrome `setRating` push for remote rows; a sync imports server
  ratings into unrated rows without clobbering local ratings
- Navidrome playlists: browse/play server playlists in the NAVIDROME tree
  (live, never imported into the DB) and PUBLISH a smart view from its
  editor — first publish creates `[NaVinaMP] <name>`, later publishes
  update it in place (mapping in `smart_view_playlists`)
- Window menu: checkable **Media Library / Equalizer / Playlist Editor**
  toggles (⌘⇧L / ⌘⇧E / ⌘⇧P), checkmarks kept in sync with actual
  visibility; plus the fallback player's LIBRARY toggle and Alt+L in the
  player for every skin (classic `.wsz` skins have no library button)
- Persisted volume + EQ in `<app_config_dir>/audio.json`: loaded before
  the engine boots, replayed on first playback, and written with a
  debounced coalescing thread (slider drags → one write per quiet window)
- Navidrome first-connect UX: a settings change auto-starts the library
  sync when no remote rows exist, and the REMOTE TRACKS empty state offers
  SYNC LIBRARY NOW / RETRY with live progress and the last sync error
- In-app skin switcher in the library window: lists the built-in fallback +
  installed `.wal`/`.wsz` files, validates before applying, persists the
  choice in localStorage, broadcasts `skin-changed`, and the player + EQ
  windows reload live (playlist colors follow via `skin-theme`)
- Navidrome favorites persist: `tracks.starred` (schema migration for old
  DBs) imports the server's `starred` flag on every sync, is mirrored
  locally after each star click, and shows in every view/playlist
- EQ polish: user presets save/delete in the library panel, persisted in
  `audio.json`, shared with the skinned PRESETS cycle; the classic EQ graph
  now draws the live response curve
- Context menus (playlist + library rows): Play/Remove/Enqueue, star,
  rating, Copy path; the stock webview menu is replaced on those rows
- Navidrome sync renders incrementally: rows are upserted to the library in
  album batches during the walk; the REMOTE/ALL views and tree come from one
  master snapshot (refreshed at most every 15s, never overlapping), header
  stats every 2s, filtered panes at most every 5s; pruning only runs after a
  complete walk
- Track list containment: the 320px table now clips/scolls its rows instead
  of painting over the footer and EQ panel (overscroll contained)

## Next up

1. **EQ AUTO mode** — needs a product definition (per-track auto-load);
   the toggle is still visual-only.
2. Release-build devtools, chunked virtualization, svelte-check/TS7 cleanup.

## Known follow-ups / deferred

- EQ: AUTO button (visual only — no per-track auto-load concept yet),
  EQ window close button (decorative), duplicate HTML EqPanel in the
  library DEV section
- Metadata bitrate is average source bytes ÷ duration; streams whose
  container declares no duration (some transcodes) leave the kbps digits
  blank by design
- Publishing a smart view only includes `source=navidrome` rows (local
  files have no Subsonic song id); playlist creation uses the
  `[NaVinaMP] ` name prefix, and republishing keeps the server-side name
- `Warning::ClassicMissingOptional` missing from the frontend `Warning`
  union in `src/lib/skin/types.ts`
- Devtools in release builds (needs the tauri `devtools` feature)
- Chunked virtualization for 100k-row lists; svelte-check/TS7 baseline
- Signing ($99/yr) for prompt-free installs; auto-updater after that
- Deferred by decision: multi-platform CI releases, MAKI runtime,
  video window, Shoutcast

## Gotchas

- `GITHUB_TOKEN`-created tags don't fire CI. After merging a release PR,
  push the tag from a local checkout:
  `git push origin :refs/tags/vX.Y.Z 2>/dev/null; git push origin vX.Y.Z`
- Empty-string GitHub secrets reach the Tauri CLI and break signing —
  the release job filters them (see ci.yml "Prepare signing environment").
- Session resumption: OpenCode sessions live on this machine
  (`~/.local/share/opencode`); resume with `opencode --resume` from
  `/Users/prop/develop/navamp` (symlink to `navinamp`).
