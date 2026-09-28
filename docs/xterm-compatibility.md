# xterm.js compatibility baseline

TShell uses **xterm.js 6.0.0** as the reference for terminal behavior, with WebGL custom glyphs enabled, `drawBoldTextInBrightColors=true`, `minimumContrastRatio=1`, and Windows keyboard behavior. User-selected font, palette, opacity and workspace shortcuts remain application preferences. Rust/GPUI remains the runtime; no browser or JavaScript interpreter is embedded.

The built-in palette definitions in [`defaults.json`](../src/terminal_theme/defaults.json) retain their
checked-in colour values; One Dark remains the default dark slot. Four additional
Codex and VS Code Modern palettes are application extensions. Workspace surfaces
and dialog tokens derive from the selected terminal background, foreground and
cursor. Users configure independent light/dark scheme IDs; automatic mode chooses
between them using only Windows lightness, never its accent colours. Legacy
Codex/VS Code choices migrate to their matching pairs; legacy single schemes
retain the matching lightness slot. Codex's red, green and magenta ANSI entries
use the supplied semantic colours; other ANSI entries reuse the existing light
and One Dark tables. VS Code Modern's terminal foreground and workspace colours
follow the locally installed default theme; its ANSI colours use the existing
light and One Dark tables because those theme files do not specify them. Theme
changes update existing/new panes, selection/cursor colours and protocol reports
together. DIM still follows xterm's alpha-compositing rules. This is an
application preference, not a change to the xterm.js baseline.

