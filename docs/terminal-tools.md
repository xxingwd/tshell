# Terminal Tools

## Output Search

`Ctrl+Shift+F` opens a compact search overlay in the active split. Search covers
that terminal's visible screen and retained grid history, including soft-wrapped
lines and Unicode. It cannot recover history already discarded by the terminal
or unavailable from tmux capture. Results are bounded to 10,000 matches; the
counter shows `+` when more may exist.

Literal search is the default. Case and regular-expression options are explicit.
The input accepts Enter/Shift+Enter for next/previous; arrow buttons provide the
same navigation and wrap around. Escape closes the overlay and restores terminal
focus. Invalid expressions produce an error state instead of changing output.
Opening search immediately focuses its input, including when invoked from
Explorer or Git. Workspace focus restoration targets the search input while the
overlay is open. Ordinary input events reach the native input handler; the
terminal only encodes key events when its own surface has focus.
Switching to Explorer or Git also restores focus to a visible workspace target
so the search shortcut remains reachable after a view change.

Search runs in a coalesced background job. Query generations reject stale
results. New output and terminal reflow refresh matches and invalidate a stale
current position; passive refresh never jumps the viewport. Explicit navigation
scrolls the match into view. The overlay does not resize the PTY.

## Links and Paths

Hold Ctrl while hovering to reveal a link cursor/underline; Ctrl-click opens an
OSC 8 link or a detected HTTP/HTTPS URL. Only HTTP/HTTPS URLs are passed to the
system browser. File links and plain paths open Explorer or the built-in editor;
`path:line:column` uses one-based source coordinates. URL-encoded file paths are
decoded. Other OSC 8 schemes are ignored.

Paths are resolved by the terminal that emitted them, using its working
directory. Local paths stay local. SSH/tmux paths use that host's SFTP connection,
including paths with `~/`; OSC file URL host names cannot redirect them to
another host. Directories open Explorer. Files retain the editor's unsaved-change
prompt. Stale terminal identities and file requests are rejected.

Ordinary clicks continue to follow selection/TUI mouse behavior. Ctrl-click is
a workspace gesture and is consumed before TUI mouse reporting, including when
there is no link under the pointer. Plain text detection is intentionally
conservative; paths containing spaces are best emitted as OSC 8 file links.

## Notification Activation

On Windows, clicking an OSC 9/777 notification while TShell is running activates
the application and selects its originating host/session/tab/split. A closed
terminal produces a notice. Unique process-local identities prevent pane ID
reuse from selecting a different terminal. Activation after application exit
and macOS/Linux click navigation are not implemented.

## Verification

Unit coverage includes retained/soft-wrapped Unicode output, case and regex
handling, navigation and new output, terminal resize, OSC 8/plain link parsing,
and local/SSH path routing. The isolated `--workspace-ui-check` dispatches native
search input keys and Ctrl/ordinary TUI mouse clicks, verifies focus and scrolling,
and captures dark/narrow-light search layouts with match-background pixel checks.
It also opens search through the real workspace shortcut from Terminal, Explorer
and Git, types characters through the native input route, and checks that search
typing sends no bytes to the terminal.
It also checks file link line/column placement, same-file draft preservation,
directory links, notification target selection and closed-terminal handling.
Real OS toast activation and remote
server behavior still require a desktop/SSH integration run.
