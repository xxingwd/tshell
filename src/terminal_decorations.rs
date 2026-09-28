//! SGR decorations are cell geometry, independent of a font's underline support.
use alacritty_terminal::term::cell::Flags;
use gpui_kit::{Bounds, PathBuilder, Pixels, Window, fill, point, px, rgb};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Underline {
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}
impl Underline {
    pub fn from_flags(flags: Flags) -> Option<Self> {
        if flags.contains(Flags::DOUBLE_UNDERLINE) {
            Some(Self::Double)
        } else if flags.contains(Flags::UNDERCURL) {
            Some(Self::Curly)
        } else if flags.contains(Flags::DOTTED_UNDERLINE) {
            Some(Self::Dotted)
        } else if flags.contains(Flags::DASHED_UNDERLINE) {
            Some(Self::Dashed)
        } else if flags.contains(Flags::UNDERLINE) {
            Some(Self::Single)
        } else {
            None
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Decoration {
    pub col: usize,
    pub columns: usize,
    pub kind: Option<Underline>, // None is strikethrough.
    pub color: u32,
    pub faint: bool,
}

pub(crate) fn ink(color: u32, faint: bool) -> gpui_kit::Hsla {
    let mut ink: gpui_kit::Hsla = rgb(color).into();
    ink.a = if faint { 0.5 } else { 1. };
    ink
}

pub(crate) fn paint(
    d: Decoration,
    bounds: Bounds<Pixels>,
    cell_width: f32,
    font_size: f32,
    window: &mut Window,
) {
    let scale = window.scale_factor();
    let snap = |v: f32| (v * scale).round() / scale;
    let stroke = (font_size * scale / if d.kind.is_none() { 10. } else { 15. })
        .floor()
        .max(1.)
        / scale;
    let left = f32::from(bounds.left());
    let top = f32::from(bounds.top());
    let height = f32::from(bounds.size.height);
    let baseline = (height + font_size) / 2.;
    let y = snap(
        top + if d.kind.is_none() {
            height / 2.
        } else {
            baseline.min(height - 3. * stroke)
        },
    );
    let mut segment = |x1: f32, y1: f32, x2: f32| {
        let x1 = snap(x1);
        let x2 = snap(x2);
        window.paint_quad(fill(
            Bounds::from_corners(point(px(x1), px(y1)), point(px(x2), px(y1 + stroke))),
            ink(d.color, d.faint),
        ));
    };
    match d.kind {
        None | Some(Underline::Single) => segment(left, y, left + d.columns as f32 * cell_width),
        Some(Underline::Double) => {
            segment(left, y, left + d.columns as f32 * cell_width);
            segment(left, y + 2. * stroke, left + d.columns as f32 * cell_width);
        }
        Some(Underline::Dotted) => {
            // Phase is based on the absolute column, preserving dotted rhythm
            // across adjacent cells even when colour/style splits the text run.
            let start = (d.col as f32 * cell_width * scale).round() as usize;
            let end = ((d.col + d.columns) as f32 * cell_width * scale).round() as usize;
            let dot = (stroke * scale).round() as usize;
            let mut pixel = start;
            while pixel < end {
                let boundary = ((pixel / dot) + 1) * dot;
                if (pixel / dot) % 2 == 0 {
                    segment(
                        left + (pixel - start) as f32 / scale,
                        y,
                        left + (boundary.min(end) - start) as f32 / scale,
                    );
                }
                pixel = boundary;
            }
        }
        Some(Underline::Dashed) => {
            for col in 0..d.columns {
                let x = left + col as f32 * cell_width;
                segment(x, y, x + cell_width * 0.6);
                segment(x + cell_width * 0.9, y, x + cell_width);
            }
        }
        Some(Underline::Curly) => {
            let mut path = PathBuilder::stroke(px(stroke));
            let mid = y + stroke;
            path.move_to(point(px(left), px(mid)));
            for col in 0..d.columns {
                let x = left + col as f32 * cell_width;
                path.cubic_bezier_to(
                    point(px(x + cell_width / 2.), px(mid)),
                    point(px(x), px(y + 2. * stroke)),
                    point(px(x + cell_width / 2.), px(y + 2. * stroke)),
                );
                path.cubic_bezier_to(
                    point(px(x + cell_width), px(mid)),
                    point(px(x + cell_width / 2.), px(y)),
                    point(px(x + cell_width), px(y)),
                );
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, ink(d.color, d.faint));
            }
        }
    }
}
