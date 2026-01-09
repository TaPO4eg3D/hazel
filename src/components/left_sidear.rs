use gpui::{Entity, IntoElement, ParentElement, RenderOnce, SharedString, StyleRefinement, Styled, div, prelude::FluentBuilder, px, rgb};
use gpui_component::{Icon, StyledExt};

use crate::assets::IconName;

#[derive(Clone)]
pub struct TextChannel {
    pub name: SharedString,

    pub is_muted: bool,
    pub has_unread: bool,
}

#[derive(IntoElement)]
pub struct TextChannelsComponent {
    channels: Vec<TextChannel>,
    is_collapsed: bool,

    style: StyleRefinement,
}

impl TextChannelsComponent {
    pub fn new(channels: Vec<TextChannel>) -> Self {
        Self {
            channels,
            is_collapsed: false,
            style: StyleRefinement::default(),
        }
    }

    pub fn collapsed(mut self, value: bool) -> Self {
        self.is_collapsed = value;
        self
    }
}

impl Styled for TextChannelsComponent {
    fn style(&mut self) ->  &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for TextChannelsComponent {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl gpui::IntoElement {
        let root = div()
            .refine_style(&self.style)
            .child("TEXT CHANNELS");

        let channels = self.channels.iter().map(|channel| {
            div()
                .bg(rgb(0x0F111A))
                .items_center()
                .rounded_lg()
                .flex()
                .child(
                    Icon::new(IconName::Hash)
                        .ml_3()
                )
                .child(
                    div()
                        .py_3()
                        .ml_3()
                        .font_normal()
                        .text_size(px(14.))
                        .child(channel.name.clone())
                )
                .child(
                    Icon::new(IconName::MessageCircleOff)
                        .ml_auto()
                        .mr_3()
                )
                .when(channel.has_unread, |this| {
                    this.relative().child(
                        div()
                            .bg(rgb(0xFF8800))
                            .absolute()
                            .top_neg_1()
                            .right_0()
                            .w_2()
                            .h_2()
                            .rounded_xl()
                    )
                })
        });

        root
            .child(
                div()
                    .mt_4()
                    .v_flex()
                    .gap_2()
                    .children(channels)
            )
    }
}
