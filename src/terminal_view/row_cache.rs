use super::*;

/// Text and cell graphics: backgrounds and selection are below these cached rows;
/// cursor and IME are painted above. The entity never reads its parent, so a
/// parent notification does not invalidate unrelated row caches.
pub(super) struct RowTextView {
    pub row: Arc<PaintedRow>,
    pub cell_width: f32,
    pub line_height: f32,
    pub font_size: f32,
    pub paints: Rc<std::cell::Cell<usize>>,
}

impl Render for RowTextView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let row = self.row.clone();
        let cell_width = self.cell_width;
        let line_height = self.line_height;
        let font_size = self.font_size;
        let width = row.source.len() as f32 * cell_width;
        let paints = self.paints.clone();
        canvas(
            |_, _, _| (),
            move |bounds, (), window, cx| {
                paints.set(paints.get() + 1);
                let paint_decoration =
                    |decoration: &crate::terminal_decorations::Decoration, window: &mut Window| {
                        crate::terminal_decorations::paint(
                            *decoration,
                            Bounds::new(
                                bounds.origin
                                    + point(px(decoration.col as f32 * cell_width), px(0.)),
                                size(px(decoration.columns as f32 * cell_width), px(line_height)),
                            ),
                            cell_width,
                            font_size,
                            window,
                        );
                    };
                // Preserve layering: underline below glyphs/text, strikethrough above.
                for decoration in row.decorations.iter().filter(|d| d.kind.is_some()) {
                    paint_decoration(decoration, window);
                }
                for glyph in &row.glyphs {
                    crate::terminal_glyphs::paint(
                        *glyph,
                        Bounds::new(
                            bounds.origin + point(px(glyph.col as f32 * cell_width), px(0.)),
                            size(px(cell_width), px(line_height)),
                        ),
                        window,
                    );
                }
                for text in &row.text {
                    let origin = bounds.origin + point(px(text.col as f32 * cell_width), px(0.));
                    // Like xterm's default WebGL glyph renderer, allow ink to
                    // overhang its cell without moving the next cell's origin.
                    // Keep row/viewport clipping, not a clip per text run.
                    let mask = ContentMask { bounds };
                    window.with_content_mask(Some(mask), |window| {
                        let _ = text.shaped.paint(
                            origin,
                            px(line_height),
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                    });
                }
                for decoration in row.decorations.iter().filter(|d| d.kind.is_none()) {
                    paint_decoration(decoration, window);
                }
            },
        )
        .w(px(width))
        .h(px(line_height))
    }
}

pub(super) fn ascii_grid_aligned(line: &ShapedLine, columns: usize, cell_width: f32) -> bool {
    ascii_positions_aligned(
        line.runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .map(|glyph| (glyph.index, f32::from(glyph.position.x))),
        columns,
        cell_width,
    )
}
// A ligature may cover several source cells. Its cluster must start at the
// original cell boundary and the entire run must retain its terminal width.
pub(super) fn ligature_grid_aligned(line: &ShapedLine, columns: usize, cell_width: f32) -> bool {
    if (f32::from(line.width) - columns as f32 * cell_width).abs() > 0.1 {
        return false;
    }
    let mut previous = None;
    for glyph in line.runs.iter().flat_map(|run| &run.glyphs) {
        if glyph.index >= columns || previous.is_some_and(|i| glyph.index < i) {
            return false;
        }
        if previous != Some(glyph.index) {
            if previous.is_none() && glyph.index != 0 {
                return false;
            }
            if (f32::from(glyph.position.x) - glyph.index as f32 * cell_width).abs() > 0.1 {
                return false;
            }
        }
        previous = Some(glyph.index);
    }
    previous.is_some()
}

// A single forward pass. Calling x_for_index for each cell would repeatedly
// scan the same run and make validation quadratic in line length.
fn ascii_positions_aligned(
    mut glyphs: impl Iterator<Item = (usize, f32)>,
    columns: usize,
    cell_width: f32,
) -> bool {
    for col in 0..columns {
        let Some((index, x)) = glyphs.next() else {
            return false;
        };
        if index != col || (x - col as f32 * cell_width).abs() >= 0.05 || !x.is_finite() {
            return false;
        }
    }
    glyphs.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::ascii_positions_aligned;

    #[test]
    fn grid_validation_rejects_ligatures_drift_and_extra_glyphs() {
        assert!(ascii_positions_aligned(
            [(0, 0.), (1, 8.), (2, 16.)].into_iter(),
            3,
            8.
        ));
        assert!(!ascii_positions_aligned(
            [(0, 0.), (2, 16.)].into_iter(),
            3,
            8.
        ));
        assert!(!ascii_positions_aligned(
            [(0, 0.), (1, 8.1)].into_iter(),
            2,
            8.
        ));
        assert!(!ascii_positions_aligned(
            [(0, 0.), (0, 0.)].into_iter(),
            1,
            8.
        ));
        let mut visits = 0;
        assert!(ascii_positions_aligned(
            (0..10_000).map(|i| {
                visits += 1;
                (i, i as f32 * 8.)
            }),
            10_000,
            8.
        ));
        assert_eq!(visits, 10_000);
    }
}
