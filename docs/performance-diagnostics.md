# Performance Diagnostics

`Ctrl+Shift+P` cycles GPUI's native frame overlay through hidden, compact,
and detailed modes. Its `CUR` value is the latest draw duration, while `1%`
and `10%` summarize the slowest 1% and 10% of the most recent 1000 draws.
GPUI paints the overlay directly into the scene without invalidating the
workspace view for each frame.

The terminal status bar retains left and right groups with space between them.
In Settings > Terminal Status Bar, drag an item to the upper group for the left
side or the lower group for the right side, and reorder within either group.
Resources default to the left, terminal geometry to the right; SSH RTT and tmux
RTT default to the right and are disabled. RTT readings belong
to the active remote host rather than GPUI's window-level frame stats. SSH RTT
is hidden for local sessions, and tmux RTT is hidden outside tmux sessions.
The frame overlay shortcut does not affect these status items.

SSH RTT uses the SSH protocol keepalive ping and does not write to a shell or
PTY. tmux RTT sends `display-message -p '#{version}'` through the existing
control stream. The latter includes SSH transport, tmux server processing, and
any commands already queued on that control stream; it is an application-level
round trip, not a raw TCP measurement.

Only enabled RTT probes run about every two seconds while the terminal status
bar is visible in a remote terminal workspace. They do not change terminal output.
Failed probes are shown as `--` and do not change session or tmux state.
