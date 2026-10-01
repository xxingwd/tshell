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
- Host forms separate connection and authentication fields. Invalid submissions
  attach the error to the corresponding field and focus that input. The footer
  remains outside the scroll area, including at the 860 x 520 minimum viewport.
- Explorer and Git use the same navigation row dimensions. Editor search uses
  icon commands; Git diff layout and context use segmented choices showing the
  current selection. Changing context resets the diff's vertical scroll.
- Status text is 12 pixels inside the existing 28-pixel status strip. SSH
  fingerprints use monospace text. Notices and update states pair an icon with
  wrapping text; failed update checks expose the retry action.
- Terminal search is a split-local overlay with a focused native input,
  match counter, option toggles, and icon navigation. Its controls wrap in narrow
  splits without changing terminal geometry. Git keeps virtual rows while using
  native text-selection gestures and raw-source copy. SFTP speed/remaining time
  uses the existing compact task rows; retry and clear are separate icon actions.

## Native Verification

`--workspace-ui-check` exercises focus, modal cleanup, host validation,
workspace actions, file editing, Git scrolling, settings persistence, and the
global transfer queue in an isolated `TSHELL_DATA_DIR`. Feature checks exercise
Git selection/system clipboard copy across virtual rows, terminal search keys
and focus, link versus TUI mouse input, and notification pane targeting.

To also capture native renders, build with `--features ui-check-screenshots`
and set `TSHELL_UI_SCREENSHOT_DIR` before running the check. Workspace captures
cover the sidebar, appearance settings, hosts, metrics, host errors, Git, and
editor search in dark mode at 1320 x 840 and light mode at 860 x 520. Screenshots
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
