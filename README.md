# TShell

[English](README.md) | [简体中文](README.zh-CN.md)

TShell is a native Rust terminal workspace for Windows.
It combines local shells, ordinary SSH connections, and remote tmux sessions
in one GPUI-based application. The terminal engine is Rust-native and uses
`alacritty_terminal`; xterm.js 6.0.0 is the compatibility reference for
terminal behavior.

The project is actively evolving. Windows is the primary verification target;
Linux and macOS startup paths are retained but are not continuously validated.
This README describes the current working tree, not a promise of complete
xterm.js or OpenSSH feature parity.

## Core Features

- Local PTY terminals and ordinary SSH terminals with shared connection reuse.
- Remote tmux control sessions with native server-side windows, panes, layouts,
  titles, and working directories.
- A host -> session -> tab -> split workspace model with collapsible navigation,
  keyboard-driven commands, equal column/row splits, floating/focus modes, and persisted
  interface preferences.
- Compact native controls, explicit sidebar hierarchy, responsive settings,
  field-level host validation, and segmented Git diff modes. See
  [workspace interface](docs/workspace-interface.md).
- Explorer and read-only Git views for local and remote working directories.
  Remote files use SFTP and remote Git commands; no remote Python or mount is
  required. Inline and side-by-side diffs support text selection/copy and
  character-level change highlighting. See [Git workspace](docs/git-workspace.md).
- A built-in editor for UTF-8 text files, background syntax highlighting,
  search, and conflict-aware remote saves.
- A global SFTP transfer queue with concurrent uploads/downloads, directory
  scanning, byte progress, a draggable panel that auto-collapses to the right
  edge, speed/remaining time, and per-task cancellation, clearing, and retry. See
  [SFTP transfers](docs/sftp-transfers.md).
- Markdown opens in a native rich-text preview; HTML uses a read-only WebView2
  preview on Windows. Both have a one-click switch back to editing. The HTML
  preview renders inline styles but blocks scripts and external resources.
- Unified workspace and terminal colors with VS Code Light/Dark Modern schemes.
  A single `theme.json` stores terminal colors and interface tokens per scheme;
  version 2 schemes migrate in place. Edit custom color schemes separately; assign the VS Code schemes to the light
  and dark slots, then select light, dark, or automatic mode. Automatic mode
  reads the Windows light/dark setting only.
  Form dialogs use one title and consistent spacing and surfaces that follow
  the active scheme; confirmation prompts remain compact. Fonts, line height,
  programming ligatures, opacity, acrylic background, and interface language
  are configurable. Appearance settings are grouped by theme, font, window,
  and background rather than by individual control.
- xterm-compatible keyboard, paste, selection, ANSI/256-color/true-color SGR,
  custom box glyphs, cursor styles, OSC 7 working-directory metadata, OSC 52
  clipboard writes, and native OSC 9/777 notifications.
- Search the active terminal's screen and retained history with match counts,
  case/regex options, and previous/next navigation. Ctrl-click OSC 8 links,
  URLs, or file paths; SSH paths open through their originating host. Windows
  notification clicks locate the originating split while TShell is running.
  See [terminal tools](docs/terminal-tools.md).
- Linux host metrics (CPU, memory, and root filesystem usage) over the existing
  SSH connection, with no sudo or Python requirement.
- Windows x64 packaging and an opt-in-background update path backed by release
  metadata checksums. See [Windows release notes](docs/windows-release.md).

## Requirements

For Windows development, install:

- Rust with the MSVC toolchain
- Visual Studio C++ Build Tools
- The system OpenSSH client for SSH configuration-based connections

The repository includes the pinned Windows GPUI patch and ConPTY runtime used
by the release build. A remote tmux host must have tmux installed. Remote
Explorer and Git require SFTP and Git on the target host respectively.

## Build and Run

```powershell
cargo run --locked --bin tshell
```

The debug executable is `target/debug/tshell.exe`. A local PowerShell starts by
default. Release builds can be created with:

```powershell
cargo build --locked --release --bin tshell
```

TShell stores host and interface preferences in the platform configuration
directory under `tshell/workspace.json`. Set `TSHELL_DATA_DIR` to use an
isolated directory for development or UI checks. Passwords are not written to
that file.

