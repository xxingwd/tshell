# Release Notes

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
