use crate::appearance::Palette;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::{prelude::*, *};

pub const CONTROL_HEIGHT: f32 = 28.;
pub const ROW_HEIGHT: f32 = 30.;
pub const TOOLBAR_HEIGHT: f32 = 36.;

pub fn empty_state(icon: IconName, message: impl Into<SharedString>, palette: Palette) -> Div {
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

pub fn section_heading(label: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(palette.muted))
        .child(label.into())
}

pub fn notice(icon: IconName, message: impl Into<SharedString>, color: u32) -> Div {
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

/// Place transient feedback inside a relative content area without resizing its siblings.
pub fn content_notice(
    id: impl Into<ElementId>,
    icon: IconName,
    message: impl Into<SharedString>,
    color: u32,
    palette: Palette,
) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .bottom_0()
        .left_0()
        .w_full()
        .max_h_full()
        .overflow_y_scroll()
        .px_3()
        .py_2()
        .bg(rgb(palette.panel))
        .border_t_1()
        .border_color(rgb(palette.border))
        .child(notice(icon, message, color))
}

use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
pub fn button(id: impl Into<ElementId>, icon: IconName, label: impl Into<SharedString>) -> Button {
    Button::new(id)
        .ghost()
        .small()
        .h(px(CONTROL_HEIGHT))
        .rounded(px(4.))
        .text_size(px(12.))
        .line_height(relative(1.))
        .icon(icon)
        .label(label)
}
pub fn icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
) -> Button {
    let label = label.into();
    Button::new(id)
        .ghost()
        .small()
        .size(px(CONTROL_HEIGHT))
        .rounded(px(4.))
        .text_size(px(12.))
        .line_height(relative(1.))
        .icon(icon)
        .accessibility_label(label.clone())
        .tooltip(label)
}

pub fn workspace_dialog(
    dialog: gpui_kit::component::dialog::Dialog,
    cx: &App,
) -> gpui_kit::component::dialog::Dialog {
    use gpui_kit::component::ActiveTheme;
    dialog
        .bg(cx.theme().popover)
        .border_color(cx.theme().border)
        .text_color(cx.theme().popover_foreground)
}
