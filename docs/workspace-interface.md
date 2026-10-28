# Workspace Interface

TShell uses a compact native tool interface. GPUI Kit owns control behavior,
keyboard focus, menus, and selected states. Workspace styling uses the active
scheme's semantic interface tokens; terminal font metrics and protocol rendering
remain owned by the terminal view.

## Module Ownership

The root Cargo package composes the application. `tshell-ui` owns shared controls,
semantic palettes and localisation; `tshell-host` owns SSH, authentication requests,
forwarding, SFTP and file operations, with no GPUI or tmux dependency. Application
host names and tmux preferences live in `src/host.rs`, separate from `SshConfig`;
the saved host JSON retains its existing flat fields. Host errors preserve typed
messages and their original causes until the UI formats them in the selected language.

`tshell-explorer`, `tshell-git` and `tshell-docker` own their native GPUI entities,
business state and IO. The root uses `Page::{Terminal, Explorer, Git, Docker}` and
one `SessionWorkspace` per stable host/session identity; Explorer/Git are created
lazily and preserve documents and diffs in their owning entities. Host IDs migrate
old endpoint-based session/forwarding keys without requiring manual config edits.

All four entries stay in the left sidebar and share the host selector. Docker shows
the selected host's Engine and container list; it does not depend on terminal session
selection and remains usable with no terminal sessions. Switching hosts in Docker
does not create a terminal session. See
[Docker workspace](docker-workspace.md) and [the implementation plan](../todo.md).

## Shared Layout

- Familiar commands use Lucide icons with localized tooltips. Shared icon
  controls occupy 28 x 28 pixels, toolbars are 36 pixels high, and
  Explorer and Git navigation rows are 30 pixels high. Window chrome retains its existing
  geometry.
- The session sidebar uses compact 28-pixel text rows. Session headings toggle
  collapse; thin separators distinguish tabs, and an accent marker identifies
  the selected pane. Pane rows have no leading icons or extra tab headings.
  Session groups use 4 pixels of top spacing; pane rows have 1-pixel vertical
  margins and tab separators have 2-pixel vertical margins.
  Click targets and hover actions have fixed dimensions.
- Hosts start in creation order; Settings > Hosts provides up/down controls,
  including for the local host. Saved display order also applies to host pickers,
  the command palette, and host keyboard navigation. New hosts append at the end.
  Sessions start in creation order, with tmux creation timestamps read from the
  server and numeric session IDs breaking timestamp ties. Drag a session heading
  onto another heading to move it to that position; the drop marker appears below
  the target when moving down and above it when moving up. The session context
  menu also provides Move Up/Down. Orders persist per host across refreshes,
  renames, and reconnects; new sessions append after a manual order. tmux order
  is a TShell preference and does not change the server's session names or IDs.
  A confirmed session refresh removes deleted sessions from saved order records;
  deleting the last session clears that host's session-order record. Connecting
  or disconnected hosts retain their saved order until a successful refresh.
  Removing a host clears its host-order and session-order records as well.
- Settings retain native navigation and search. Setting labels and controls
  wrap into separate lines when the content area cannot fit both. Groups are
  unframed sections capped at 680 pixels, with compact headings and 36-pixel
  minimum setting rows. This keeps labels and controls close in wide dialogs.
  Pages omit introductory descriptions. Non-obvious options use an adjacent
  information icon with localized help; language and font selection have no
  explanatory subtitle. Shortcut and status-item lists use compact 36-pixel
  minimum rows with 4-pixel gaps.
