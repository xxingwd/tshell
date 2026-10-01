use super::*;
use gpui_kit::base::{
    TextSelectionContentKey, TextSelectionHandle, TextSelectionRegistration, TextSelectionRun,
};
use std::{ops::Range, sync::Arc};

#[derive(Default)]
pub(super) struct DiffDocument {
    text: String,
    lines: Vec<Option<Range<usize>>>,
}

impl DiffDocument {
    pub(super) fn new(lines: impl IntoIterator<Item = Option<String>>) -> Arc<Self> {
        let mut document = Self::default();
        let mut first = true;
        for line in lines {
            document.lines.push(line.map(|line| {
                if !first {
                    document.text.push('\n');
                }
                first = false;
                let start = document.text.len();
                document.text.push_str(&line);
                start..document.text.len()
            }));
        }
        Arc::new(document)
    }

    fn selected(&self, start: usize, end: usize) -> String {
        let start = start.min(self.text.len());
        let end = end.min(self.text.len());
        self.text
            .get(start.min(end)..start.max(end))
            .unwrap_or_default()
            .to_owned()
    }
}

struct PaintedLine {
    source: usize,
    bounds: Bounds<Pixels>,
    shaped: ShapedLine,
}

pub(super) struct DiffSelection {
    handle: TextSelectionHandle,
    document: Arc<DiffDocument>,
    bounds: Bounds<Pixels>,
    painted: Vec<PaintedLine>,
    runs: Vec<TextSelectionRun>,
    #[cfg(debug_assertions)]
    hitbox: Option<Hitbox>,
}