## Workspace Model

The sidebar starts at a host. Each host contains sessions, each session contains
tabs, and each tab contains one or more splits. Local and ordinary SSH sessions
persist their name and starting directory, then start fresh processes on the
next launch. tmux persists its own sessions, windows, panes, layouts, and
working directories on the remote server; TShell does not duplicate those
snapshots in local configuration.

The host selector keeps its original appearance. Its dropdown shows a
connection-status dot for each host and a disconnect button beside each
connected remote host. The button disconnects only that host without switching
the current host or removing its configuration; selecting that host again
reconnects it. Disconnecting ordinary SSH closes its terminals, while tmux
sessions remain on the server. Explorer and Git do not access a disconnected
host. Reconnecting restores its file drafts. A session's context menu offers
Delete Session; deleting a session or closing its last tab or pane is blocked
while its files have unsaved edits or a file operation is running.
Save open files before changing a host's connection or removing it. TShell
prevents those actions while the affected host has unsaved edits or a file
operation is running. Disconnecting the same host preserves its file drafts.
Changing connection details clears that host's file and tool
views so paths from the previous connection are not reused.

The command palette is available with `Ctrl+Shift+K`. Important defaults are:

| Shortcut | Action |
| --- | --- |
| `Ctrl+B` | Collapse or expand the sidebar |
| `Ctrl+1` / `Ctrl+2` / `Ctrl+3` | Terminal / Explorer / Git |
| `Ctrl+Shift+T` / `Ctrl+Shift+W` | New / close terminal tab |
| `Alt+Enter` | Focus the current split or restore the layout |
| `Alt+F` | Float the current tab or restore tiling |
| `Alt+N` | Add a column and equalize columns |
| `Alt+Shift+N` | Add a row and equalize the current column |
| `Alt+H/J/K/L` or `Alt` + arrows | Move focus |
| `Alt+Shift+H/J/K/L` or `Alt+Shift` + arrows | Move the split |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy / paste |
| `Ctrl+Shift+F` | Search the active terminal's output |
| `Ctrl+Shift+P` | Cycle GPUI's frame diagnostics |
| `Ctrl+-` / `Ctrl+=` | Decrease / increase terminal font size |

