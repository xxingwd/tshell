# Windows transparency patch

Upstream: crates.io `gpui-pre-windows` **0.3.4**, Zed snapshot
`69164008341295ad481bb11c0334a712ca8c23e3`. Original Apache-2.0 license is retained.
This local crate is selected through the root `[patch.crates-io]` and lockfile.

## Source-over alpha

The composition swap chain stores premultiplied RGBA. The ordinary sprite/quad
and premultiplied path-sprite blend states used `ONE` for destination alpha,
producing `As + Ad`, while RGB already used source-over. Correct alpha is
`As + Ad * (1 - As)`. Both destination-alpha factors now use
`D3D11_BLEND_INV_SRC_ALPHA`. Path rasterization already used this factor.
Opaque subpixel text keeps its separate, unchanged blending rules.

With a 20%-opaque white background and 25%-covered blue ink, the old pipeline
read back RGBA `[47, 61, 86, 115]`; source-over requires `[47, 61, 86, 102]`.
The excess alpha blocks too much desktop background, darkening translucent
glyphs and antialiased edges. This error is invisible on a fully opaque target.

`directx_renderer/blend_tests.rs` runs the production blend-state factories
on D3D11 WARP, renders into a texture, and reads actual RGBA pixels back.
It covers both blend states, light/dark backgrounds, five background alphas,
and four ink alphas: **80 cases**, with one-byte rounding tolerance.
The test failed on the original code and passes on the patch.

```powershell
cargo test --locked --offline -p gpui-pre-windows transparent_ink_matches_source_over_gpu_readback
```

The `PlatformWindow::render_to_image` implementation is gated by `test-support`
to match the dependency trait. This crate's own `cfg(test)` does not enable
`cfg(test)` in the GPUI dependency; the adjustment permits standalone backend
unit tests without enabling the entire application's test-support feature.

This does not make faint text independent of the desktop: xterm.js uses 50%
ink alpha for DIM, so its final color still legitimately depends on what is
behind it. It removes the additional darkening from incorrect composition.
It also does not replace native font antialiasing with browser rasterization.
