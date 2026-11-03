# NaVinaMP — current status & next steps

> Handoff note for the next development session. Last updated: 2026-11-03,
> after v0.10.0.

## State

- All six roadmap milestones shipped. Latest release: **v0.10.0**
  (dmg on the Releases page; built successfully by the cached self-hosted
  runner after clearing a root-owned npm cache — see the gotchas below).
  v0.10.0 dmg SHA-256: `1eb43a1a9710bd0182939eac7738677da777a49d6ca8a8f62d9e3f606764e506`.
- Test suites green across six crates + frontend. Clippy/fmt clean.
- Four windows: player (skinned), playlist editor, media library, EQ.

## Just shipped (v0.5.0 + master since)

- **v0.10.0**: first-run onboarding tour + What's-new tours, offline AI search
  demo and provider wizard, windowshade fold/unfold, desktop notifications and
  the global now-playing shortcut, long-press transport seek, library tabs,
  the curated popular-skins pack, and CP1251/KOI8-R tag repair.
- **v0.8.0**: per-server Navidrome playlists + publish-target picker, sticky
  A–Z tree rails, the EQ AUTO curve manager, EQ clipping/headroom fix with
  Bass Boost, selection-aware row context menus and global Always on Top.
- **v0.9.0**: the equalizer's automatic headroom is now visible as a live
  `auto headroom −X.X dB` readout.
- **v0.9.0**: three-mode library search — SIMPLE (full-text),
  ADVANCED (field rules) and AI (LLM-built rules, Discogs catalog lookups) —
  with save-to-smart-view, publish-to-server and replace-playlist actions.
  Detailed notes near the end of this section.
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
  player for every skin (classic `.wsz` skins have no library button).
- Always on Top: the Window menu's checkable item (**⌘⇧T**) puts all four
  windows above other apps at once. The per-window flag is persisted in
  `windows.json` (`onTop`) and restored on launch; the old DEV checkboxes
  were removed.
- Context menus act on the whole table selection: right-clicking a selected
  row gives “Play/Enqueue N selected” (virtual SELECT ALL included);
  right-clicking an unselected row makes it current and acts on it alone.
  Copy/star/rating/EQ actions stay per-row.
- The Media Library window keeps its title strip fixed and scrolls the
  library/EQ/skin/DEV panels, so the lower controls are reachable in a
  short window instead of being clipped.
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
- Library loads lazily: startup fetches only stats/roots/views; the table
  pages 500 rows on entry and appends on scroll; artist/genre/album tree
  lists load when a group expands and render only the visible window;
  counts come from per-source stats. No 20k-row master snapshot and no
  sync-time aggregate reload.
- Navidrome sync still writes rows in album batches; the UI now refreshes
  only counts (2s) and the tree groups that are actually open (15s);
  pruning only runs after a complete walk.
- Track list containment: the 320px table now clips/scrolls its rows instead
  of painting over the footer and EQ panel (overscroll contained)
- Track table virtualization: only the visible ~20 rows (+8 overscan) are in
  the DOM; top/bottom spacers preserve the scroll height. Page loading still
  appends as you scroll.
- EQ AUTO works: each track change applies the curve saved for that track,
  else the one saved for its artist, else a preset named after its genre
  (built-in curves or user saves). Track/artist curves are captured from the
  library and playlist row context menus (the live EQ bands are stored in
  localStorage); saving an artist curve replaces a track curve for that row,
  and clear removes both. The AUTO flag persists in localStorage.
- EQ headroom: the cascade estimates its composite worst-case gain whenever
  parameters change and attenuates the pre-EQ signal, so boosted classic
  presets (Rock, Full Treble, …) and single +12 dB bands can no longer
  hard-clip the output. The tightly-spaced 6k/12k/14k/16k bands also use
  Q=8 (low/mid bands Q=2) so three adjacent treble boosts don't multiply
  into a +30 dB spike; Rock's headroom is now ~-13 dB instead of ~-28 dB.
  Volume sits after the EQ, so loudness can be raised back without
  reintroducing distortion. “Bass Boost” (+6/+5/+4/+2 at 60–600 Hz) was
  added to the preset table. The panel now shows the computed attenuation
  (`auto headroom −X.X dB`) under the response graph and dims it while the
  EQ is bypassed; `eq_headroom` is stateless (the UI sends the displayed
  bands) so the readout tracks preset applies and slider releases exactly.
