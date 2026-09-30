# SFTP transfers

Remote Explorer uploads and downloads use a workspace-wide in-memory queue. The queue
survives host, session, tab, and view changes while TShell remains open. Two
transfers may run concurrently; additional transfers wait in FIFO order.

The transfer panel appears in the lower-right corner when a task is added. It
shows the direction, host, file name, current file, bytes transferred, and a
progress bar once the total size is known. Directory transfers scan their
entries first, so the panel can briefly show `Scanning files`. The header can
be dragged to reposition the panel. Its dock button returns it to the lower-right
corner. The panel stays inside the workspace when the window is resized.

Moving the pointer away for four seconds collapses the panel to a small launcher
at the right edge, preserving its vertical position. Hovering or dragging keeps
it open. Manual expansion and newly queued transfers restart the same idle timer.
The launcher shows pending or failed task counts and restores the expanded
position when clicked. Each task has a separate cancel or clear button; long
file names, paths, and errors are available in tooltips.

Queued and running tasks can be cancelled. Cancellation is cooperative between
SFTP reads or writes; a running task is marked as cancelling until its current
request returns. Partial local downloads and remote uploads are removed when a
task fails or is cancelled. Finished tasks remain in the panel until cleared or
until the 32-task history limit requires evicting the oldest finished task.

When the destination already exists, a transfer writes to a temporary sibling
first and replaces the destination only after the new contents are complete.
This honors the operating system's overwrite confirmation for downloads and
keeps the previous file intact if a transfer is cancelled or fails.

The queue is intentionally process-local. It does not persist across launches,
does not resume interrupted transfers, and does not combine progress across
separate source files beyond the total byte count.

## UI verification

The debug `--workspace-ui-check` command exercises header dragging, launcher
expansion, individual cancellation/clearing, hover retention, and automatic
collapse in an isolated workspace. Build with `--features ui-check-screenshots`
and set `TSHELL_UI_SCREENSHOT_DIR` to capture native GPUI frames in dark and light
appearances, including an 860 x 520 window. As with all workspace UI checks,
`TSHELL_DATA_DIR` must point at an isolated test directory.
