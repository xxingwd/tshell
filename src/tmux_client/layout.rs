//! Client-computed column/row geometry encoded for tmux's classic layout format.
use super::q;
use crate::backend::{PaneInfo, SplitAxis, WindowInfo};
use std::collections::BTreeMap;

pub(super) fn pane_columns(window: &WindowInfo) -> BTreeMap<usize, Vec<&PaneInfo>> {
    let mut columns: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for pane in &window.panes {
        columns.entry(pane.x).or_default().push(pane);
    }
    for rows in columns.values_mut() {
        rows.sort_by_key(|pane| pane.y);
    }
    columns
}

fn sizes_are_equal(mut values: impl Iterator<Item = usize>) -> bool {
    let Some(first) = values.next() else {
        return true;
    };
    let (mut min, mut max) = (first, first);
    for value in values {
        min = min.min(value);
        max = max.max(value);
    }
    max - min <= 1
}

// Old select-layout -E results can extend beyond the window in tmux 3.8.
// Recognize the intact column groups separately from their total width.
pub(super) fn column_layout_width(window: &WindowInfo) -> Option<usize> {
    let columns = pane_columns(window);
    if columns.is_empty() {
        return None;
    }
    let mut expected_x = 0;
    for (&x, rows) in &columns {
        let first = rows.first()?;
        if x != expected_x || rows.iter().any(|pane| pane.cols != first.cols) {
            return None;
        }
        let mut expected_y = 0;
        for pane in rows {
            if pane.y != expected_y || pane.cols == 0 || pane.rows == 0 {
                return None;
            }
            expected_y = pane.y.checked_add(pane.rows)?.checked_add(1)?;
        }
        if expected_y.checked_sub(1)? != window.rows {
            return None;
        }
        expected_x = x.checked_add(first.cols)?.checked_add(1)?;
    }
    expected_x.checked_sub(1)
}

pub(super) fn is_column_layout(window: &WindowInfo) -> bool {
    column_layout_width(window) == Some(window.cols)
}

pub(super) fn is_equal_column_layout(window: &WindowInfo) -> bool {
    let columns = pane_columns(window);
    is_column_layout(window)
        && sizes_are_equal(columns.values().map(|rows| rows[0].cols))
        && columns
            .values()
            .all(|rows| sizes_are_equal(rows.iter().map(|pane| pane.rows)))
}

fn equal_sizes(extent: usize, count: usize) -> Option<Vec<usize>> {
    let available = extent.checked_sub(count.checked_sub(1)?)?;
    if available < count {
        return None;
    }
    Some(
        (0..count)
            .map(|index| available / count + usize::from(index < available % count))
            .collect(),
    )
}

pub(super) fn can_equalize(window: &WindowInfo) -> bool {
    if window.panes.is_empty() || window.zoomed {
        return false;
    }
    let columns = pane_columns(window);
    equal_sizes(window.cols, columns.len()).is_some()
        && columns
            .values()
            .all(|rows| equal_sizes(window.rows, rows.len()).is_some())
}

fn layout_group(
    cols: usize,
    rows: usize,
    x: usize,
    children: &[String],
    axis: SplitAxis,
) -> String {
    if children.len() == 1 {
        return children[0].clone();
    }
    let (opening, closing) = match axis {
        SplitAxis::Horizontal => ('{', '}'),
        SplitAxis::Vertical => ('[', ']'),
    };
    format!(
        "{cols}x{rows},{x},0{opening}{}{closing}",
        children.join(",")
    )
}