- Navidrome library selection: `getMusicFolders` lists the user's libraries,
  the selected id (`libraryId`) persists in `navidrome.json` and scopes the
  sync walk via `musicFolderId`; Navidrome Settings has an all-libraries /
  library dropdown that re-syncs (the prune drops rows outside the choice).
- Multiple Navidrome servers are aggregated: every saved profile syncs into
  the unified `tracks` table; rows carry `server_id` and paths are
  `navidrome://<profile>/<remote_id>`, so identical remote ids stay distinct.
  Playback, scrobbling, stars, ratings and cover art route to the row's
  server; the NAVIDROME tree lists each server with its count and selecting
  one filters the table. REMOTE TRACKS shows the union; SYNC NOW walks all
  servers (an unreachable one is reported and skipped). The active profile
  is only the default for publishing and the status card; removing it
  promotes the first remaining server to active. `navidrome.json`
  v1 files migrate to one `default` profile; existing remote rows/paths are
  qualified on first boot. Publish mappings are keyed per server
  (`smart_view_playlists.server`); legacy mappings move to the migrated
  profile on first boot.
- Navidrome playlists are per server: the NAVIDROME tree lists each saved
  server's live playlists under its name (fetched independently in parallel,
  with a per-server error/loader). Playlist rows carry that server id, so
  playback/star/rating route correctly. The smart-view PUBLISH button has a
  server picker (active preselected) and each target updates its own
  per-server playlist mapping.
- Navidrome Settings UX: saved servers and the add form share one SERVERS
  block. The form starts empty as “+ ADD SERVER” after every completed
  action; typing a saved `url` + `user` flips it to UPDATE SERVER or
  RECONNECT with the matching button and consequence hint. Rows show the
  last sync, and DISCONNECT was folded into REMOVE on the active row.
- Release-build devtools enabled (tauri `devtools` feature); inspect works in
  packaged builds too.
- svelte-check is clean (0 errors). The old 14-diagnostic baseline was a
  variable named `state` colliding with the `$state` rune (parsed as a store
  subscription); renamed to `audio`, plus InvokeArgs typings.
- SELECT ALL is view-wide: in paged views it covers every row matching the
  view (not just the loaded pages), and ENQUEUE SEL fetches the full filtered
  set on demand. Any explicit row click leaves the virtual all-mode; clicking
  SELECT ALL again (DESELECT ALL) clears it.
- Paged views: shift-range selection is limited to loaded rows. Deselecting a
  single row out of a 20k-row SELECT ALL isn't representable, so a row click
  replaces the virtual selection.
- Tree lists are virtualized too: ARTISTS / ALBUMS / GENRES keep their full
  scroll height with only the visible rows (+8 overscan) in the DOM, using
  fixed-height leaf slots. Expanding/collapsing groups recomputes offsets;
  the old 200-row chunk extension is gone.
- Long tree lists (30+ rows) get a sticky two-row A–Z rail: present initials
  only, accents folded (É → E) and digits/symbols/other scripts grouped under
  `#`, so the 14×2 cells always fit the 176px column. The active initial
  tracks the viewport; clicking one scrolls that group's first matching row
  just below the rail. The rail height is included in the virtual list math.
- The library's equalizer panel has a collapsible AUTO CURVES manager: saved
  track and artist curves are listed with a tiny band preview, APPLY loads
  one into the live EQ, and × deletes just that entry. Track titles are
  captured on save; older entries resolve lazily through the library (path
  basename fallback). Deletions broadcast the usual clear event, so the
  playing track falls back to its artist curve or genre preset immediately.
  The manager is library-only; the EQ window's HTML fallback stays compact.
