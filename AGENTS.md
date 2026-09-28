# TShell Project Guide

## Project Overview

TShell is a native Rust terminal workspace. GPUI Kit
renders the desktop interface, `alacritty_terminal` owns terminal state,
`portable-pty` starts local shells, and `russh`/`russh-sftp` provide shared SSH
sessions and remote file access. Windows is the primary supported platform;
Linux and macOS launch paths are retained but are not continuously verified.

The application deliberately separates terminal compatibility from workspace
preferences. xterm.js 6.0.0 is the reference for terminal bytes, colors, SGR,
input, selection, Unicode, and custom glyph behavior. TShell's workspace
shortcuts, host/session organization, themes, and window behavior are application
features and must not be changed merely to imitate a browser terminal.

## Repository Layout

Keep the repository shallow and put code beside the subsystem it owns:

- `src/main.rs`, `src/app_identity.rs`, `src/appearance.rs`, `src/i18n.rs`,
  `src/input.rs`, and `src/shortcuts.rs` contain application bootstrap and
  cross-cutting UI behavior.
- `src/backend.rs` and `src/backend/` contain workspace actions, snapshots,
  local/SSH lifecycle, and deterministic split-layout geometry.
- `src/terminal.rs`, `src/terminal/`, `src/terminal_view/`, and terminal
  rendering/protocol modules contain PTY input/output, parsing, snapshots,
  selection, glyphs, decorations, metrics, and notifications.
- `src/ssh_pool.rs` and `src/ssh_pool/` contain SSH connection pooling,
  authentication, interactive prompts, and tests.
- `src/tmux.rs`, `src/tmux_client.rs`, `src/tmux_client/`, and `src/tmux_titles.rs`
  contain the tmux control protocol and title/layout synchronization.
- `src/workspace.rs` and `src/workspace/` contain hosts, sessions, settings,
  Explorer, Git, SFTP, metrics, editor state, updates, and UI checks.
- `src/terminal_theme.rs` and `src/terminal_theme/` contain terminal palettes,
  custom theme persistence, and theme defaults.
- `src/update.rs` and `src/update/` contain Windows update discovery,
  verification, staging, and installation.
- `tests/` stores checked-in compatibility fixtures and integration assets.
- `docs/` records compatibility baselines, design decisions, release notes,
  and known gaps. `tools/` may generate or validate fixtures; tools are never
  part of the application runtime.
- `assets/`, `locales/`, `vendor/`, and `.github/` contain packaged resources,
  translations, pinned native dependencies, and CI workflows respectively.
- Build output belongs in `target/`; temporary or generated local material
  belongs in `tmp/` or `artifacts/`. Do not commit either unless a documented
  fixture explicitly requires it.

Use a new submodule only when a directory has a stable ownership boundary and
more than one cohesive file. Avoid wrapper modules that only re-export one
function or empty directory layers.

## UI and Layout Rules

- Keep the workspace hierarchy consistent: host -> session -> tab -> split.
  Local and ordinary SSH sessions are managed by TShell; tmux sessions remain
  owned by the remote tmux server.
- The collapsible sidebar is the navigation surface. The main pane owns the
  terminal, Explorer, or Git view and its status bar. Shared window controls
  and drag regions remain available even when the sidebar or title text is
  hidden.
- Compute split geometry from the available pixel rectangle first, then derive
  PTY rows and columns from each content rectangle. A splitter has a stable
  one-pixel visual line and a five-pixel interaction target. Never let labels,
  hover states, or dynamic content resize a grid cell.
- Keep interface appearance independent from terminal palette, ANSI output,
  opacity, and font metrics. Settings changes must be explicit, persisted, and
  immediately reflected in the owning view.
- Prefer dense, scan-friendly controls for repeated terminal work. Use icons
  for familiar window and editing commands, tooltips for unfamiliar icons, and
  stable spacing instead of decorative containers.
- Keep layout state in the owning layer: backend geometry must not perform UI
  drawing, and UI code must not mutate PTY/tmux state directly. Use typed
  actions, snapshots, and notifications at the boundary.

## Rust Code Style

- Prefer small, pure transformations with explicit inputs and outputs. Keep IO,
  UI mutation, process control, and shared state changes at boundary modules.
- Model state with enums and structs; use `Option` and `Result` instead of
  sentinel values. Make invalid states difficult to represent.
- Use ownership and borrowing deliberately. Prefer immutable data flow, scoped
  locks, and message passing over hidden global mutation.
- Use exhaustive `match` for protocol and UI state. Avoid `unwrap`/`expect` in
  runtime paths unless the invariant is local, documented, and impossible to
  recover from.
- Prefer iterator combinators when they make a conversion clearer; use a
  straightforward loop when it makes sequencing, ownership, or backpressure
  easier to audit.
- Keep modules focused and public APIs small. Extract an abstraction only when
  it removes stable duplication or clarifies ownership.
- Add comments only for non-obvious invariants, compatibility decisions, or
  scheduling constraints. Do not narrate self-evident code.
- Run `cargo fmt --all` and keep dependency changes locked and reproducible.

## Naming Conventions

- Use `snake_case` for modules, files, functions, methods, locals, and fields.
- Use `UpperCamelCase` for types, traits, and enum variants; use `SCREAMING_SNAKE_CASE`
  only for true constants.
- Name values after domain meaning, not implementation detail: `session_id`,
  `pane_snapshot`, `terminal_size`, and `workspace_action` are preferred over
  `data`, `item`, or `thing`.
- Use verb-first names for actions (`open_session`, `refresh_git_status`) and
  noun-first names for data (`SessionSnapshot`, `GitStatus`). Keep abbreviations
  conventional (`SSH`, `PTY`, `SFTP`, `TUI`, `OSC`, `CSI`, `DPR`, `UI`).
- Keep user-visible strings in `locales/`; do not introduce untranslated UI
  literals in Rust when a locale key is appropriate.
- Name tests after observable behavior, for example
  `faint_rgb_preserves_foreground_and_composites_alpha`.

## Terminal Compatibility

Read `docs/xterm-compatibility.md` before changing colors, SGR, glyphs, screen
state, terminal queries, input, paste, selection, or Unicode handling. Compare
observable output for identical terminal bytes and options; do not infer
behavior from labels such as "bold", "dim", or "canvas". Add a focused
regression fixture against the pinned xterm.js source before declaring a
compatibility issue fixed.

Record deliberate extensions and remaining gaps in the relevant document.
Never claim complete xterm.js parity from a limited test suite, and never move
the baseline silently to upstream main.

## Verification and Change Boundaries

- Use `cargo test --locked` for the normal suite. Run focused compatibility,
  SSH, tmux, or UI-check commands when touching those boundaries.
- Keep JavaScript and PowerShell tooling limited to fixture generation,
  packaging, or validation. They are not runtime dependencies.
- Preserve unrelated working-tree changes and running terminal sessions. Do not
  replace a locked executable or terminate the user's application.
- Update the nearest document when behavior, compatibility scope, or a known
  limitation changes. Prefer links from `README.md` over duplicating long
  implementation notes.
- Keep `README.md` as the English source document. Store translations in
  `README.<locale>.md` (for example, `README.zh-CN.md`) and put a language
  switcher at the top of every translated copy. Update every maintained
  translation when user-facing features or requirements change; technical names,
  commands, paths, and code identifiers must remain exact.
