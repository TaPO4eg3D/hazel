use std::{cell::RefCell, time::Duration};

use gpui::{
    Animation, AnimationExt as _, AppContext, ElementId, Entity, InteractiveElement, IntoElement, ParentElement as _, RenderOnce, StatefulInteractiveElement, Styled, bounce, div, ease_in_out, linear, prelude::FluentBuilder, px
};
use gpui_component::{
    ActiveTheme, Icon, Sizable, Size, StyledExt,
    button::{Button, ButtonVariants},
    label::Label,
};

use crate::{assets::IconName, components::{animation::HoverAnimationExt, chat_state::ChatState}};

#[derive(IntoElement)]
pub struct TextChannelsComponent {
    chat_state: Entity<ChatState>,
}

impl TextChannelsComponent {
    pub fn new(chat_state: &Entity<ChatState>) -> Self {
        Self {
            chat_state: chat_state.clone(),
        }
    }
}

impl RenderOnce for TextChannelsComponent {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl gpui::IntoElement {
        let state = self.chat_state.read(cx);

        let secondary = cx.theme().secondary;
        let channels = state.text_channels.iter().map(|channel| {
            div()
                .id(ElementId::Integer(channel.id))
                .when(channel.is_active, |this| this.bg(cx.theme().muted))
                .rounded_lg()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .py_2()
                        .px_3()
                        .child(Icon::new(IconName::Hash).mr_2().with_size(Size::Medium))
                        .child(Label::new(channel.name.clone()).mt(px(0.5))),
                )
                .with_hover_animation(
                    format!("{}-hover-bg-opacity", channel.id),
                    Animation::new(Duration::from_millis(200))
                        .with_easing(ease_in_out),
                    move |this, delta| {
                        this.bg(
                            secondary.opacity(delta)
                        )
                    }
                )
        });

        div()
            .id("text-channels")
            .p_3()
            .w_full()
            .v_flex()
            .child(
                div()
                    .mb_2()
                    .w_full()
                    .flex()
                    .items_center()
                    .child(Label::new("Text channels").text_sm().font_semibold())
                    .child(
                        Button::new("collapse")
                            .ml_auto()
                            .cursor_pointer()
                            .icon(IconName::ChevronDown)
                            .ghost(),
                    ),
            )
            .child(div().v_flex().children(channels))
    }
}
