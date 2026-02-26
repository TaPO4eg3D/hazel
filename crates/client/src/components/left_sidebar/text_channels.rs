use std::time::Duration;

use gpui::{
    Animation, ElementId, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, Styled as _, div, ease_in_out, px,
};
use gpui_component::{ActiveTheme as _, Icon, Sizable as _, Size, StyledExt as _, label::Label};

use crate::{
    assets::IconName,
    components::{
        animation::HoverAnimationExt as _,
        chat_state::ChatState,
        collapsable_card::{CollapsableCard, CollapsableCardState},
    },
};

#[derive(IntoElement)]
pub struct TextChannelsComponent {
    card_state: Entity<CollapsableCardState>,
    chat_state: Entity<ChatState>,
}

impl TextChannelsComponent {
    pub fn new(card_state: &Entity<CollapsableCardState>, chat_state: &Entity<ChatState>) -> Self {
        Self {
            card_state: card_state.clone(),
            chat_state: chat_state.clone(),
        }
    }
}

impl RenderOnce for TextChannelsComponent {
    fn render(self, _window: &mut gpui::Window, cx: &mut gpui::App) -> impl gpui::IntoElement {
        let state = self.chat_state.read(cx);
        let secondary = cx.theme().secondary;

        let channels = state.text_channels.iter().map(|channel| {
            let is_active = channel.is_active;
            let muted = cx.theme().muted;

            div().id(ElementId::Integer(channel.id)).child(
                div()
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
                        "hover-bg",
                        Animation::new(Duration::from_millis(200)).with_easing(ease_in_out),
                        move |this, delta| {
                            if is_active {
                                this.bg(muted.opacity(1. - delta.min(0.2)))
                            } else {
                                this.bg(secondary.opacity(delta))
                            }
                        },
                    ),
            )
        });

        CollapsableCard::new("text-channels", self.card_state)
            .title("Text channels")
            .content(div().v_flex().children(channels))
    }
}
