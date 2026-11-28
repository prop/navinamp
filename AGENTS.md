# Agent instructions

## Workflow

- **Open a pull request for every new feature or fix.** Work on a topic branch
  and merge through a PR (`gh pr create --base master`) only after CI passes.
  Never push feature or fix commits directly to `master`.
- **Do not merge a pull request without the user's explicit approval.** Open
  the PR, wait for CI, and report back; merging requires the user's go-ahead.
- **Delete the branch after merging its PR.** Merge with
  `gh pr merge <n> --merge --delete-branch` (or delete the remote branch
  afterwards); never leave a merged topic branch behind.

## Windows and dialogs

- **A dialog fits the window it opens in and always shows a way out.**
  Windows are sized to their content and their page does not scroll, so a
  dialog is bounded by the viewport, not by the pane it belongs to: its
  overlay is `position: fixed; inset: 0` and its panel gets a `max-height`
  a little under `100%`. Centre the panel with flex, not grid: under grid a
  percentage `max-height` is ignored until the panel has outgrown the window.
- **Whatever can outgrow its space scrolls, and the close control stays
  put.** The panel is a flex column: the header and footer (`flex: none`)
  hold the close buttons and only the body between them scrolls
  (`flex: 1; min-height: 0; overflow-y: auto`). Never put the only close
  button inside the scrolling part. The same goes for menus and cards.
  Register Escape with `registerEscape`.
- **Do not lock the user in.** Disabling ×, CLOSE and Escape while an
  operation runs is acceptable only when that operation has a timeout; say
  how long in a comment.
- **Every window shows how it closes.** A secondary window's `TitleStrip`
  carries the × that hides it; a window without the strip needs its own
  control. Only the player (`main`) may quit the app.
- **A window made smaller than its content scrolls.** `src/lib/window/scroll.ts`
  sets `html.clipped` once that lasts 400 ms. Leave `html, body
  { overflow: hidden }` alone and do not work around the class.
- **Measure a new window or dialog before opening its PR.** There is no
  frontend test runner. Boot the real app with Tauri's IPC stubbed, in WebKit
  and in Chromium, at the window's natural size, at a smaller size and at a
  larger UI scale. Check that the panel is inside the viewport, that its
  close control is inside and hit-testable, and that the body scrolls. Put
  the numbers in the PR.
