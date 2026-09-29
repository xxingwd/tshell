# Terminal rendering and performance checks

Workspace split dividers use an opaque one-pixel stroke and extend
through crossing gaps, including tmux's cell-sized gaps. Only existing internal
dividers adjacent to the active pane receive the accent color, with the same
stroke width and coordinates. No frame is drawn on workspace outer edges.
Normal and active colors blend 60% muted and 80% accent, respectively, over the
UI border color; the final strokes stay opaque to avoid darkened intersections.
Local, ordinary SSH and tmux reserve an eight-pixel minimum on both sides of
each padded axis. Any additional remainder stays on the trailing side: normal
mode starts at the top and puts extra height below, while focus mode starts at
eight pixels and keeps at least eight below; horizontally it starts at eight
pixels and puts extra width on the right. Tmux retains its native cell-sized
separator gaps inside the outer inset.
The terminal workspace wrapper adds no additional padding;
Explorer and Git diff use their own unchanged layout branches.
The highlight is paint-only and does not consume
pointer events. Divider hover and resize feedback remain available.

The renderer consumes immutable row snapshots. A view has at most one background
snapshot job in flight. PTY output notifications are bounded and drain already
pending signals without a debounce timer. Output immediately requests a background
snapshot for visible views rather than waiting for a paint to request it.
Hidden views continue parsing output but stop acquiring snapshots; at most an
already-running capture finishes. Showing a view requests the latest state.
If output arrives during capture, a visible view follows up to its latest revision.
Selection, scrolling,
copy and input are ordered through a mailbox capped at 256 pending operations and
2 MiB of pending input. A single input operation is capped at 1 MiB. Consecutive
selection moves and TUI mouse-motion reports replace their respective previous
pending move; copy, button, wheel, focus and keyboard input form ordering barriers.
A full mailbox reports an error instead of silently dropping input.

Interactions run in their own ordered background job, independently of snapshot
serialization. They request a snapshot after applying commands and sending input.
The grid lock is released during transport submission. Interaction processing can
still briefly contend with parsing/capture for the grid lock; it does not wait for
snapshot delivery to the UI. Transport queues remain bounded.
Row sharing and scene caching remain enabled to avoid increasing drawing work.

`Session` serializes snapshot capture and is the only consumer of Alacritty damage
tracking. Damaged rows are compared with the previous snapshot and replaced only
when their cells actually changed. Unchanged rows share `Arc<[Cell]>` storage.
Snapshots own their cells and remain valid after output, scrolling, resize or
alternate-screen changes. Grid mutation and output revision publication happen
under the same lock. Key and paste encoding read atomically published modes, so
they do not wait for rendering to catch up.

tmux capture restoration uses `capture-pane -e -N` to retain stored trailing
spaces and their SGR backgrounds. Unallocated trailing cells are still absent,
so TShell uses a default cell template only while processing synthetic line
feeds; otherwise a coloured or styled final cell could colour newly scrolled
blank rows. The captured SGR state continues across rows. A failed capture stays
pending and is retried on the next coalesced snapshot refresh. A resize that
supersedes an in-flight capture discards the old result and schedules a fresh
capture at the new size.
An existing pane with an uninterrupted `%output` stream does not recapture just
because it resized: replaying coloured cells cannot reconstruct tmux's current
SGR pen, and would interrupt an application's incremental redraw. New panes,
window switches and stream reconnections still restore their captured grid.

`TerminalView::prepare` and `paint` never acquire the terminal state lock. Each
visible row retains its shaped text and merged backgrounds. Each text row also
has an immutable GPUI entity rendered through the public `Entity::cached` API.
Unchanged rows replay scene primitives, avoiding repeated glyph positioning,
raster-bounds queries and atlas lookups. Backgrounds and selection are painted
below the text; cursor and IME remain above it. Selection-only frames reuse the
text scene and add at most one selection rectangle per visible row. Font
size, palette and OSC color changes invalidate the row styling. Cache storage is
bounded by terminal viewport rows; there is no ever-growing cache of previously
printed text. Rows without text allocate no text entity and do no text painting.
Row entities never read their parent, so parent notifications do not
invalidate every row. Bounds changes and GPUI refreshes invalidate the scene
cache; content, font and palette changes replace affected row identities.

Snapshot delivery compares visible state before notifying the view. Input or copy
that leaves the screen unchanged does not request another paint when its job
completes. Output notifications still wake the visible view to obtain fresh data.