- Search modes share one result table:
  - SIMPLE keeps the FTS5/bm25 path; `tracks_fts` now also indexes
    album_artist and path (navidrome://server/id), rebuilt once on upgrade.
  - ADVANCED is the Smart View rule engine surfaced inline (AdvancedSearch)
    with new `albumArtist`, `codec`, `starred` (boolean) and `server` fields;
    `smart_view_query` runs an unsaved definition with an explicit 5000-row
    cap.
  - AI sends a bounded library digest (counts, genres, sampled artists, server
    names/ids) and the rule schema to the configured provider. The model may
    call Discogs (structured artist/release/label/genre/style/year/format
    filters, paged up to 100/call, max 4 calls, personal token required for
    catalog calls) and
    can finish with a Smart View definition, `def: null` for catalog-only
    answers, or both. Catalog entries are a first-class result: the UI shows
    a scrollable Discogs panel with COPY LIST, while library rules are
    validated locally with one repair pass. EDIT RULES moves the generated
    definition into the Advanced builder; REFINE reuses it.
  - Results can be SAVED as a smart view (simple mode synthesizes path rules),
    published as a `[NaVinaMP]` server playlist (new publish-by-name command),
    or used to replace the playlist editor (new queue `replaceAll` intent).
    Catalog-only answers keep the library table empty and expose COPY LIST.
  - Post-audit hardening: catalog totals sum distinct filter sets; the model
    payload is capped at 25 releases and wrapped as untrusted data; a missing
    Discogs token short-circuits with an explicit settings hint; `per_page`
    defaults to 100.
- AI settings (`ai.json`, plaintext like navidrome.json): provider presets for
  OpenRouter, OpenAI, Claude, DeepSeek, GLM, Kimi, MiniMax and a custom
  OpenAI-compatible endpoint (covers local Ollama/LM Studio), plus a Discogs
  token (required for catalog lookups). The model picker lists current ids
  (DeepSeek V4.1 Flash is
  `deepseek-flash`, GLM-5.3-Flash, Kimi K3.1 Pro, MiniMax M3.1 Flash, GPT-5.2,
  Claude Sonnet 5) and REFRESH fetches the provider's live `/models` list.
  `navinamp-llm` has OpenAI-compatible and Anthropic adapters;
  the key never reaches the model or logs. Embedding/vector search is
  deferred — GitHub issue #15 has the design.

## Next up

The backlog lives in GitHub issues — `gh issue list` (labels:
`enhancement`, `bug`). Nothing is staged for release right now; cut the next
version when the next batch lands.

## Design notes & deferred

- Metadata bitrate is average source bytes ÷ duration; streams whose
  container declares no duration (some transcodes) leave the kbps digits
  blank by design
- Publishing a smart view only includes `source=navidrome` rows (local
  files have no Subsonic song id); playlist creation uses the
  `[NaVinaMP] ` name prefix, and republishing keeps the server-side name
- Removing a server profile drops its per-server playlist mappings and
  its profile id; re-adding the same server later gets a fresh id, so the
  next publish creates a new `[NaVinaMP]` playlist instead of updating the
  old one
- Deferred by decision: multi-platform CI releases, MAKI runtime,
  video window, Shoutcast

## Gotchas

- `GITHUB_TOKEN`-created tags don't fire CI. After merging a release PR,
  push the tag from a local checkout:
  `git push origin :refs/tags/vX.Y.Z 2>/dev/null; git push origin vX.Y.Z`
- Deleting/re-pushing a release tag can detach the GitHub Release into an
  untagged draft. Re-publish it with
  `gh release edit vX.Y.Z --draft=false --tag vX.Y.Z`.
- The self-hosted runner's npm cache can contain root-owned directories,
  making `npm ci` fail with `EACCES ... _cacache`. Move it aside
  (`mv ~/.npm/_cacache ~/.npm/_cacache.root-owned-backup`) and rerun the
  failed release job; this is what unblocked v0.10.0.
- Self-hosted mac-mini release builds: a cold tauri release build can
  starve the 2014 2-core/8 GB runner — rustc stalls its heartbeat for 10+
  minutes and GitHub cancels the job (later `CompleteJob` returns "job not
  found"). `ci.yml` caches the release target and caps
  `CARGO_BUILD_JOBS=2`; v0.8.0, v0.9.0 and v0.10.0 built, packaged and
  uploaded on the runner this way (each ~16–18 minutes). v0.7.0 predated the
  cache and needed a local build. If a tag job still stalls, build locally:
  `CARGO_BUILD_JOBS=2 npm run tauri build -- --bundles app`, package with
  the workflow's `hdiutil` command, and
  `gh release upload vX.Y.Z <dmg> --clobber`.
- Empty-string GitHub secrets reach the Tauri CLI and break signing —
  the release job filters them (see ci.yml "Prepare signing environment").
- Session resumption: OpenCode sessions live on this machine
  (`~/.local/share/opencode`); resume with `opencode --resume` from
  `/Users/prop/develop/navamp` (symlink to `navinamp`).
