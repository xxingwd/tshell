//! Pure local pane layout; no process or terminal IO.
use super::{PANE_GAP, PANE_PADDING, PaneBounds, PaneInfo, SplitAxis};

#[derive(Clone)]
pub(super) enum Layout {
    Leaf(String),
    Split {
        axis: SplitAxis,
        ratio: f32,
        first: Box<Layout>,
        second: Box<Layout>,
    },
}
impl Layout {
    fn axis_count(&self, axis: SplitAxis) -> usize {
        match self {
            Self::Split {
                axis: own,
                first,
                second,
                ..
            } if *own == axis => first.axis_count(axis) + second.axis_count(axis),
            _ => 1,
        }
    }

    pub(super) fn pixel_panes(&self, bounds: PaneBounds, out: &mut Vec<PaneBounds>) {
        match self {
            Self::Leaf(id) => out.push(PaneBounds {
                id: id.clone(),
                ..bounds
            }),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let extent = match axis {
                    SplitAxis::Horizontal => bounds.width,
                    SplitAxis::Vertical => bounds.height,
                };
                let first_gaps = first.axis_count(*axis) - 1;
                let second_gaps = second.axis_count(*axis) - 1;
                let available = (extent - (first_gaps + second_gaps + 1) as f32 * PANE_GAP).max(0.);
                let size = (available * ratio + first_gaps as f32 * PANE_GAP)
                    .round()
                    .clamp(0., (extent - PANE_GAP).max(0.));
                let mut a = bounds.clone();
                let mut b = bounds;
                match axis {
                    SplitAxis::Horizontal => {
                        a.width = size;
                        b.x += size + PANE_GAP;
                        b.width = (b.width - size - PANE_GAP).max(0.);
                    }
                    SplitAxis::Vertical => {
                        a.height = size;
                        b.y += size + PANE_GAP;
                        b.height = (b.height - size - PANE_GAP).max(0.);
                    }
                }
                first.pixel_panes(a, out);
                second.pixel_panes(b, out);
            }
        }
    }

    pub(super) fn contains(&self, target: &str) -> bool {
        match self {
            Self::Leaf(id) => id == target,
            Self::Split { first, second, .. } => first.contains(target) || second.contains(target),
        }
    }
    pub(super) fn swap(&mut self, a: &str, b: &str) {
        match self {
            Self::Leaf(id) => {
                if id == a {
                    *id = b.into();
                } else if id == b {
                    *id = a.into();
                }
            }
            Self::Split { first, second, .. } => {
                first.swap(a, b);
                second.swap(a, b);
            }
        }
    }
    pub(super) fn pane_ids(&self, out: &mut Vec<String>) {
        match self {
            Self::Leaf(id) => out.push(id.clone()),
            Self::Split { first, second, .. } => {
                first.pane_ids(out);
                second.pane_ids(out);
            }
        }
    }
    fn collect_axis(layout: Self, axis: SplitAxis, out: &mut Vec<Self>) {
        match layout {
            Self::Split {
                axis: own,
                first,
                second,
                ..
            } if own == axis => {
                Self::collect_axis(*first, axis, out);
                Self::collect_axis(*second, axis, out);
            }
            layout => out.push(layout),
        }
    }
    fn equal_group(mut layouts: Vec<Self>, axis: SplitAxis) -> Self {
        debug_assert!(!layouts.is_empty());
        if layouts.len() == 1 {
            return layouts.pop().unwrap();
        }
        let count = layouts.len();
        let first = layouts.remove(0);
        Self::Split {
            axis,
            ratio: 1. / count as f32,
            first: Box::new(first),
            second: Box::new(Self::equal_group(layouts, axis)),
        }
    }
    pub(super) fn add_column(&mut self, new: &str) {
        let mut columns = Vec::new();
        Self::collect_axis(self.clone(), SplitAxis::Horizontal, &mut columns);
        columns.push(Self::Leaf(new.into()));
        *self = Self::equal_group(columns, SplitAxis::Horizontal);
    }
    pub(super) fn add_row(&mut self, target: &str, new: &str) -> bool {
        if let Self::Split {
            axis: SplitAxis::Horizontal,
            first,
            second,
            ..
        } = self
        {
            return if first.contains(target) {
                first.add_row(target, new)
            } else {
                second.add_row(target, new)
            };
        }
        if !self.contains(target) {
            return false;
        }
        let mut rows = Vec::new();
        Self::collect_axis(self.clone(), SplitAxis::Vertical, &mut rows);
        let index = rows
            .iter()
            .position(|row| row.contains(target))
            .unwrap_or(rows.len() - 1);
        rows.insert(index + 1, Self::Leaf(new.into()));
        *self = Self::equal_group(rows, SplitAxis::Vertical);
        true
    }
    pub(super) fn normalized(self) -> Self {
        match self {
            Self::Leaf(_) => self,
            Self::Split {
                axis,
                first,
                second,
                ..
            } => {
                let mut layouts = Vec::new();
                Self::collect_axis(first.normalized(), axis, &mut layouts);
                Self::collect_axis(second.normalized(), axis, &mut layouts);
                Self::equal_group(layouts, axis)
            }
        }
    }
    pub(super) fn cycle(&mut self) {
        let axis = match self {
            Self::Split {
                axis: SplitAxis::Horizontal,
                ..
            } => SplitAxis::Vertical,
            _ => SplitAxis::Horizontal,
        };
        let mut ids = Vec::new();
        self.pane_ids(&mut ids);
        *self = Self::equal_group(ids.into_iter().map(Self::Leaf).collect(), axis);
    }
    pub(super) fn without(self, target: &str) -> Option<Self> {
        match self {
            Self::Leaf(id) => (id != target).then_some(Self::Leaf(id)),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => match (first.without(target), second.without(target)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (one, None) | (None, one) => one,
            },
        }
    }
    pub(super) fn panes(
        &self,
        x: usize,
        y: usize,
        cols: usize,
        rows: usize,
        out: &mut Vec<PaneInfo>,
    ) {
        match self {
            Self::Leaf(id) => out.push(PaneInfo {
                cwd: String::new(),
                id: id.clone(),
                title: crate::t!("term.shell").to_string(),
                notice_count: 0,
                x,
                y,
                cols,
                rows,
            }),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let extent = if *axis == SplitAxis::Horizontal {
                    cols
                } else {
                    rows
                };
                let first_size = ((extent.saturating_sub(1)) as f32 * ratio).round() as usize;
                let first_size = first_size.max(1).min(extent.saturating_sub(2).max(1));
                let second_size = extent.saturating_sub(first_size + 1).max(1);
                if *axis == SplitAxis::Horizontal {
                    first.panes(x, y, first_size, rows, out);
                    second.panes(x + first_size + 1, y, second_size, rows, out);
                } else {
                    first.panes(x, y, cols, first_size, out);
                    second.panes(x, y + first_size + 1, cols, second_size, out);
                }
            }
        }
    }
    pub(super) fn adjust(
        &mut self,
        target: &str,
        axis: SplitAxis,
        amount: i32,
        cols: usize,
        rows: usize,
    ) -> bool {
        match self {
            Self::Leaf(_) => false,
            Self::Split {
                axis: own,
                ratio,
                first,
                second,
            } => {
                let extent = if *own == SplitAxis::Horizontal {
                    cols
                } else {
                    rows
                };
                let a = ((extent.saturating_sub(1)) as f32 * *ratio) as usize;
                let (fc, fr, sc, sr) = if *own == SplitAxis::Horizontal {
                    (a, rows, cols.saturating_sub(a + 1), rows)
                } else {
                    (cols, a, cols, rows.saturating_sub(a + 1))
                };
                if first.adjust(target, axis, amount, fc, fr)
                    || second.adjust(target, axis, amount, sc, sr)
                {
                    return true;
                }
                if *own != axis {
                    return false;
                }
                let sign = if first.contains(target) {
                    1.
                } else if second.contains(target) {
                    -1.
                } else {
                    return false;
                };
                *ratio = (*ratio + sign * amount as f32 / extent.max(1) as f32).clamp(0.1, 0.9);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn pixel_columns_fill_viewport_with_equal_widths_and_one_pixel_dividers() {
        for count in 2..=12 {
            let mut layout = Layout::Leaf("0".into());
            for id in 1..count {
                layout.add_column(&id.to_string());
            }
            for width in [401., 1000., 1373.] {
                let mut panes = Vec::new();
                layout.pixel_panes(
                    PaneBounds {
                        id: String::new(),
                        x: 0.,
                        y: 0.,
                        width,
                        height: 701.,
                    },
                    &mut panes,
                );
                let min = panes.iter().map(|p| p.width).fold(f32::INFINITY, f32::min);
                let max = panes.iter().map(|p| p.width).fold(0., f32::max);
                assert!(max - min <= 1., "{count} panes in {width}px: {panes:?}");
                assert_eq!(panes[0].x, 0.);
                let last = panes.last().unwrap();
                assert_eq!(last.x + last.width, width);
                for pair in panes.windows(2) {
                    assert_eq!(pair[1].x - pair[0].x - pair[0].width, 1.);
                }
            }
        }
    }

    #[test]
    fn pixel_rows_and_terminal_grid_use_inner_bounds() {
        let mut layout = Layout::Leaf("a".into());
        layout.add_column("b");
        layout.add_row("b", "c");
        layout.add_row("c", "d");
        let mut panes = Vec::new();
        layout.pixel_panes(
            PaneBounds {
                id: String::new(),
                x: 0.,
                y: 0.,
                width: 1001.,
                height: 602.,
            },
            &mut panes,
        );
        assert_eq!(
            panes.iter().map(|p| p.width).collect::<Vec<_>>(),
            vec![500.; 4]
        );
        assert_eq!(
            panes.iter().map(|p| p.height).collect::<Vec<_>>(),
            vec![602., 200., 200., 200.]
        );
        assert_eq!(panes[3].y + panes[3].height, 602.);
        let viewport = super::super::PixelViewport {
            width: 1001.,
            height: 602.,
            cell_width: 8.,
            line_height: 21.,
        };
        assert_eq!(viewport.terminal_size(), (124, 28));
        assert_eq!(panes[1].terminal_size(viewport), (61, 9));
        assert!(61. * 8. + PANE_PADDING <= panes[1].width);
        assert!(9. * 21. + PANE_PADDING <= panes[1].height);
    }

    fn geometry(layout: &Layout) -> Vec<(String, usize, usize, usize, usize)> {
        let mut panes = Vec::new();
        layout.panes(0, 0, 101, 31, &mut panes);
        panes
            .into_iter()
            .map(|p| (p.id, p.x, p.y, p.cols, p.rows))
            .collect()
    }

    #[test]
    fn resizing_targets_the_nearest_matching_split() {
        let mut layout = Layout::Leaf("a".into());
        layout.add_column("b");
        layout.add_row("b", "c");
        assert!(layout.adjust("c", SplitAxis::Vertical, 3, 101, 31));
        assert!(layout.adjust("b", SplitAxis::Horizontal, 10, 101, 31));
        assert_eq!(
            geometry(&layout),
            [
                ("a".into(), 0, 0, 40, 31),
                ("b".into(), 41, 0, 60, 12),
                ("c".into(), 41, 13, 60, 18),
            ]
        );
        let before = geometry(&layout);
        assert!(!layout.adjust("missing", SplitAxis::Horizontal, 10, 101, 31));
        assert_eq!(geometry(&layout), before);
    }

    #[test]
    fn removing_panes_preserves_order_and_collapses_empty_branches() {
        let mut layout = Layout::Leaf("a".into());
        layout.add_column("b");
        layout.add_row("b", "c");
        let layout = layout.without("b").unwrap().normalized();
        assert_eq!(
            geometry(&layout),
            [("a".into(), 0, 0, 50, 31), ("c".into(), 51, 0, 50, 31),]
        );
        let layout = layout.without("a").unwrap();
        assert_eq!(geometry(&layout), [("c".into(), 0, 0, 101, 31)]);
        assert!(layout.without("c").is_none());
    }

    #[test]
    fn layout_adds_equal_columns_and_equal_rows_within_one_column() {
        let mut layout = Layout::Leaf("a".into());
        layout.add_column("b");
        layout.add_column("c");
        layout.add_row("b", "d");

        let mut panes = Vec::new();
        layout.panes(0, 0, 101, 31, &mut panes);
        let pane = |id: &str| panes.iter().find(|pane| pane.id == id).unwrap();
        assert_eq!(
            (pane("a").cols, pane("b").cols, pane("c").cols),
            (33, 33, 33)
        );
        assert_eq!((pane("a").rows, pane("c").rows), (31, 31));
        assert_eq!((pane("b").rows, pane("d").rows), (15, 15));
        assert_eq!(pane("b").x, pane("d").x);

        layout.add_column("e");
        panes.clear();
        layout.panes(0, 0, 101, 31, &mut panes);
        let column_widths: BTreeSet<_> = panes
            .iter()
            .filter(|pane| pane.y == 0)
            .map(|pane| pane.cols)
            .collect();
        assert_eq!(column_widths, BTreeSet::from([24, 25]));
        assert_eq!(
            panes
                .iter()
                .map(|pane| pane.x)
                .collect::<BTreeSet<_>>()
                .len(),
            4
        );
    }
}
