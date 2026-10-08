# Workspace Interface

TShell uses a compact native tool interface. GPUI Kit owns control behavior,
keyboard focus, menus, and selected states. Workspace styling uses the active
scheme's semantic interface tokens; terminal font metrics and protocol rendering
remain owned by the terminal view.

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
- Settings retain native navigation and search. Setting labels and controls
  wrap into separate lines when the content area cannot fit both. Groups are
  unframed sections, with compact headings and consistent row spacing.
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
  after filtering. The list height adapts to the viewport. Reopening the palette resets
  its query and focus in the existing dialog.
  `Alt+1` through `Alt+9` select sessions on the active host in sidebar order;
  unavailable positions leave the selection unchanged. The shortcuts can be
  rebound in Settings. `Ctrl+Alt+1` through `Ctrl+Alt+9` continue to select hosts.
- Host forms separate connection and authentication fields. Invalid submissions
  attach the error to the corresponding field and focus that input. The footer
  remains outside the scroll area, including at the 860 x 520 minimum viewport.
- Explorer and Git use the same navigation row dimensions. Editor search uses
  icon commands; Git diff layout and context use segmented choices showing the
  current selection. Changing context resets the diff's vertical scroll.
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
without mutable split ratios. tmux remains the owner of remote geometry. TShell
submits each split/close and its native sibling redistribution as one ordered
control sequence. Mixed groups use `select-layout -E` to spread row siblings,
then their column parent, preserving pane identity and row groups. This replaces
successive `resize-pane -x/-y` calls that could undo earlier adjustments.
Queued workspace actions wait for the preceding server snapshot to settle.

Observed tmux pane changes, window resizes, and focus-mode restoration also
redistribute column layouts, including layouts whose previous sizes had drifted.
The UI keeps the previous confirmed layout during recovery and publishes the
new server geometry after refresh; it does not predict pane sizes. Arbitrary
layouts made by external tmux clients remain server-owned; automatic recovery
applies when their geometry fits the column/row model, not every tmux topology.
Native redistribution behavior is based on [tmux 3.6a select-layout](https://github.com/tmux/tmux/blob/3.6a/cmd-select-layout.c)
and [sibling spreading](https://github.com/tmux/tmux/blob/3.6a/layout.c).

## Native Verification

`--workspace-ui-check` exercises focus, modal cleanup, host validation,
first-frame terminal bounds across session/tab switches (single and mixed splits,
with and without a cached viewport),
workspace actions, file editing, Git scrolling, settings persistence, and the
global transfer queue, split shortcuts, and close redistribution in an isolated
`TSHELL_DATA_DIR`. Feature checks exercise
Git selection/system clipboard copy across virtual rows, terminal search keys
and focus, link versus TUI mouse input, and notification pane targeting.

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
creation, close and shell-exit recovery, rapid mixed splits, resize, and zoom.
Local unit and native UI checks do not substitute for that remote test.
