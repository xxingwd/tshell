# Terminal colour control

Local PTYs and ordinary SSH terminals share a small colour-control implementation.
Per-terminal overrides affect query replies and rendering without changing saved
themes or interface colours. Reset restores the currently selected terminal theme.
Queries observe operations in byte order, including repeated set/query pairs inside
a single OSC.

| Sequence | Support |
| --- | --- |
| OSC `4;index;?` / `4;index;color` | Query/set palette indices 0–255; multiple pairs accepted. |
| OSC `10`, `11`, `12` | Query/set foreground, background and cursor; stacked parameters advance through those three slots. |
| OSC `104` | Reset specified palette indices, or all indexed colours with no parameters. |
| OSC `110`, `111`, `112` | Reset foreground, background and cursor. |
| CSI `?996n` | Query configured terminal-theme lightness; reply is CSI `?997;1n` (dark) or `?997;2n` (light). |
| CSI `?2031h` / `?2031l` | Enable/disable notifications. Changes to configured foreground, background, cursor or ANSI colours emit a report, even if lightness stays the same. Application OSC writes do not emit notifications. |
| CSI `?2031$p` | Query subscription state with DECRQM. |

OSC colour values follow pinned xterm.js 6.0.0: `#RGB`, `#RRGGBB`,
`#RRRGGGBBB`, `#RRRRGGGGBBBB`, or `rgb:r/g/b` with equal channel widths of
1–4 hexadecimal digits. RGB fractions round to 8-bit channels; hash formats
truncate/shift as in the pinned parser. Colour names are unsupported.
Replies use `rgb:rrrr/gggg/bbbb`; TShell preserves the query's BEL/ST terminator
as in xterm, while xterm.js always returns ST. Fragmented input and CAN/SUB
cancellation are covered by tests.

tmux owns protocol replies for its control-mode panes. TShell observes colours
without injecting duplicate replies. Synthetic capture restoration preserves live
subscriptions; a real terminal reset disables them. tmux's version and forwarding
support determine the capabilities available to a TUI inside it. The VTE parser's
16-parameter OSC limit still applies; split large palette queries into smaller
requests. Window compositing and the desktop interface palette are not exposed
as terminal colours.

The implementation intentionally stays limited to the common OSC controls above
and the selected theme-reporting extension. Selection colour controls, Kitty OSC 21,
palette stacks and XTGETTCAP are not implemented.

## References and validation

- [Pinned xterm.js InputHandler](https://github.com/xtermjs/xterm.js/blob/6.0.0/src/common/InputHandler.ts),
  [colour parser](https://github.com/xtermjs/xterm.js/blob/6.0.0/src/common/input/XParseColor.ts),
  and [reply handler](https://github.com/xtermjs/xterm.js/blob/6.0.0/src/browser/CoreBrowserTerminal.ts).
- [xterm controls](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html).
- [Contour palette notifications](https://contour-terminal.org/vt-extensions/color-palette-update-notifications/).

`tools/generate-color-protocol-fixtures.mjs` executes the pinned colour handlers,
parser and reply formatter, recording source hashes and 17 scenarios in
`tests/fixtures/xterm-color-protocol.json`. Tests compare both terminators at
every byte split and bytewise input. Additional tests cover cursor rendering,
cancellation, per-pane isolation, tmux reply ownership and same-lightness
notifications. This is focused coverage, not complete xterm.js or xterm parity.

```powershell
node tools/generate-color-protocol-fixtures.mjs
cargo test --locked color_protocol
cargo test --workspace --locked
```

An optional local source directory can be passed to the generator when the host
requires a system proxy. The native `--terminal-render-check` also validates
cursor overrides and resets through the GPUI draw path with unchanged grid metrics.
