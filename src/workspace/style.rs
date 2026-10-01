use super::*;

pub(super) const CONTROL_HEIGHT: f32 = 28.;
pub(super) const ROW_HEIGHT: f32 = 30.;
pub(super) const TOOLBAR_HEIGHT: f32 = 36.;

pub(super) fn empty_state(
    icon: IconName,
    message: impl Into<SharedString>,
    palette: Palette,
) -> Div {
    div()
        .size_full()
        .min_w_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .p_4()
        .text_size(px(13.))
        .text_color(rgb(palette.muted))
        .child(Icon::new(icon).size(px(24.)).flex_shrink_0())
        .child(div().max_w_full().text_center().child(message.into()))
}

pub(super) fn section_heading(label: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(palette.muted))
        .child(label.into())
}

pub(super) fn notice(icon: IconName, message: impl Into<SharedString>, color: u32) -> Div {
    div()
        .min_w_0()
        .flex()
        .items_start()
        .gap_2()
        .text_size(px(13.))
        .text_color(rgb(color))
        .child(Icon::new(icon).size(px(16.)).mt(px(2.)).flex_shrink_0())
        .child(div().min_w_0().flex_1().child(message.into()))
}
