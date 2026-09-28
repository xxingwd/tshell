//! Cell-sized TUI graphics. Font ascenders/descenders must not create seams.
use gpui_kit::{Bounds, PathBuilder, Pixels, Window, fill, point, px, size};
#[path = "terminal_glyphs_table.rs"]
mod table;

#[derive(Clone, Copy, Debug)]
pub(crate) struct GlyphCell {
    pub col: usize,
    pub ch: char,
    pub color: u32,
    pub faint: bool,
}

pub(crate) fn supported(ch: char) -> bool {
    table::arms(ch).is_some()
        || double_lines(ch).is_some()
        || matches!(ch, '╭'..='╳' | '▀'..='▟' | '┄'..='┋' | '╌'..='╏')
}

// Endpoints in a five-position grid: edge, negative rail, centre, positive
// rail, edge. Separate rails preserve the empty interior of double borders.
fn double_lines(ch: char) -> Option<&'static [[usize; 4]]> {
    Some(match ch {
        '═' => &[[0, 1, 4, 1], [0, 3, 4, 3]],
        '║' => &[[1, 0, 1, 4], [3, 0, 3, 4]],
        '╔' => &[[4, 1, 1, 1], [1, 1, 1, 4], [4, 3, 3, 3], [3, 3, 3, 4]],
        '╗' => &[[0, 1, 3, 1], [3, 1, 3, 4], [0, 3, 1, 3], [1, 3, 1, 4]],
        '╚' => &[[4, 1, 3, 1], [3, 1, 3, 0], [4, 3, 1, 3], [1, 3, 1, 0]],
        '╝' => &[[0, 1, 1, 1], [1, 1, 1, 0], [0, 3, 3, 3], [3, 3, 3, 0]],
        '╠' => &[
            [1, 0, 1, 4],
            [4, 1, 3, 1],
            [3, 1, 3, 0],
            [4, 3, 3, 3],
            [3, 3, 3, 4],
        ],
        '╣' => &[
            [3, 0, 3, 4],
            [0, 1, 1, 1],
            [1, 1, 1, 0],
            [0, 3, 1, 3],
            [1, 3, 1, 4],
        ],
        '╦' => &[
            [0, 1, 4, 1],
            [0, 3, 1, 3],
            [1, 3, 1, 4],
            [4, 3, 3, 3],
            [3, 3, 3, 4],
        ],
        '╩' => &[
            [0, 3, 4, 3],
            [0, 1, 1, 1],
            [1, 1, 1, 0],
            [4, 1, 3, 1],
            [3, 1, 3, 0],
        ],
        '╬' => &[
            [0, 1, 1, 1],
            [1, 1, 1, 0],
            [4, 1, 3, 1],
            [3, 1, 3, 0],
            [0, 3, 1, 3],
            [1, 3, 1, 4],
            [4, 3, 3, 3],
            [3, 3, 3, 4],
        ],
        '╒' => &[[2, 4, 2, 1], [2, 1, 4, 1], [2, 3, 4, 3]],
        '╓' => &[[1, 4, 1, 2], [1, 2, 4, 2], [3, 2, 3, 4]],
        '╕' => &[[0, 1, 2, 1], [2, 1, 2, 4], [0, 3, 2, 3]],
        '╖' => &[[3, 4, 3, 2], [3, 2, 0, 2], [1, 2, 1, 4]],
        '╘' => &[[2, 0, 2, 3], [2, 3, 4, 3], [2, 1, 4, 1]],
        '╙' => &[[4, 2, 1, 2], [1, 2, 1, 0], [3, 2, 3, 0]],
        '╛' => &[[0, 3, 2, 3], [2, 3, 2, 0], [0, 1, 2, 1]],
        '╜' => &[[0, 2, 3, 2], [3, 2, 3, 0], [1, 2, 1, 0]],
        '╞' => &[[2, 0, 2, 4], [2, 1, 4, 1], [2, 3, 4, 3]],
        '╟' => &[[1, 0, 1, 4], [3, 0, 3, 4], [3, 2, 4, 2]],
        '╡' => &[[2, 0, 2, 4], [0, 1, 2, 1], [0, 3, 2, 3]],
        '╢' => &[[0, 2, 1, 2], [1, 0, 1, 4], [3, 0, 3, 4]],
        '╤' => &[[0, 1, 4, 1], [0, 3, 4, 3], [2, 3, 2, 4]],
        '╥' => &[[0, 2, 4, 2], [1, 2, 1, 4], [3, 2, 3, 4]],
        '╧' => &[[2, 0, 2, 1], [0, 1, 4, 1], [0, 3, 4, 3]],
        '╨' => &[[0, 2, 4, 2], [1, 2, 1, 0], [3, 2, 3, 0]],
        '╪' => &[[2, 0, 2, 4], [0, 1, 4, 1], [0, 3, 4, 3]],
        '╫' => &[[0, 2, 4, 2], [1, 0, 1, 4], [3, 0, 3, 4]],
        _ => return None,
    })
}