Printable ASCII is batched with discretionary ligatures and kerning disabled.
Every cell origin is checked against the terminal grid in one forward pass,
including glyph count and byte indices. This avoids the quadratic cost of calling
the linear `x_for_index` lookup for every character. If shaping drifts, that run
falls back to fixed cell origins. Non-ASCII cells, wide characters and combining
sequences are isolated at their terminal column and clipped to their cell bounds.
Wide-character selection expands to cover the whole character. Half-cell hit
testing works in both drag directions; pending moves are merged in the mailbox,
and release/copy flush the final pending endpoint. Mouse moves outside the surface
are tracked while selecting. Wheel movement retains sub-line deltas. Applications
using DEC mouse tracking receive button, release, drag, any-motion and wheel reports
in SGR 1006, UTF-8 1005 or legacy X10 encoding. Mouse-motion reports leave scrollback and selection unchanged. Shift bypasses application mouse
tracking for local selection/scrollback. Focus in/out reports follow DEC mode 1004.

## Reproduce checks

```powershell
cargo test --locked --bin tshell
cargo test --locked --bin tshell render_snapshot_benchmark -- --ignored --nocapture
cargo build --locked --bin tshell
.\target\debug\tshell.exe --terminal-render-check C:\absolute\path\render-check.json
```

The last option exists in debug builds only. It opens an invisible GPUI window,
uses an in-memory terminal without a shell or network connection, checks the real
Windows text system and drawing path, writes a JSON report, and exits. It checks
200×60 mixed-content rendering, 120 selection frames with zero row copying and
zero row reshaping or text-row repainting, 1,000 coalesced pointer moves including
immediate release, and font sizes 10/14/24 with theme invalidation. It also compares
120 single-character updates on mostly empty and full screens with row scene
caching enabled/disabled in the same binary. It asserts the initial rows are
actually painted, and checks changed-row paint counts, not just timings. The
uncached control is only available to the debug diagnostic. It does not inject OS mouse or
keyboard input, read user sessions, or modify the clipboard.

On Windows, the check explicitly resizes the hidden window and waits for a valid
1700×1260 viewport before measuring. Every manual `Window::draw` clears its element
arena, as required by GPUI. The report includes viewport dimensions and the actual
initial text-row paint count to reject empty/clipped-away benchmarks.

Set `RUST_LOG=tshell::render=trace` when diagnosing a running app to record snapshot,
prepare and paint CPU timings, copied/shaped row counts, actual text-row paint
counts and text run counts (these are not GPU draw-call counts).
Trace logging is off by default. The GPUI overlay measures its own frame stages.

CPU benchmarks and hidden-window drawing checks do **not** measure display input
latency, GPU presentation, refresh-rate pacing, or establish a hardware-independent
FPS guarantee. Scene replay still copies/submits primitives and the GPU still
draws visible content; this is not an offscreen texture cache or damage-only GPU
presentation. Terminal image protocols and complete IME editing remain separate
compatibility work. The dev profile optimization level is unchanged.

## Validation on 2026-09-15

The updated Windows debug build passed 26 default tests (4 opt-in tests excluded)
and the strengthened GPUI drawing check. The v2 report is
`artifacts/terminal-render-check-v2.json`. It compares the same single-character
update in the same binary, with row scene caching enabled or disabled:

| Screen content | Scene cache | Text rows painted per update | CPU median | CPU P99 |
| --- | --- | ---: | ---: | ---: |
| Mostly empty | Disabled | 1 | 0.299 ms | 0.813 ms |
| Mostly empty | Enabled | 1 | 0.317 ms | 0.680 ms |
| Full 200×60 | Disabled | 60 | 12.473 ms | 13.175 ms |
| Full 200×60 | Enabled | 1 | 5.895 ms | 7.045 ms |

Full-screen CPU draw time fell about 53% in this controlled run. Selection-only
frames repainted zero text rows (5.843 ms median); scene replay and submission
still scale with visible content. These numbers are from one machine and exclude
GPU presentation. The uncached control isolates scene caching; both controls
already use the linear alignment check and current input handling.

**Correction to the first report:** `artifacts/terminal-render-check.json` did not
validate the hidden native viewport. GPUI on Windows can leave it invalid until
resized or shown, causing text to be clipped away. Its 0.896 ms / 1.360 ms draw
measurements must not be used as a performance baseline. The intermediate
0.14 ms result had the same problem. The old report is marked invalid for draw
timing; v2 checks real geometry, actual paint execution and cache invalidation.

If a running application locks `target/debug/tshell.exe`, use an isolated Cargo
target directory for validation and preserve existing terminal sessions. Rebuild
the default executable after the user closes the old app. The commands above use
the current executable name; historical timing reports do not establish a
performance baseline for the current build.