TShell keeps the user's terminal font and, on Windows, explicitly uses Chromium's
simplified-Han fallback order: Noto Sans SC, Noto Sans CJK SC, Microsoft YaHei,
SimSun, then system fallback. Missing Chinese glyphs fall back through the
browser/platform font system. Terminal output and IME preedit share this
selection. Other platforms retain native system fallback. Reference: Chromium's
[kSimplifiedHanFonts](https://github.com/chromium/chromium/blob/main/third_party/blink/renderer/platform/fonts/win/font_fallback_win.cc),
checked 2026-09-17. This is a font preference, not a change to the pinned xterm
baseline or a claim of identical browser/native rasterization. A primary font
that already contains Chinese glyphs remains authoritative.

## Rules verified in this pass

Terminal line height uses the selected primary font's native ascent plus absolute
descent, multiplied by the saved `line_height_scale` (default 1.2, range 1.0–2.0).
Settings expose 0.1 steps independently of font size; older preferences default
to 1.2. Explicitly saved multipliers are preserved. Layout and rendering share the same calculation, and the font preview
uses it as well. In device pixels the formula is
`floor(ceil(natural_height * DPR) * line_height_scale) / DPR`, following the pinned
[WebGL renderer](https://github.com/xtermjs/xterm.js/blob/6.0.0/addons/addon-webgl/src/WebglRenderer.ts).
Native font metrics replace browser `fontBoundingBox` measurements, so this does
not claim pixel-identical browser metrics. The hidden-window check verifies
font-based heights and pointer rows at 1.0/1.2/2.0; settings checks verify saving
and propagation. The former fixed `ceil(font_size * 1.5)` behavior is removed.

Selection drag scrolling follows the pinned [SelectionService](https://github.com/xtermjs/xterm.js/blob/6.0.0/src/browser/services/SelectionService.ts): a 50 ms interval, a 50 px distance threshold, and up to 15 rows per tick. `tools/generate-selection-scroll.mjs` executes the upstream distance-to-speed method and records its source hash in `tests/fixtures/xterm-selection-scroll.json`. Rust tests compare those cases and verify selection endpoints after scrolling. Scroll and selection extension execute atomically against the current terminal grid; rectangular selections retain their column. Returning inside, releasing the mouse, losing focus or hiding the pane stops the timer. Slow interaction workers apply backpressure rather than accumulating timer commands. This verifies speed mapping and grid updates, not every native pointer-capture or selection-mode interaction.

Mouse release preserves the last move/scroll endpoint, following the pinned
`SelectionService._handleMouseUp`; it does not re-hit-test the release coordinates.
The hidden-window render check covers scroll/release/copy in both directions,
ordinary and rectangular selections, and current/stale view snapshots. This
prevents an outside release from truncating a selected row or reverting to an
older viewport. The check calls the view's mouseup handler; native OS pointer
capture outside the app still requires manual validation.

| Behavior | TShell implementation and evidence |
| --- | --- |
| Bold ANSI colors | Displayed foreground palette indices 0–7 select indices 8–15. Default foreground, RGB and background are not brightened; inverse swaps colors before foreground selection. |
| Faint (SGR 2) | Preserve foreground RGB and use 50% ink alpha. Background, desktop transparency and selection can compose naturally. No fixed RGB multiplication or pre-blended background. |
| Decoration colors | Default underline/strike inherit faint ink; explicit SGR 58 underline colors remain opaque and indexed colors follow bold brightening. |
| Underline variants | Single, double, curly, dotted and dashed SGR 4 variants are distinct cell geometry. Strikethrough is drawn over text. Decorated custom glyphs retain cell-sized geometry. |
| Custom glyphs | Box borders, blocks, quadrants, shade, arcs and diagonals use cell graphics. Faint rectangular junctions are partitioned before painting to avoid multiple alpha layers at intersections. |
| Modified keys | Arrows, Home/End, F1–F12, Delete, Insert, PageUp/PageDown, Tab, Enter, Escape and Backspace follow the reference combinations. Shift+Page keys scroll the viewport. |
| Cursor | The default is a standard full-cell blinking block with xterm's 600 ms WebGL blink interval. DECSCUSR shape/blink requests remain authoritative; an unfocused TShell pane deliberately hides its cursor instead of drawing xterm's inactive outline. |
| Paste | LF and CRLF become CR, with or without bracketed paste. Bracket markers wrap the original content; ESC bytes are preserved as in xterm.js. Existing input-size limits remain. |

Primary reference files:

- [WebGL TextureAtlas](https://github.com/xtermjs/xterm.js/blob/6.0.0/addons/addon-webgl/src/TextureAtlas.ts): foreground/background resolution, decorations, drawing order.
- [WebGL cursor blink manager](https://github.com/xtermjs/xterm.js/blob/6.0.0/addons/addon-webgl/src/CursorBlinkStateManager.ts) and [renderer](https://github.com/xtermjs/xterm.js/blob/6.0.0/addons/addon-webgl/src/WebglRenderer.ts): 600 ms blink cadence, focus lifecycle and cursor redraw behavior.
- [WebGL constants](https://github.com/xtermjs/xterm.js/blob/6.0.0/addons/addon-webgl/src/Constants.ts): faint opacity.
- [CustomGlyphs](https://github.com/xtermjs/xterm.js/blob/6.0.0/addons/addon-webgl/src/CustomGlyphs.ts): cell-sized graphic behavior.
- [Keyboard](https://github.com/xtermjs/xterm.js/blob/6.0.0/src/common/input/Keyboard.ts) and [Clipboard](https://github.com/xtermjs/xterm.js/blob/6.0.0/src/browser/Clipboard.ts): terminal input bytes.

## Repeatable comparison

`tools/generate-xterm-fixtures.mjs` executes the pinned upstream keyboard/paste functions and the original WebGL foreground resolver. Its minimum-contrast helper is disabled to match the default option. It writes source URLs and SHA-256 hashes together with **416 keyboard, 10 paste and 144 color cases** to `tests/fixtures/xterm-6.json`. Keyboard cases cover every Shift/Alt/Ctrl combination with application cursor mode both on and off. Colors cover both background themes, default/16/256/RGB foreground modes, red/blue, bold, faint and inverse.

Regenerate with Node 24+ and network access:

```powershell
node tools/generate-xterm-fixtures.mjs
cargo test --locked xterm_compat
```

Normal Rust tests use the checked-in fixture and need neither Node nor network. Additional tests cover custom-glyph junction coverage, SGR decoration variants, explicit underline color, reverse-video backgrounds and ash-style faint output. The hidden-window render check draws these styles with the real GPUI text/drawing path and checks row-cache reuse.

Windows composition has a separate [backend patch and GPU readback test](../vendor/gpui-pre-windows/TSHELL_PATCH.md). Both ordinary and premultiplied path sprites use source-over alpha; 80 D3D11 WARP cases validate actual pixels on translucent light/dark targets. These tests cover the blend states, not complete browser/native glyph pixel equivalence. A parser-to-render-plan test also compares shell SGR 34 with ash's SGR 38;5;4, including DIM, bold, resets, themes and OSC overrides. `tools/terminal-colors.ps1` prints these combinations for manual comparison in different terminals.

## Extensions and remaining gaps

### Input cursor and symbol overhang

The input block/underline cursor now takes its width from the current cell's
wide-character flag; beam width is unchanged. Regression coverage moves the
cursor horizontally and vertically across Chinese, ASCII and empty cells.
This follows the pinned [WebglRenderer](https://github.com/xtermjs/xterm.js/blob/6.0.0/addons/addon-webgl/src/WebglRenderer.ts)
and [RectangleRenderer](https://github.com/xtermjs/xterm.js/blob/6.0.0/addons/addon-webgl/src/RectangleRenderer.ts).

Ordinary text ink is clipped to the row and pane, rather than each text run.
Like [GlyphRenderer](https://github.com/xtermjs/xterm.js/blob/6.0.0/addons/addon-webgl/src/GlyphRenderer.ts)
with `rescaleOverlappingGlyphs=false`, a fallback glyph may overhang its cell;
subsequent text remains anchored to its original columns. Tests cover `⏸`,
VS15/VS16 forms and mixed-width text, and the native hidden-window check draws
these symbols. This does not implement horizontal rescaling, complete grapheme
clustering or xterm's background-sensitive left-overhang clipping. Vertical row
clipping remains unchanged. Selection and scrollback behavior are unchanged.

This baseline is a compatibility direction, not a claim that the complete xterm.js implementation has been ported. Keep future fixes anchored to source behavior and add independent cases.

- Theme reports CSI 996/2031 and native tmux control integration are existing extensions. tmux still owns query replies and pane state; its version affects capabilities.
- OSC 0/2 titles are workspace metadata: pure title output updates the latest title and notifies the workspace through a bounded metadata channel, without advancing the screen revision or waking the terminal renderer. Mixed output and other OSC/CSI/ESC sequences still use normal visual invalidation. This is a scheduling optimization, not a change to the title bytes accepted by the terminal parser.
- The Alacritty parser/grid remains the engine. Every VT sequence, DA/DECRQM reply and Unicode version has not yet been differentially tested against xterm. Differences such as overline/blink attributes require separate parser/state support.
- Font rasterization and decoration baseline metrics use the native text system. Pixel-identical browser output, full custom powerline/Braille/legacy-computing glyph coverage and overlapping-glyph rescaling are not yet established.
- Selection ink/contrast rules, IME edge cases, platform-specific keyboard layouts, mouse protocol variants and transparency combinations require further reference cases. The current fixture validates the listed keyboard bytes and base WebGL color resolver, not every interactive browser path.
- Workspace shortcuts take priority; TShell identifies itself as TShell, and does not advertise capabilities solely to imitate xterm's identity.
- Cursor hiding on blur is an explicit workspace preference. The focused block shape, blink timing and DECSCUSR shape/blink behavior follow the pinned xterm terminal contract.

Upgrading xterm requires regenerating and reviewing fixtures and documenting changed defaults. Do not silently change the baseline to upstream main.

## Native terminal notifications

OSC `9;message` and `777;notify;title;body` are deliberate desktop extensions,
not xterm.js core behavior. Sources: [iTerm2 OSC 9](https://iterm2.com/documentation-escape-codes.html)
and [WezTerm's OSC 777 reference](https://wezterm.org/escape-sequences.html).
The checked-in OSC 9 handler is also a workspace reference.
BEL and ST terminators and fragmented UTF-8 output are supported. OSC `9;4`
progress messages, empty messages, and CAN/SUB-cancelled sequences do not notify.

Local and plain SSH sessions observe live PTY output. tmux observes each pane's
ordered `%output` stream, including background panes and output during capture
restoration; synthetic captures never generate notifications. A bounded queue
of 32 requests feeds one native worker without blocking terminal reads; overflow
is dropped. Titles over 1024 bytes and bodies over 8192 bytes are discarded.
The VTE parser's 16-parameter OSC limit still applies to semicolon-heavy payloads.

`notify-rust` 4.18.0 delivers Windows toast, macOS native, and Linux D-Bus
notifications. Desktop permissions, notification settings and an available
notification service remain required. Unpackaged Windows builds use the library's
PowerShell application identity (the notification title still identifies TShell);
a branded installer identity and click-to-focus behavior are not implemented.
OSC 99, actions, notification lifecycle replies, and tmux DCS passthrough are not
implemented. Notifications are delivered whether the pane is focused or not.

Run `cargo test --locked terminal_notifications` for parser regression cases.
Run `cargo test --locked notifications_native_smoke -- --ignored` in a desktop
session to submit a real native notification. This checks API acceptance, not
whether OS focus-assist settings permit a visible banner. macOS/Linux delivery
requires validation on those platforms.

PowerShell manual check inside TShell:

```powershell
[Console]::Write("$([char]27)]9;Task finished$([char]7)")
[Console]::Write("$([char]27)]777;notify;Build;All tests passed$([char]7)")
```

## Event-driven workspace metadata

The workspace subscribes to title/exit and tmux snapshot notifications instead of polling every 33 ms. Notifications have capacity one and a 4 ms coalescing window; parsing still consumes every byte in order. tmux titles are decoded from the live `%output` stream using the same Rust ANSI processor, independently of synthetic capture restoration. Query responses cannot overwrite titles received after a query began. External tmux title and path changes use quoted format subscriptions (tmux limits these to once per second). Subscription values update the matching pane directly; version checks keep older in-flight snapshots from replacing newer metadata. These changes do not alter background output parsing, OSC handling or screen restoration. Layout notifications request a snapshot with a 16 ms coalescing interval. Healthy sessions do not periodically query session/window/pane lists; failed discovery or a disconnected control stream retries after 10 seconds. Format subscriptions still use tmux's own one-second change checks. Without a live control stream, external changes cannot notify the client; reconnect to discover externally created sessions. This is native workspace scheduling, not a claim of new xterm protocol parity. Sources: [tmux 3.6 control subscriptions](https://github.com/tmux/tmux/blob/3.6/tmux.1) and [OSC title handling](https://github.com/tmux/tmux/blob/3.6/input.c).

## OSC 52 clipboard writes

Local PTY, plain SSH and each tmux pane's ordered live output share the Rust
`terminal_clipboard` observer. A bounded 16-entry channel sends writes to GPUI's
system clipboard on the UI thread, including background panes. tmux capture
restoration and duplicate screen parsing never enqueue writes. Queue overflow
drops new writes; accepted UTF-8 text is limited to 1 MiB. This is an accepted
payload limit, not a new bound on the underlying VTE parser's OSC buffer.

The reference is the pinned [xterm.js 6.0.0 ClipboardAddon](https://github.com/xtermjs/xterm.js/blob/6.0.0/addons/addon-clipboard/src/ClipboardAddon.ts),
not a core OSC handler. `tools/generate-clipboard-fixtures.mjs` executes the addon
with js-base64 3.7.7 and records both source hashes in
`tests/fixtures/xterm-clipboard.json`. Thirteen cases cover UTF-8, empty writes,
invalid/noncanonical Base64 (clears the clipboard), and unsupported selections.
Rust tests feed both BEL/ST terminators at every byte split and additionally
check cancellation, size limits, independent tmux panes and synthetic restoration.

Only the system clipboard (`c`) is supported; an empty selector also chooses it
as a deliberate extension to the addon's browser provider. Primary selections,
multi-selection targets, OSC 52 reads (`?`) and tmux DCS passthrough are not
implemented. Reads retain TShell's existing copy-only policy; this differs from
the addon's optional clipboard-read support. These tests do not verify native
clipboard delivery on every OS or guarantee that a nested tmux/ConPTY forwards
the original bytes.

Run `cargo test --locked osc52`. Manual PowerShell check in a rebuilt TShell:

```powershell
[Console]::Write(([char]27).ToString() + ']52;c;aGVsbG8=' + [char]7)
```

Pasting into an editor should insert `hello`.

## Working directory metadata

OSC 7 `file://host/absolute/path` is a deliberate workspace extension, not xterm.js core behavior. The separate metadata parser accepts BEL/ST and fragmented UTF-8, decodes percent escapes, and ignores CAN/SUB-cancelled sequences and paths containing control characters. It does not alter rendered bytes or send replies. Windows PowerShell started by TShell reports this at each prompt; other plain shells need their own OSC 7 integration (otherwise the launch directory is the fallback). tmux uses its native `pane_current_path`, including when creating splits, rather than OSC metadata or locally persisted paths. Tests: `directory_metadata_handles_fragments_and_cancellation`.
