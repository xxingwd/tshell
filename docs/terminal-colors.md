# Terminal colour control

Local PTYs and ordinary SSH terminals share one colour-control implementation.
Protocol overrides belong to each terminal, do not change saved themes or interface
colours, and are used both for query replies and rendering. Reset restores the
currently selected terminal theme. Queries observe operations in byte order,
including repeated set/query pairs inside a single OSC.

| Sequence | Support |
| --- | --- |
| OSC `4;index;?` / `4;index;color` | Query/set palette indices 0–255; multiple pairs accepted. |
| OSC `10`, `11`, `12` | Query/set foreground, background and cursor; stacked parameters advance through those three slots. |
| OSC `17`, `19` | Query/set selection background and foreground. Explicit selection ink is opaque; default selection ink retains existing cell colours. |
| OSC `104` | Reset specified palette indices, or the entire indexed palette with no parameters. |
| OSC `110`, `111`, `112`, `117`, `119` | Reset foreground, background, cursor, selection background and selection foreground. |
| CSI `?996n` | Query configured terminal-theme lightness; reply is CSI `?997;1n` (dark) or `?997;2n` (light). |
| CSI `?2031h` / `?2031l` | Enable/disable notifications. Any change to configured foreground, background, cursor, selection or ANSI colours emits a report, even if lightness stays the same. Application OSC writes do not emit notifications. |
| CSI `?2031$p` | Query subscription state with DECRQM. |
| DCS `+q` | XTGETTCAP for hexadecimal names `Co`/`colors` (256), and boolean `RGB`/`Tc`. Unknown names receive failure; other capabilities are not advertised. |
| OSC `21;key=value` | Kitty named colour control for indices 0–255 and `foreground`, `background`, `cursor`, `selection_background`, `selection_foreground`. |
| CSI `#P`, `#Q`, `#R` | Save, restore and report the xterm palette stack. Slots 1–10 address saved palettes; zero/omitted parameters push/pop. Report is CSI `?current;highest#Q`. |
| OSC `30001`, `30101` | Kitty aliases for palette push/pop. |

Traditional OSC colour values follow pinned xterm.js 6.0.0: `#RGB`, `#RRGGBB`,
`#RRRGGGBBB`, `#RRRRGGGGBBBB`, or `rgb:r/g/b` with equal channel widths of
1–4 hexadecimal digits. RGB fractions round to 8-bit channels; hash formats
truncate/shift as in the pinned parser. Names are unsupported in traditional OSC.
Replies use `rgb:rrrr/gggg/bbbb`; TShell preserves the query's BEL/ST terminator
as in xterm, while xterm.js always returns ST. Fragmented input and CAN/SUB
cancellation are covered by tests.

In Kitty OSC 21, `key=?` queries, `key=color` sets, and a bare key resets.
An empty selection value makes it dynamic: selection background follows displayed
cell foreground and selection foreground follows displayed cell background.
Queries return an empty value for dynamic/unset selection colours. Traditional
OSC 17/19 cannot express this state and report the theme background/foreground
fallback. Kitty control also accepts the basic CSS sixteen named colours and
`grey`, `cyan`, `magenta` aliases. Default foreground/background/cursor are
explicitly configured in TShell; empty assignments to these slots are ignored.
Unknown queried keys return `unknown=<unpadded Base64 key>`. Cursor-text, visual-bell,
opacity/wide-gamut fields and additional named colours are not implemented.
OSC 21 is a deliberate Kitty extension, not xterm's differently defined OSC 21.

The colour stack stores actual effective colours, so restoring a palette after a
theme switch restores the saved values rather than the new theme. Ten retained
slots bound its memory; overflow/underflow and invalid slots are ignored. A terminal
reset clears extra selection overrides, subscriptions and saved palettes.

tmux owns protocol replies for its control-mode panes. TShell observes colours
without injecting duplicate OSC, XTGETTCAP or stack replies. Synthetic capture
restoration preserves live subscriptions and extra colour state. tmux's version
and forwarding support determine the capabilities available to a TUI inside it;
DCS passthrough is not implemented here. The VTE parser's 16-parameter OSC limit
still applies; split large palette queries into smaller requests. XTGETTCAP
payloads are bounded to 4096 bytes and 64 replies per request. Neither transparent-window compositing nor
the desktop interface palette is exposed as a terminal colour.

## References and validation

- [Pinned xterm.js InputHandler](https://github.com/xtermjs/xterm.js/blob/6.0.0/src/common/InputHandler.ts),
  [colour parser](https://github.com/xtermjs/xterm.js/blob/6.0.0/src/common/input/XParseColor.ts),
  and [reply handler](https://github.com/xtermjs/xterm.js/blob/6.0.0/src/browser/CoreBrowserTerminal.ts).
- [xterm controls](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html).
- [Kitty colour control and stack](https://sw.kovidgoyal.net/kitty/color-stack/).
- [Contour palette notifications](https://contour-terminal.org/vt-extensions/color-palette-update-notifications/).

`tools/generate-color-protocol-fixtures.mjs` executes the pinned colour handlers,
parser and reply formatter, recording source hashes and 17 scenarios in
`tests/fixtures/xterm-color-protocol.json`. Tests compare both terminators at
every byte split and bytewise input. Extension tests cover selection rendering,
unknown/dynamic fields, capability queries, bounds, resets, per-pane isolation,
palette restoration and same-lightness notifications. This is focused coverage,
not full xterm.js, xterm or Kitty parity.

```powershell
node tools/generate-color-protocol-fixtures.mjs
cargo test --locked color_protocol
cargo test --workspace --locked
```

An optional local source directory can be passed to the generator when the host
requires a system proxy. The native `--terminal-render-check` also checks explicit
selection/cursor colours, unchanged grid metrics and unrelated row-cache reuse.
