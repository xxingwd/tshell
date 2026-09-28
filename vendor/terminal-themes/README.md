# Bundled terminal themes

This directory contains the checked-in palette source used by TShell.
The Apache-2.0 license for the source data is retained in `LICENSE`.

TShell imports all 17 palette definitions without changing colour values,
names, preview colours or the default (`one-dark`). These definitions are
embedded as the first-run `theme.json` template and can be edited with the
same format as user-created palettes.

Adaptations: TypeScript data becomes a JSON template, theme IDs are
serialized into workspace preferences, and GPUI renders a selector with colour
swatches. The terminal theme is independent of the interface's system/light/dark
setting. Unknown or missing stored IDs fall back to One Dark without discarding
other workspace settings.

The source snapshot remains in `tests/fixtures/theme-catalog.json` for
compatibility verification. The hidden workspace UI check selects and draws all
17 file-backed themes, checks saved preferences and verifies that changing
interface appearance preserves terminal colours.

This import contains palette data only; no separate interface runtime is bundled.