Splits use equal-width columns and equal-height rows within each column. Closing
a pane or exiting its shell redistributes the remaining group automatically.
There are no draggable dividers, pane-size shortcuts, or alternate tiling modes.
Local, ordinary SSH, and tmux use this same split model. See
[split layout behavior](docs/workspace-interface.md#equal-splits).

All actions are listed in Settings and may be rebound. The workspace shortcuts
are an application contract and are not replaced by xterm compatibility work.
Configure terminal status items, including optional SSH and tmux RTT, in
Settings. See [performance diagnostics](docs/performance-diagnostics.md) for
the RTT measurement details.

## SSH and tmux

SSH hosts can use an explicit address, user, port, and private key, or resolve
an existing system OpenSSH entry with `ssh -G`. SSH connection pools are kept
separate for terminal panes, tmux control, SFTP, metrics, and short commands so
one tool cannot consume another tool's channel budget.

tmux sessions use one dedicated control connection per session. Pane output,
titles, layouts, and external changes are synchronized through the tmux
control protocol. Closing TShell disconnects the client but leaves the remote
tmux session running. A host configured without tmux uses independent SSH PTY
channels for its panes.

Host-key verification follows the selected connection path. Direct hosts store
accepted fingerprints in the local `known_hosts`; existing OpenSSH entries use
the system configuration and its agent/proxy settings. TShell does not claim
full support for every OpenSSH option, user certificate, or host CA behavior.

## Explorer and Git

Explorer scans a bounded file tree and opens ordinary UTF-8 files up to 20 MiB.
It skips `.git`, `artifacts`, `node_modules`, and `target`. Remote reads and
writes use SFTP; a remote save compares the original contents, writes a sibling
temporary file, and atomically replaces the target when the server supports
`posix-rename@openssh.com`. External edits produce a conflict instead of being
silently overwritten.

Markdown (`.md`, `.mdx`) and HTML (`.html`, `.htm`) files show a read-only
preview when opened at up to 256 KiB; larger files stay in the source editor.
Markdown source renders directly through GPUI Kit's native `TextView::markdown`
on all platforms. On Windows, an embedded WebView2 displays HTML
layout and inline CSS. HTML preview pages cannot run scripts, load external
resources, submit forms, or navigate away. Relative assets are not loaded,
including for remote files. Other platforms and systems without WebView2 use
the text-oriented native HTML fallback. Use the Edit/Preview controls in the
file header to switch views; saving always writes the original UTF-8 source.

Git view reports `git status --porcelain` for the active tool directory. It is a
read-only browser: staging, commits, pushes, and history editing are outside
the application. Use the command line for those operations.
Both diff layouts allow dragging to select and copying with `Ctrl+C`, the
configured Copy shortcut, or the toolbar Copy button. Copied text excludes
line numbers and patch markers, preserving tabs and Unicode.

## Terminal Compatibility

The compatibility baseline and evidence are maintained in
[docs/xterm-compatibility.md](docs/xterm-compatibility.md). It documents the
pinned xterm.js source links, checked-in fixtures, rendering behavior, and
intentional extensions. Highlights include:

- ANSI 16-color, 256-color, true-color, bold brightening, inverse, faint alpha
  compositing, and multiple underline styles.
- Keyboard, paste, bracketed paste, mouse, focus, cursor, selection, IME, and
  Unicode handling based on the pinned reference cases.
- Cell-sized custom glyphs for box drawing, blocks, shade, arcs, and diagonals.
- OSC title and working-directory metadata, OSC 52 clipboard writes, and
  desktop OSC 9/777 notifications.
- Theme query and subscription extensions (`CSI ?996n`, `?2031h`, and related
  replies) used by TShell and tmux-aware applications.

The Alacritty parser/grid remains the terminal state engine. Browser-native
font rasterization, every VT sequence, every OpenSSH keyboard layout, and all
complex terminal recovery paths are not yet differentially verified. Read the
compatibility document before changing terminal behavior.

## Architecture

- `src/backend.rs` and `src/backend/` coordinate typed workspace actions,
  snapshots, lifecycle, and split geometry.
- `src/terminal.rs`, `src/terminal/`, and `src/terminal_view/` connect PTYs to
  terminal state, rendering, selection, glyphs, and protocol observers.
- `src/ssh_pool/` owns reusable SSH connections and interactive authentication.
- `src/tmux_client/` implements the tmux control stream and synchronization.
- `src/workspace/` owns hosts, sessions, settings, Explorer, Git, SFTP, metrics,
  editor state, and update UI.
- `src/terminal_theme/` owns built-in and custom terminal palettes.

See [AGENTS.md](AGENTS.md) for repository layout, UI boundaries, Rust style,
naming rules, and contribution constraints.

## Testing and Validation

Run the normal suite with:

```powershell
cargo test --locked
```

Focused checks include:

```powershell
cargo test --locked xterm_compat
cargo test --locked terminal_notifications
cargo test --locked osc52
```

Optional real-host coverage uses an existing passwordless SSH alias:

```powershell
$env:TSHELL_SSH_TEST_HOST = 'your-ssh-alias'
cargo test --locked -- --include-ignored
```

Compatibility fixtures can be regenerated with Node 24+ and network access via
`tools/generate-xterm-fixtures.mjs`; checked-in fixtures are sufficient for
normal Rust tests. The hidden-window UI check is available from a development
build with `tshell.exe --workspace-ui-check <report.json>`.

## Packaging and Updates

See [release notes](docs/release-notes.md) for changes in TShell 0.1.2.

Pushing a `v*` tag runs the Windows GitHub Actions workflow, which tests,
builds, packages, and publishes a same-repository release. The package contains
the executable, notices, checksums, and update metadata. See
[docs/windows-release.md](docs/windows-release.md) for the workflow and
recovery details.

Windows release builds check public release metadata over HTTPS, verify version,
architecture, size, and SHA-256, then stage the update for the next launch. The
update path does not sign binaries or force a restart, and it never stores GitHub
credentials.

## License

TShell is licensed under the Apache License, Version 2.0. Third-party notices
and vendored component licenses are included in the repository.
