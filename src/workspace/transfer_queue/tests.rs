use super::{CHROME_BAR_HEIGHT, PANEL_MARGIN, PanelDrag, TransferQueue};
use gpui_kit::{point, px, size};

#[test]
fn auto_collapse_respects_hover_drag_and_newer_interactions() {
    let mut queue = TransferQueue::default();
    queue.panel_open = true;
    queue.panel_revision = 3;
    assert!(!queue.collapse(2));
    queue.hovered = true;
    assert!(!queue.collapse(3));
    queue.hovered = false;
    queue.drag = Some(PanelDrag {
        pointer: point(px(0.), px(0.)),
        offset: queue.offset,
    });
    assert!(!queue.collapse(3));
    queue.drag = None;
    assert!(queue.collapse(3));
    assert!(!queue.panel_open);
    assert!(!queue.collapse(3));
}

#[test]
fn collapsing_docks_right_and_preserves_expanded_position() {
    let viewport = size(px(1320.), px(840.));
    let mut queue = TransferQueue::default();
    queue.panel_open = true;
    queue.offset = point(px(260.), px(140.));
    let expanded = queue.panel_bounds(viewport);
    queue.panel_open = false;
    let compact = queue.panel_bounds(viewport);
    assert_eq!(compact.right(), viewport.width - px(PANEL_MARGIN));
    assert_eq!(compact.bottom(), expanded.bottom());
    queue.panel_open = true;
    assert_eq!(queue.panel_bounds(viewport), expanded);
}

#[test]
fn panel_stays_inside_smaller_viewport_after_dragging() {
    let mut queue = TransferQueue::default();
    queue.panel_open = true;
    queue.offset = point(px(1200.), px(800.));
    for viewport in [size(px(860.), px(520.)), size(px(320.), px(240.))] {
        let bounds = queue.panel_bounds(viewport);
        assert!(bounds.left() >= px(PANEL_MARGIN));
        assert!(bounds.top() >= px(CHROME_BAR_HEIGHT + PANEL_MARGIN));
        assert!(bounds.right() <= viewport.width - px(PANEL_MARGIN));
        assert!(bounds.bottom() <= viewport.height - px(32.));
    }
}