- The command palette (`Ctrl+Shift+K`) uses one searchable list with Hosts,
  Sessions, and Actions headings. Hosts and sessions appear before commands for
  quick navigation. Actions contains all commands, including Add/Edit/Remove Host.
  Hosts and sessions use checkmarks
  for selection, without a context summary, “current” suffix, or tutorial footer.
  All hosts appear regardless of shortcut availability. Sessions contains only
  the active host's live sessions in sidebar order, labeled by session name.
  Commands show their configured shortcut. Names, addresses, directories, and
  command identifiers remain searchable, with native arrow/Enter navigation and
  a localized empty result. Confirmation uses the original target even
  after filtering. The list body uses up to 560 pixels, constrained by the viewport;
  filtered results can shrink instead of leaving an empty lower area. Reopening the palette resets
  its query and focus in the existing dialog.
  `Alt+1` through `Alt+9` select sessions on the active host in sidebar order;
  unavailable positions leave the selection unchanged. The shortcuts can be
  rebound in Settings. `Ctrl+Alt+1` through `Ctrl+Alt+9` continue to select hosts.
- Host forms separate connection and authentication fields. Invalid submissions
  attach the error to the corresponding field and focus that input. The footer
  remains outside the scroll area, including at the 860 x 520 minimum viewport.
  The local-host rename form uses a compact 140-pixel body.
  Add and edit use the same form and compact check/cancel icon controls. Session
  creation/rename and theme editing also use check icons for confirmation, with
  localized tooltips and accessible names. Authentication and host-key trust
  keep explicit text actions so their consequences remain clear.
- Explorer and Git use the same navigation row dimensions. Editor search uses
  icon commands; Git diff layout and context use segmented choices showing the
  current selection. Below 720 pixels of Git content width, the context choice
  becomes a dropdown and Open File uses an icon with a tooltip. Changing context
  resets the diff's vertical scroll.
- Loading, empty and request-failure feedback belongs to the owning content area.
  An empty Docker table shows its state below the persistent search and column
  headers. Refreshing existing tables, logs and editors uses a content overlay;
  feedback does not resize controls, list viewports or terminal grids. Long
  notices wrap and can scroll within the content bounds.
  Connection failures show their message and Reconnect action together in the
  center of the main content area, with navigation and terminal geometry intact.
  Git keeps the selected diff and toolbar while refreshing, and reloads the diff
  after a successful status request. A failed refresh retains the previous list.
  Explorer reports read, save and directory failures in its content area without
  moving its editor toolbar. With no document open, failures replace the empty
  prompt. Shortcut conflicts use a fixed space beside the recording control;
  window-size validation also reserves its feedback space.
  Theme-editor failures stay inside the editor,
  update actions precede their status text, and forwarding shows feedback
  inside its scroll area so validation cannot change the dialog height. Field validation remains
  attached to its input.
- Private-key file selection fills the host editor's path field whether the
  editor was opened from the sidebar or Settings. Closing the host editor
  discards pending picker results.
- Explorer refreshes keep the existing tree, expanded directories and selection
  until a successful replacement arrives. Git uses a persistent file-list scroll
  handle across requests. Its compact count area and session unread dots reserve
  their width so asynchronous updates cannot shorten adjacent names.
- New-session directory completion uses the available window height for a
  single-column scrollable result area, up to 420 pixels (14 directory rows).
  Validation takes 32 pixels from that area only while a message is present;
  an empty feedback area no longer limits the visible directories. The form
  controls and overall result-area height stay stable across listing and validation.
  Local and SSH completion no longer truncate matching directories to eight;
  remaining results are reachable by scrolling. SSH keeps its existing 4096-entry
  directory read limit.
  Enter and the confirm button both wait for remote directory validation when
  necessary, showing a busy button and ignoring duplicate submissions. Validation
  publishes before completion listing finishes; editing the form, closing it, or
  switching hosts cancels a pending submission. Failed validation keeps the form
  and reports the actual lookup error. The dialog's default Enter confirmation
  delegates to session submission instead of closing and clearing validation first.
  Theme-list errors use a fixed single-line slot with the full message in a
  tooltip. Docker configuration pages keep Save/Discard controls present and
  disabled when unavailable, and reserve the source-file row before loading.
- Enabled terminal metrics keep stable identities and status-bar height across
  pending, successful and failed requests. Each item follows its content width;
  separators have 4 pixels of padding on each side, with no reserved space after
  the text. Left and right groups retain their configured alignment. At narrow
  widths, the left group takes the remaining space and truncates the system name
  before numeric values; the right group stays at the trailing edge. Missing
  values use `--`; request errors and full details remain available in tooltips.
