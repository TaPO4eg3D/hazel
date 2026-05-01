use gpui::{IntoElement, ParentElement, RenderOnce, Styled, div};
use gpui_component::{
    ActiveTheme, Colorize, Icon, Sizable, Size, StyledExt as _,
    button::{Button, ButtonVariants},
    label::Label,
};

use crate::assets::IconName;

#[derive(IntoElement)]
pub struct CallRoom {}

impl CallRoom {
    pub fn new() -> Self {
        Self {}
    }
}

impl RenderOnce for CallRoom {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl gpui::IntoElement {
        div()
            .p_3()
            .v_flex()
            .gap_4()
            .size_full()
            .child(ScreenSpace::new())
            .child(ControlPanel::new())
    }
}

#[derive(IntoElement)]
struct ScreenSpace {}

impl ScreenSpace {
    pub fn new() -> Self {
        Self {}
    }
}

impl RenderOnce for ScreenSpace {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl gpui::IntoElement {
        div()
            .v_flex()
            .items_center()
            .justify_center()
            .rounded_xl()
            .border_1()
            .size_full()
            .border_color(cx.theme().secondary)
            .child(
                div()
                    .flex()
                    .justify_center()
                    .items_center()
                    .size_16()
                    .rounded_full()
                    .border_1()
                    .border_color(cx.theme().muted_foreground)
                    .child(
                        Icon::new(IconName::ScreenShare)
                            .with_size(Size::Large)
                            .text_color(cx.theme().muted_foreground),
                    )
                    .bg(cx.theme().secondary),
            )
            .child(
                Label::new("Stream is not selected")
                    .mt_4()
                    .text_base()
                    .font_semibold(),
            )
            .child(
                Label::new(
                    "Only one stream can be selected at a time. \
                    Right click on a member and select \"Watch stream\" option",
                )
                .mt_2()
                .max_w_112()
                .text_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground),
            )
    }
}

#[derive(IntoElement)]
struct ControlPanel {}

impl ControlPanel {
    pub fn new() -> Self {
        Self {}
    }
}

impl RenderOnce for ControlPanel {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        div()
            .p_4()
            .flex()
            .items_center()
            .rounded_xl()
            .border_1()
            .w_full()
            .border_color(cx.theme().secondary)
            .child(
                Button::new("start-streaming")
                    .icon(IconName::ScreenShare)
                    .label("Share screen")
                    .max_w_64()
                    .w_full()
                    .primary(),
            )
            .child(
                Label::new("Screen share is currently off")
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .ml_auto(),
            )
    }
}