/// Rectangles use logical cell coordinates; paint snaps their shared edges to
/// physical pixels. All full-height segments end at exactly the next row.
pub(crate) fn rectangles(ch: char, w: f32, h: f32, scale: f32) -> Vec<[f32; 4]> {
    let thin = (w * scale / 8.).round().max(1.) / scale;
    let cx = w / 2.;
    let cy = h / 2.;
    let mut result = Vec::new();
    let mut line = |x1: f32, y1: f32, x2: f32, y2: f32, t: f32| {
        result.push(if x1 == x2 {
            [
                (x1 - t / 2.).max(0.),
                (y1.min(y2) - t / 2.).max(0.),
                (x1 + t / 2.).min(w),
                (y1.max(y2) + t / 2.).min(h),
            ]
        } else {
            [
                (x1.min(x2) - t / 2.).max(0.),
                (y1 - t / 2.).max(0.),
                (x1.max(x2) + t / 2.).min(w),
                (y1 + t / 2.).min(h),
            ]
        });
    };
    if let Some(arms) = table::arms(ch) {
        for (i, weight) in arms.into_iter().enumerate() {
            if weight == 0 {
                continue;
            }
            let (x, y) = [(0., cy), (w, cy), (cx, 0.), (cx, h)][i];
            line(cx, cy, x, y, thin * f32::from(weight));
        }
    } else if let Some(lines) = double_lines(ch) {
        let gap = thin.max(w * 0.15);
        let xs = [0., cx - gap, cx, cx + gap, w];
        let ys = [0., cy - gap, cy, cy + gap, h];
        for &[a, b, c, d] in lines {
            line(xs[a], ys[b], xs[c], ys[d], thin);
        }
    } else if matches!(ch, '┄'..='┋' | '╌'..='╏') {
        let (vertical, count, heavy) = match ch {
            '┄' | '┅' => (false, 3, ch == '┅'),
            '┆' | '┇' => (true, 3, ch == '┇'),
            '┈' | '┉' => (false, 4, ch == '┉'),
            '┊' | '┋' => (true, 4, ch == '┋'),
            '╌' | '╍' => (false, 2, ch == '╍'),
            _ => (true, 2, ch == '╏'),
        };
        let length = if vertical { h } else { w };
        for i in 0..count {
            let a = (i as f32 + 0.15) * length / count as f32;
            let b = (i as f32 + 0.85) * length / count as f32;
            let t = thin * if heavy { 2. } else { 1. };
            // Preserve intentional dash gaps rather than extending their caps.
            result.push(if vertical {
                [cx - t / 2., a, cx + t / 2., b]
            } else {
                [a, cy - t / 2., b, cy + t / 2.]
            });
        }
    } else {
        match ch {
            '▀' => result.push([0., 0., w, h / 2.]),
            '▁'..='█' => result.push([0., h * (1. - (ch as u32 - 0x2580) as f32 / 8.), w, h]),
            '▉'..='▏' => result.push([0., 0., w * (0x2590 - ch as u32) as f32 / 8., h]),
            '▐' => result.push([w / 2., 0., w, h]),
            '▔' => result.push([0., 0., w, h / 8.]),
            '▕' => result.push([w * 7. / 8., 0., w, h]),
            '▖'..='▟' => {
                let mask = [4, 8, 1, 13, 9, 7, 11, 2, 6, 14][ch as usize - 0x2596];
                for i in 0..4 {
                    if mask & (1 << i) != 0 {
                        let x = (i % 2) as f32 * w / 2.;
                        let y = (i / 2) as f32 * h / 2.;
                        result.push([x, y, x + w / 2., y + h / 2.]);
                    }
                }
            }
            // Shade cells are a stable grid pattern, not font-size-dependent dots.
            '░'..='▓' => {
                for y in 0..8 {
                    for x in 0..4 {
                        let value = (x + y * 2) % 4;
                        if value < ch as u32 - 0x2590 {
                            result.push([
                                x as f32 * w / 4.,
                                y as f32 * h / 8.,
                                (x + 1) as f32 * w / 4.,
                                (y + 1) as f32 * h / 8.,
                            ]);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    result
}

pub(crate) fn paint(glyph: GlyphCell, bounds: Bounds<Pixels>, window: &mut Window) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let scale = window.scale_factor();
    let snap = |v: f32| (v * scale).round() / scale;
    let ox = f32::from(bounds.origin.x);
    let oy = f32::from(bounds.origin.y);
    let rectangles = rectangles(glyph.ch, w, h, scale)
        .into_iter()
        .map(|[x1, y1, x2, y2]| [snap(ox + x1), snap(oy + y1), snap(ox + x2), snap(oy + y2)])
        .collect::<Vec<_>>();
    // A semi-transparent crossing must be composited once, not once per arm.
    let rectangles = if glyph.faint {
        disjoint_rectangles(&rectangles)
    } else {
        rectangles
    };
    for [left, top, right, bottom] in rectangles {
        window.paint_quad(fill(
            Bounds::new(
                point(px(left), px(top)),
                size(px(right - left), px(bottom - top)),
            ),
            crate::terminal_decorations::ink(glyph.color, glyph.faint),
        ));
    }
    if matches!(glyph.ch, '╭'..='╰') {
        let thin = (w * scale / 8.).round().max(1.) / scale;
        // Curve centres lie half a stroke from an integer device-pixel edge.
        let cx = snap(ox + w / 2. - thin / 2.) + thin / 2.;
        let cy = snap(oy + h / 2. - thin / 2.) + thin / 2.;
        let right = matches!(glyph.ch, '╭' | '╰');
        let down = matches!(glyph.ch, '╭' | '╮');
        let x = snap(if right { ox + w } else { ox });
        let y = snap(if down { oy + h } else { oy });
        let radius = (x - cx).abs().min((y - cy).abs());
        let sy = if down { 1. } else { -1. };
        let sx = if right { 1. } else { -1. };
        let mut path = PathBuilder::stroke(px(thin));
        path.move_to(point(px(cx), px(y)));
        path.line_to(point(px(cx), px(cy + sy * radius)));
        path.curve_to(point(px(cx + sx * radius), px(cy)), point(px(cx), px(cy)));
        path.line_to(point(px(x), px(cy)));
        if let Ok(path) = path.build() {
            window.paint_path(
                path,
                crate::terminal_decorations::ink(glyph.color, glyph.faint),
            );
        }
    }
    if matches!(glyph.ch, '╱'..='╳') {
        let thin = (w * scale / 8.).round().max(1.) / scale;
        let mut path = PathBuilder::stroke(px(thin));
        if glyph.ch != '╲' {
            path.move_to(bounds.top_right());
            path.line_to(bounds.bottom_left());
        }
        if glyph.ch != '╱' {
            path.move_to(bounds.origin);
            path.line_to(bounds.bottom_right());
        }
        if let Ok(path) = path.build() {
            window.paint_path(
                path,
                crate::terminal_decorations::ink(glyph.color, glyph.faint),
            );
        }
    }
}

fn disjoint_rectangles(rects: &[[f32; 4]]) -> Vec<[f32; 4]> {
    let mut ys = rects.iter().flat_map(|r| [r[1], r[3]]).collect::<Vec<_>>();
    ys.sort_by(f32::total_cmp);
    ys.dedup();
    let mut result = Vec::new();
    for band in ys.windows(2) {
        let mut spans = rects
            .iter()
            .filter(|r| r[1] <= band[0] && r[3] >= band[1])
            .map(|r| (r[0], r[2]))
            .collect::<Vec<_>>();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut current: Option<(f32, f32)> = None;
        for (left, right) in spans {
            match current {
                Some((start, end)) if left <= end => current = Some((start, end.max(right))),
                Some((start, end)) => {
                    result.push([start, band[0], end, band[1]]);
                    current = Some((left, right));
                }
                None => current = Some((left, right)),
            }
        }
        if let Some((left, right)) = current {
            result.push([left, band[0], right, band[1]]);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn faint_border_junctions_composite_exactly_once() {
        for ch in ['│', '┼', '╬', '╔', '┿', '█', '▓'] {
            let original = rectangles(ch, 12., 24., 1.);
            let disjoint = disjoint_rectangles(&original);
            for y in 0..24 {
                for x in 0..12 {
                    let contains = |r: &&[f32; 4]| {
                        x as f32 + 0.5 >= r[0]
                            && x as f32 + 0.5 < r[2]
                            && y as f32 + 0.5 >= r[1]
                            && y as f32 + 0.5 < r[3]
                    };
                    let expected = original.iter().any(|r| contains(&r));
                    let layers = disjoint.iter().filter(contains).count();
                    assert_eq!(layers, usize::from(expected), "{ch} pixel {x},{y}");
                }
            }
        }
    }
    #[test]
    fn continuous_edges_survive_font_sizes_and_fractional_scaling() {
        for w in [6.3, 8.4, 14.4] {
            for scale in [1., 1.25, 1.5, 2.] {
                let h = (w * 2.5_f32).ceil();
                for ch in ['│', '┃', '║'] {
                    let r = rectangles(ch, w, h, scale);
                    assert!(!r.is_empty());
                    assert!(r.iter().any(|r| r[1] == 0.));
                    assert!(r.iter().any(|r| r[3] == h));
                    for y in 0..(h * scale).round() as usize {
                        let y = y as f32 + 0.5;
                        assert!(r.iter().any(|r| y>=(r[1]*scale).round() && y<=(r[3]*scale).round()));
                    }
                    // Adjacent rows share exactly the same rounded device boundary.
                    let bottom = r.iter().map(|r| r[3]).fold(0., f32::max);
                    let top = r.iter().map(|r| r[1]).fold(h, f32::min);
                    for row in 0..60 {
                        assert_eq!(
                            ((row as f32 * h + bottom) * scale).round(),
                            (((row + 1) as f32 * h + top) * scale).round()
                        );
                    }
                }
                for ch in ['─', '━', '═'] {
                    let r = rectangles(ch, w, h, scale);
                    assert!(r.iter().any(|r| r[0] == 0.) && r.iter().any(|r| r[2] == w));
                }
                assert_eq!(rectangles('█', w, h, scale), vec![[0., 0., w, h]]);
            }
        }
        assert!(!supported('|')); // Shell pipes retain their font glyph.
        assert!(!supported('¦')); // Broken bar must remain broken.
        assert_eq!(table::arms('┌'), Some([0, 1, 0, 1]));
        assert_eq!(table::arms('╋'), Some([2, 2, 2, 2]));
        assert_eq!(table::arms('┾'), Some([1, 2, 1, 1]));
    }
}