- Tab navigation in interface views emphasizes the focused control's existing
  border; mouse clicks restore the ordinary border. There is no outer focus ring,
  and terminal Tab input retains its terminal behavior. Focus never changes layout.
- Labels name the action or object directly. Repeated context descriptions and
  aggregate counts are omitted: pane totals, Git change totals, forwarding
  totals/aggregate bytes, and transfer task/pending/failed counts. Forwarding and
  collapsed transfers use icon buttons with tooltips. Per-task progress and
  errors, search match positions, and explicitly configured metrics remain
  functional information at their point of use.
- Status text is 12 pixels inside the existing 28-pixel status strip. SSH
  fingerprints use monospace text. Notices and update states pair an icon with
  wrapping text; failed update checks expose the retry action.
  Forwarding shows only its table and actions; its entry lives in the bottom
  status bar. Session directory selection and updates show operational data
  first, with usage help behind information icons. Authentication fingerprints,
  field errors, transfer failures, and destructive-action consequences remain
  visible at the point of use.
- Terminal search is a split-local overlay with a focused native input,
  match counter, option toggles, and icon navigation. Its controls wrap in narrow
  splits without changing terminal geometry. Git keeps virtual rows while using
  native text-selection gestures and raw-source copy. SFTP speed/remaining time
  uses the existing compact task rows; retry and clear are separate icon actions.

## Equal Splits

Terminal surfaces measure the current pixel bounds and font metrics before
building pane content in the same frame. Session and tab switches therefore
apply their final insets immediately, including newly opened terminals and
invalidated viewport caches. Whole-cell rounding leaves unused pixels on the
right and bottom. tmux keeps its server-confirmed pane geometry while a resize
is pending, with leading insets based on the measured viewport so asynchronous
row/column updates do not move the entire terminal surface.

The terminal has one tiling model: equal-width columns, with equal-height rows
inside each column. `Alt+N` appends a full-height column on the right and
redistributes columns. `Alt+Shift+N` inserts a row below the active pane and
redistributes that column. Closing a pane or a shell exiting redistributes its
remaining rows; removing the last row removes the column and redistributes the
remaining columns. Focus mode restores this same layout. Window resizing keeps
groups equal, with at most one pixel of local rounding or one tmux cell.

Pane-size shortcuts, draggable dividers, and alternate layout cycling are removed.
Existing shortcut overrides for removed commands are ignored. Separators are
opaque one-pixel strokes painted without pointer handlers. A restrained accent
marks the active pane's internal edges; hover and focus never change geometry.

Local and ordinary SSH geometry derives group proportions from membership,
without mutable split ratios. For tmux, TShell computes explicit pane widths,
heights and offsets in cells from the confirmed window size and column/row
membership, reserving one cell per divider. It submits the whole result as one
checksummed classic `select-layout` layout, with rounding remainders assigned
from left to right and top to bottom. This avoids `select-layout -E`: tmux 3.8
can exclude nested row groups when counting column siblings, producing unequal
widths and panes outside the window.

Each split/close first refreshes the server's surviving pane IDs, native pane
order and focus. Layout submission preserves pane identities and row groups;
if native pane order differs from geometric order, ordered swaps and focus
restoration complete the submission. Queued workspace actions wait for the
preceding server snapshot to settle.