pub(super) fn equalize_commands(window: &WindowInfo) -> Vec<String> {
    if window.panes.is_empty() || window.zoomed {
        return Vec::new();
    }
    let columns = pane_columns(window);
    let Some(widths) = equal_sizes(window.cols, columns.len()) else {
        return Vec::new();
    };
    let mut column_layouts = Vec::new();
    let mut pane_order = Vec::new();
    let mut x = 0;
    for (rows, cols) in columns.values().zip(widths) {
        let Some(heights) = equal_sizes(window.rows, rows.len()) else {
            return Vec::new();
        };
        let mut row_layouts = Vec::new();
        let mut y = 0;
        for (pane, height) in rows.iter().zip(heights) {
            let Some(id) = pane
                .id
                .strip_prefix('%')
                .and_then(|id| id.parse::<u64>().ok())
            else {
                return Vec::new();
            };
            row_layouts.push(format!("{cols}x{height},{x},{y},{id}"));
            pane_order.push(pane.id.as_str());
            y += height + 1;
        }
        column_layouts.push(layout_group(
            cols,
            window.rows,
            x,
            &row_layouts,
            SplitAxis::Vertical,
        ));
        x += cols + 1;
    }
    let tree = layout_group(
        window.cols,
        window.rows,
        0,
        &column_layouts,
        SplitAxis::Horizontal,
    );
    let checksum = tree.bytes().fold(0_u16, |checksum, byte| {
        checksum.rotate_right(1).wrapping_add(u16::from(byte))
    });
    let mut commands = vec![format!(
        "select-layout -t {} {}",
        q(&window.id),
        q(&format!("{checksum:04x},{tree}"))
    )];
    // Classic select-layout assigns cells in native pane order, ignoring the
    // IDs in its string. Restore identities if server order differs from geometry.
    let mut assigned: Vec<_> = window.panes.iter().map(|pane| pane.id.as_str()).collect();
    for (index, desired) in pane_order.iter().enumerate() {
        if assigned[index] == *desired {
            continue;
        }
        let Some(source) = assigned.iter().position(|id| id == desired) else {
            return Vec::new();
        };
        commands.push(format!(
            "swap-pane -d -s {} -t {}",
            q(assigned[index]),
            q(desired)
        ));
        assigned.swap(index, source);
    }
    if commands.len() > 1 {
        commands.push(format!("select-pane -t {}", q(&window.active_pane)));
    }
    commands
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::parse_layout;

    fn window(layout: &str) -> WindowInfo {
        WindowInfo {
            id: "@0".into(),
            cols: 80,
            rows: 24,
            active_pane: "%3".into(),
            panes: parse_layout(layout).unwrap(),
            ..Default::default()
        }
    }

    #[test]
    fn mixed_columns_fit_the_window_and_keep_original_rows() {
        // tmux 3.8: down once, right twice, then select-layout -E.
        let mut window =
            window("0000,80x24,0,0{20x24,0,0[20x12,0,0,0,20x11,0,13,1],40x24,21,0,2,39x24,62,0,3}");
        assert_eq!(column_layout_width(&window), Some(101));
        assert!(!is_column_layout(&window));
        let commands = equalize_commands(&window);
        assert_eq!(commands.len(), 1);
        let args = shell_words::split(&commands[0]).unwrap();
        window.panes = parse_layout(&args[3]).unwrap();
        assert!(is_equal_column_layout(&window));
        let groups = pane_columns(&window);
        assert_eq!(groups.values().map(Vec::len).collect::<Vec<_>>(), [2, 1, 1]);
        assert_eq!(
            window
                .panes
                .iter()
                .map(|pane| pane.cols)
                .collect::<Vec<_>>(),
            [26; 4]
        );
        assert_eq!(
            window
                .panes
                .iter()
                .map(|pane| pane.rows)
                .collect::<Vec<_>>(),
            [12, 11, 24, 24]
        );
        assert_eq!(
            window
                .panes
                .iter()
                .map(|pane| pane.id.as_str())
                .collect::<Vec<_>>(),
            ["%0", "%1", "%2", "%3"]
        );
    }

    #[test]
    fn removing_appended_columns_restores_original_dimensions() {
        let mut window =
            window("0000,80x24,0,0{26x24,0,0[26x12,0,0,0,26x11,0,13,1],26x24,27,0,2,26x24,54,0,3}");
        window.panes.retain(|pane| pane.x == 0);
        let commands = equalize_commands(&window);
        let args = shell_words::split(&commands[0]).unwrap();
        window.panes = parse_layout(&args[3]).unwrap();
        assert!(is_equal_column_layout(&window));
        assert_eq!(window.panes[0].cols, 80);
        assert_eq!(window.panes[1].cols, 80);
        assert_eq!((window.panes[0].rows, window.panes[1].rows), (12, 11));
    }

    #[test]
    fn different_native_order_restores_pane_identities_and_focus() {
        let mut window = window("0000,80x24,0,0{40x24,0,0[40x12,0,0,0,40x11,0,13,1],39x24,41,0,3}");
        window.panes.swap(0, 2);
        let commands = equalize_commands(&window);
        assert_eq!(commands.len(), 3);
        assert_eq!(commands[1], "swap-pane -d -s '%3' -t '%0'");
        assert_eq!(commands[2], "select-pane -t '%3'");
    }

    #[test]
    fn insufficient_space_does_not_submit_zero_sized_panes() {
        assert_eq!(equal_sizes(8, 3), Some(vec![2, 2, 2]));
        assert_eq!(equal_sizes(9, 3), Some(vec![3, 2, 2]));
        assert_eq!(equal_sizes(10, 3), Some(vec![3, 3, 2]));
        assert!(equal_sizes(4, 3).is_none());
        assert!(equal_sizes(24, 0).is_none());
        let mut window = window("0000,80x24,0,0[80x12,0,0,0,80x11,0,13,1]");
        window.rows = 2;
        assert!(!can_equalize(&window));
        assert!(equalize_commands(&window).is_empty());
    }
}
