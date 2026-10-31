# Release Notes

## 0.1.5 — 2026-10-31

- Added offline SVG and standalone Mermaid (`.mmd`, `.mermaid`) previews on
  Windows with zoom, fit, source editing and recoverable syntax errors.
  Preview scripts are bundled; source cannot load external resources. See
  [file previews](file-previews.md) for limits and platform fallback behavior.
- Preserve PDF binary/read-only state across renames and ignore source-line
  navigation for PDF files. Restrict preview navigation to the active document.

## 0.1.4 — 2026-10-25

- Fixed a Windows crash when opening HTML files such as `index.html`.
  WebView2 initialization now runs outside GPUI state updates so its nested
  message loop cannot reenter borrowed application state. A new preview stays
  hidden until the current HTML view is ready to display it.

- Removed unused application dependency declarations left after tool-crate
  extraction, an unused Docker UI-check accessor, and obsolete English/Chinese
  translation keys. Removed an obsolete theme-selection wrapper and confined
  UI-check imports to their debug-only scope. Localization remains owned by
  `tshell-ui`.

- Confirmed session refreshes clean deleted sessions from saved ordering; host
  deletion clears associated ordering records. Connection gaps preserve orders.
  Removed the unused container-as-host-session path and the obsolete internal
  tmux session-record format, along with unused session/window action variants
  and their obsolete translations and tests.

- Hosts and sessions default to creation order, including tmux sessions. Hosts
  can be moved up/down in Settings; session headings support drag reordering
  and context-menu Move Up/Down. Custom orders persist per host and apply to
  pickers, the command palette and numbered navigation.

- New-session directory lists adapt to window height and show up to 14 rows.
  Empty validation feedback no longer reserves a separate area. Local and SSH
  completion no longer truncate matches to eight directories.
- Command and forwarding lists can grow to a 560-pixel body, constrained by the
  window. Filtered command results shrink with their content, forwarding omits
  empty feedback space, and local-host rename uses a compact form.
- Transfer panels grow with the queue instead of stopping at four tasks. Docker
  related-container lists use remaining detail height, and JSON action forms
  adapt to window size with scrollable overflow.
- tmux pane dimensions are calculated in the client and submitted as complete
  layouts. Splitting down once and then right twice keeps columns equal, and
  closing the appended columns restores the original rows. This avoids tmux
  3.8 sibling-spreading errors and repairs existing column layouts that extend
  beyond the window.
- Connecting to an existing unequal tmux layout starts control channels before
  waiting for layout repair, avoiding a permanent "Connecting" state.
- tmux connection progress now reports SSH discovery, control-channel startup,
  and layout recovery phases in the empty terminal surface. Failed layout
  commands retry twice after a fresh snapshot; a window that cannot fit equal
  panes reports the reason instead of submitting zero-sized cells.
- Rapid focus and pane-selection actions are coalesced while server snapshots
  settle, reducing stale-target requests during keyboard bursts.

Local validation passed 260 workspace tests, formatting, UI-feature compilation,
and the scoped native HTML-preview and navigation-order checks. The 21 opt-in
external-environment tests were skipped. The full hidden-window UI check stalled
after reporting `window not found`; a complete native UI pass is not claimed.

## 0.1.3 — 2026-10-17

- Remote session creation waits for directory validation when Enter or the
  confirm button is used. Failed validation keeps the form open; repeated
  submissions are ignored, and editing, closing, or switching hosts cancels
  a pending submission.
- Switching hosts or metric selections releases SSH collector channels on a
  persistent runtime, preventing stale collectors from exhausting remote
  session capacity and making the status-bar metrics disappear.
- The light terminal palette uses darker ANSI yellow and white entries so
  PowerShell commands and arguments remain visible on a white background.
  Unmodified legacy built-in light palettes migrate when loaded; custom ANSI
  values are preserved.
- Selecting a private-key file fills its path in the standalone Add/Edit Host
  dialog as well as the dialog opened from Settings.
- Docker follows the workspace host selector independently of terminal
  sessions. Resource trees, tables, adjacent previews, container exec terminals,
  logs, Compose and daemon configuration editors preserve per-host navigation
  and unsaved drafts. Docker communicates with the Engine through Bollard.
- Native SSH forwarding supports local, remote, and SOCKS rules. Explorer, Git,
  Docker, SSH transport, shared controls, and localization now have dedicated
  workspace crates.
- Terminal color controls support common OSC palette/default-color queries,
  overrides and resets, plus opt-in theme reports. Loading and validation
  feedback keep terminal grids and form controls stable.

Local validation passed the normal workspace tests, UI-feature compilation,
the four host-metrics integration tests, and the scoped remote session-submission
UI check against a passwordless Linux SSH host. The full hidden-window UI suite still stops at its
`Alt+1` session-shortcut assertion; a complete native UI pass is not claimed.

See [workspace behavior](workspace-interface.md), [host metrics](host-metrics.md),
[terminal themes](custom-terminal-theme.md), [Docker](docker-workspace.md),
and [Windows packaging and updates](windows-release.md).

## 0.1.2 — 2026-10-04

- Terminal splits now use one layout: equal-width columns and equal-height rows
  within each column. `Alt+N` adds a column on the right; `Alt+Shift+N` adds a row
  below the active pane. Closing a pane or exiting its shell redistributes the
  remaining group automatically.
- tmux redistributes sibling groups through native layout commands, replacing
  sequential pane-size adjustments. Queued workspace actions use the preceding
  server snapshot. Pane exits, window resizes, and focus-mode restoration can
  recover column layouts whose previous sizes had drifted.
- Split dividers use restrained one-pixel strokes and stable focus highlights.
  Divider dragging, `Alt+-` / `Alt+=` pane resizing, and `Alt+T` layout cycling
  are removed. Terminal font-size shortcuts and focus/floating modes remain
  available.
- Switching to Explorer or Git restores workspace keyboard focus, keeping
  terminal search reachable. Local session creation resolves a valid directory
  immediately when asynchronous path completion has not finished. Remote
  connection errors expose a reconnect action.
- Native checks cover mixed splits and close redistribution in dark and narrow
  light windows, alongside the existing focus, search, selection, and file checks.

Local validation passed 199 tests and the isolated Windows native UI check.
Remote tmux integration tests require `TSHELL_SSH_TEST_HOST` and were not run
locally for this release. External tmux layouts with other topologies remain
server-owned; this release does not claim support for every tmux layout or new
xterm.js compatibility coverage.

See [equal split behavior](workspace-interface.md#equal-splits) and
[Windows packaging and updates](windows-release.md).