Initial connection, observed tmux geometry changes, window resizes, and focus-mode
restoration also redistribute column layouts, including existing layouts whose
widths had drifted beyond the window while their row groups remained intact.
Control streams are opened from discovery before layout recovery can defer
the first visible snapshot, so attaching to an unequal saved layout cannot
wait for repair commands on a connection that has not yet been started.
The UI keeps the previous confirmed layout during recovery and publishes the
new server geometry after refresh; it does not predict pane sizes. Arbitrary
layouts made by external tmux clients remain server-owned; automatic recovery
applies when their geometry fits the column/row model, not every tmux topology.
Connection progress is shown as SSH discovery, control-channel startup, or
layout recovery in the empty terminal surface. If a layout command races with
an external change, TShell refreshes pane IDs and retries it twice. Very small
windows are left untouched and explain that there is not enough space for equal
panes. Repeated focus and pane-selection actions are collapsed while a snapshot
is in flight.
The classic layout format and native pane assignment are based on
[tmux 3.6 layout-custom.c](https://github.com/tmux/tmux/blob/3.6/layout-custom.c)
and [tmux 3.8 layout-custom.c](https://github.com/tmux/tmux/blob/3.8/layout-custom.c).

## Native Verification

Use `TSHELL_UI_CHECK_SCOPE=common-previews` to verify actual SVG image decoding,
offline Mermaid flowchart/sequence rendering, malformed-source errors, source
switching and hiding child WebViews. See [file previews](file-previews.md).

Set `TSHELL_UI_CHECK_SCOPE=html-preview` and `TSHELL_UI_CHECK_VISIBLE=1` to
exercise opening an isolated `index.html` through native frame events, waiting
for WebView2 initialization, and hiding it when returning to the terminal.
This check deliberately avoids synchronous manual draws: initialization must
allow queued application work to run without reentering borrowed GPUI state.
Run it with an isolated `TSHELL_DATA_DIR` and an installed WebView2 runtime.

`--workspace-ui-check` exercises focus, modal cleanup, host validation,
first-frame terminal bounds across session/tab switches (single and mixed splits,
with and without a cached viewport),
workspace actions, file editing, Git scrolling, settings persistence, and the
global transfer queue, split shortcuts, and close redistribution in an isolated
`TSHELL_DATA_DIR`. Feature checks exercise
Git selection/system clipboard copy across virtual rows, terminal search keys
and focus, link versus TUI mouse input, and notification pane targeting.
The same check compares Docker search, table and editor bounds across loaded,
loading and failure states, including empty results in both window sizes, and
verifies that workspace errors do not resize terminal panes.
It also checks directory/theme feedback sizes, status metric identities and height,
retained Explorer selection/scroll, Git list scrolling during failed refreshes, and Docker editor
bounds before document arrival and across clean/dirty states.

With `TSHELL_SSH_TEST_HOST` set, the check also covers immediate remote session
submission, duplicate Enter/confirm, missing directories, and cancellation after
editing or changing hosts. Set `TSHELL_UI_CHECK_SCOPE=session-submission` to run
only those cases, still using an isolated `TSHELL_DATA_DIR`.

To also capture native renders, build with `--features ui-check-screenshots`
and set `TSHELL_UI_SCREENSHOT_DIR` before running the check. Workspace captures
cover the command palette and filtered sessions, sidebar, every settings page, session creation, host errors, Git, and
editor search and mixed equal splits in dark mode at 1320 x 840 and light mode at
860 x 520. Screenshots
use fixture content and do not connect to a remote host.
Additional captures cover both Git character-highlight layouts and terminal
search in dark and narrow light windows.

The screenshot feature is diagnostic only. Normal builds do not enable GPUI's
test support or entity leak detector. Windows is the verified rendering target;
Linux and macOS appearance is not continuously checked.

The clipboard assertion is enabled by default. In an execution environment
where Windows denies even an independent `OpenClipboard` call, setting
`TSHELL_UI_CHECK_SKIP_CLIPBOARD` permits the remaining checks and records
`clipboard_checked: false` in the report. This does not verify OS clipboard copy.

The opt-in `real_tmux_grouped_splits_close_exit_resize_and_zoom_stay_equal` test
uses `TSHELL_SSH_TEST_HOST` and an isolated tmux socket. It checks repeated column
creation, attaching to existing unequal row/column groups, down once followed
by right twice, exact restoration of the original
rows after closing appended columns, repeated row-first mixed splits, close and
shell-exit recovery, rapid mixed splits, resize, and zoom. The regression has
been checked against isolated tmux 3.6 and 3.8 servers.
Local unit and native UI checks do not substitute for that remote test.