impl DiffSelection {
    pub(super) fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|cx: &mut Context<Self>| {
            let handle = TextSelectionHandle::new("", cx);
            let view = cx.weak_entity();
            let copy = view.clone();
            handle.copy_with(
                move |cx| {
                    copy.upgrade()
                        .map(|view| view.read(cx).selected(cx))
                        .unwrap_or_default()
                },
                cx,
            );
            handle.resolve_content_key_with(
                move |point, cx| {
                    view.upgrade()
                        .and_then(|view| view.read(cx).index_at(point))
                        .map(|index| TextSelectionContentKey::new(index as u64))
                },
                cx,
            );
            Self {
                handle,
                document: Arc::default(),
                bounds: Bounds::default(),
                painted: Vec::new(),
                runs: Vec::new(),
                #[cfg(debug_assertions)]
                hitbox: None,
            }
        })
    }

    pub(super) fn set_document(&mut self, document: Arc<DiffDocument>) {
        if !Arc::ptr_eq(&self.document, &document) {
            self.document = document;
            self.painted.clear();
        }
    }

    fn range(&self, cx: &App) -> Option<Range<usize>> {
        let snapshot = self.handle.snapshot(cx)?;
        if snapshot.anchor().entity_id() != Some(self.handle.entity_id()) {
            return None;
        }
        let a = snapshot.anchor().content_key()?.value() as usize;
        let b = if snapshot.cursor().entity_id() == Some(self.handle.entity_id()) {
            snapshot.cursor().content_key()?.value() as usize
        } else {
            let position = snapshot.window_points()?.cursor() - self.bounds.origin;
            self.index_at(point(
                position.x.clamp(px(0.), self.bounds.size.width),
                position.y,
            ))?
        };
        Some(a.min(b)..a.max(b))
    }

    fn selected(&self, cx: &App) -> String {
        self.range(cx)
            .map(|range| self.document.selected(range.start, range.end))
            .unwrap_or_default()
    }

    fn index_at(&self, point: Point<Pixels>) -> Option<usize> {
        let point = self.bounds.origin + point;
        let line = self.painted.iter().min_by(|a, b| {
            let distance =
                |line: &PaintedLine| (f32::from(point.y - line.bounds.origin.y) - 12.).abs();
            distance(a).total_cmp(&distance(b))
        })?;
        let source = self.document.lines.get(line.source)?.as_ref()?;
        let raw = &self.document.text[source.clone()];
        let index = line
            .shaped
            .closest_index_for_x(point.x - line.bounds.origin.x);
        Some(source.start + raw_index(raw, index))
    }

    #[cfg(debug_assertions)]
    pub(super) fn check_points(&self, source: usize) -> Option<(Point<Pixels>, Point<Pixels>)> {
        let line = self.painted.iter().find(|line| line.source == source)?;
        Some((
            line.bounds.origin + point(px(0.1), px(12.)),
            line.bounds.origin + point(line.shaped.width + px(1.), px(12.)),
        ))
    }

    #[cfg(debug_assertions)]
    pub(super) fn check_word_point(&self, source: usize) -> Option<Point<Pixels>> {
        let line = self.painted.iter().find(|line| line.source == source)?;
        Some(line.bounds.origin + point(line.shaped.x_for_index(5) + px(1.), px(12.)))
    }

    #[cfg(debug_assertions)]
    pub(super) fn check_debug(&self, window: &Window, cx: &App) -> String {
        format!(
            "bounds={:?} painted={} hover={} snapshot={:?}",
            self.bounds,
            self.painted.len(),
            self.hitbox
                .as_ref()
                .is_some_and(|hitbox| hitbox.is_hovered(window)),
            self.handle.snapshot(cx)
        )
    }

    pub(super) fn surface(view: Entity<Self>) -> impl IntoElement {
        let prepare = view.clone();
        canvas(
            move |bounds, window, cx| {
                prepare.update(cx, |this, cx| {
                    this.bounds = bounds;
                    this.painted.clear();
                    this.runs.clear();
                    let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                    #[cfg(debug_assertions)]
                    {
                        this.hitbox = Some(hitbox.clone());
                    }
                    this.handle.register(
                        TextSelectionRegistration::new(hitbox, bounds)
                            .with_text_bounds(vec![bounds]),
                        window,
                        cx,
                    );
                });
            },
            move |_, _, _, cx| {
                view.update(cx, |this, cx| {
                    this.handle.update_runs(&this.runs, cx);
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    pub(super) fn row(
        view: Entity<Self>,
        source: usize,
        text: String,
        highlights: Vec<Range<usize>>,
        font: SharedString,
        font_size: f32,
        color: u32,
        emphasis: u32,
        selection: u32,
    ) -> impl IntoElement {
        let prepare = view.clone();
        let display = text.replace('\t', "    ");
        let mut ranges: Vec<_> = highlights
            .iter()
            .map(|range| display_index(&text, range.start)..display_index(&text, range.end))
            .collect();
        ranges.retain(|range| !range.is_empty());
        canvas(
            move |bounds, window, cx| {
                let mut runs = Vec::new();
                let mut previous = 0;
                for range in &ranges {
                    if range.start > previous {
                        runs.push(text_run(range.start - previous, &font, color, None));
                    }
                    runs.push(text_run(range.len(), &font, color, Some(emphasis)));
                    previous = range.end;
                }
                if previous < display.len() {
                    runs.push(text_run(display.len() - previous, &font, color, None));
                }
                let text = StyledText::new(display.clone()).with_runs(runs.clone());
                let layout = text.layout().clone();
                let mut element = text.into_any_element();
                element.layout_as_root(
                    size(
                        AvailableSpace::MaxContent,
                        AvailableSpace::Definite(px(24.)),
                    ),
                    window,
                    cx,
                );
                element.prepaint_at(bounds.origin, window, cx);
                let shaped = window.text_system().shape_line(
                    display.clone().into(),
                    px(font_size),
                    &runs,
                    None,
                );
                prepare.update(cx, |this, _| {
                    this.runs.push(
                        TextSelectionRun::new(display.clone(), layout, bounds)
                            .with_document_order(source as u64),
                    );
                    this.painted.push(PaintedLine {
                        source,
                        bounds,
                        shaped: shaped.clone(),
                    });
                });
                shaped
            },
            move |bounds, shaped, window, cx| {
                let _ = shaped.paint_background(
                    bounds.origin,
                    px(24.),
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
                if let Some(range) = view.read(cx).range(cx)
                    && let Some(Some(line)) = view.read(cx).document.lines.get(source)
                {
                    let start = range.start.max(line.start);
                    let end = range.end.min(line.end);
                    if start < end {
                        let raw = &view.read(cx).document.text[line.clone()];
                        let x1 = shaped.x_for_index(display_index(raw, start - line.start));
                        let x2 = shaped.x_for_index(display_index(raw, end - line.start));
                        window.paint_quad(fill(
                            Bounds::new(bounds.origin + point(x1, px(0.)), size(x2 - x1, px(24.))),
                            rgba((selection << 8) | 160),
                        ));
                    }
                }
                let _ = shaped.paint(bounds.origin, px(24.), TextAlign::Left, None, window, cx);
            },
        )
        .w_full()
        .h(px(24.))
    }
}

fn text_run(len: usize, family: &SharedString, color: u32, background: Option<u32>) -> TextRun {
    TextRun {
        len,
        font: font(family.clone()),
        color: rgb(color).into(),
        background_color: background.map(|color| rgba((color << 8) | 70).into()),
        underline: None,
        strikethrough: None,
    }
}

fn display_index(raw: &str, index: usize) -> usize {
    raw.get(..index)
        .unwrap_or_default()
        .chars()
        .map(|c| if c == '\t' { 4 } else { c.len_utf8() })
        .sum()
}

fn raw_index(raw: &str, display: usize) -> usize {
    let mut position = 0;
    for (index, c) in raw.char_indices() {
        let end = position + if c == '\t' { 4 } else { c.len_utf8() };
        if display < end {
            return index;
        }
        position = end;
    }
    raw.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn copy_preserves_tabs_unicode_and_invisible_intermediate_lines() {
        let document = DiffDocument::new([
            Some("\t中文".into()),
            None,
            Some("middle".into()),
            Some("tail".into()),
        ]);
        let first = document.lines[0].clone().unwrap();
        let last = document.lines[3].clone().unwrap();
        assert_eq!(
            document.selected(first.start, last.end),
            "\t中文\nmiddle\ntail"
        );
        assert_eq!(display_index("\t中文", 4), 7);
        assert_eq!(raw_index("\t中文", 7), 4);
        assert_eq!(raw_index("\t中文", 3), 0);
    }
}
