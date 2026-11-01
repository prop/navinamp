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

## Next up (priority order)

1. **Persisted volume + EQ** across restarts (localStorage or config file).
2. **In-app skin switcher** (currently: drop `.wal` in the skins folder +
   restart; the loader picks alphabetically).

## Known follow-ups / deferred

- EQ: AUTO button (visual only), preset saving, live curve rendering,
  EQ window close button (decorative), duplicate HTML EqPanel in the
  library DEV section
- Metadata bitrate is average source bytes ÷ duration; streams whose
  container declares no duration (some transcodes) leave the kbps digits
  blank by design
- Publishing a smart view only includes `source=navidrome` rows (local
  files have no Subsonic song id); playlist creation uses the
  `[NaVinaMP] ` name prefix, and republishing keeps the server-side name
- Navidrome favorite stars are still session-local (no `starred` column in
  `tracks`); rating stars are the persisted path
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
