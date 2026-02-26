use std::time::Duration;

use gpui::{
    Animation, App, Div, ElementId, Entity, InteractiveElement, IntoElement, ParentElement as _,
    RenderOnce, Stateful, StatefulInteractiveElement, Styled, Window, div, ease_in_out,
    prelude::FluentBuilder, px, rgb,
};
use gpui_component::{ActiveTheme, Icon, Sizable, Size, StyledExt, label::Label};

use crate::{
    ConnectionManger,
    assets::IconName,
    components::{
        animation::HoverAnimationExt,
        collapsable_card::{CollapsableCard, CollapsableCardState},
        streaming_state::{StreamingState, VoiceChannel, VoiceChannelMember},
    },
};

#[derive(IntoElement)]
pub struct VoiceChannelsComponent {
    card_state: Entity<CollapsableCardState>,
    streaming_state: Entity<StreamingState>,
}

impl VoiceChannelsComponent {
    pub fn new(
        card_state: &Entity<CollapsableCardState>,
        streaming_state: &Entity<StreamingState>,
    ) -> Self {
        Self {
            card_state: card_state.clone(),
            streaming_state: streaming_state.clone(),
        }
    }
}

impl VoiceChannelsComponent {
    fn render_memebers(
        &self,
        members: &[VoiceChannelMember],
        cx: &mut App,
    ) -> impl Iterator<Item = Stateful<Div>> {
        let current_user = ConnectionManger::get_user_id(cx);

        let (is_mic_off, is_sound_off) = {
            let state = self.streaming_state.read(cx);

            (!state.is_capture_enabled, !state.is_playback_enabled)
        };

        let secondary = cx.theme().secondary;
        members.iter().map(move |member| {
            let is_me = current_user.is_some_and(|id| member.id == id);

            div().id(ElementId::Integer(member.id.value as u64)).child(
                div()
                    .rounded_lg()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .py_2()
                            .px_3()
                            .child(Icon::new(IconName::User).mr_2().with_size(Size::Medium))
                            .child(Label::new(member.name.clone()).mt(px(0.5)))
                            // Status icons
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .ml_auto()
                                    .when(member.is_mic_off || is_me && is_mic_off, |this| {
                                        this.child(
                                            Icon::new(IconName::MicOff)
                                                .text_color(cx.theme().danger)
                                                .with_size(Size::XSmall),
                                        )
                                    })
                                    .when(member.is_sound_off || is_me && is_sound_off, |this| {
                                        this.child(
                                            Icon::new(IconName::HeadphoneOff)
                                                .text_color(cx.theme().danger)
                                                .with_size(Size::XSmall),
                                        )
                                    })
                                    // `is_talking` is special since it's managed internally
                                    .when(member.is_talking, |this| {
                                        this.child(div().size_2().rounded_full().bg(rgb(0x00C950)))
                                    }),
                            ),
                    )
                    .with_hover_animation(
                        "hover-bg",
                        Animation::new(Duration::from_millis(200)).with_easing(ease_in_out),
                        move |this, delta| this.bg(secondary.opacity(delta)),
                    ),
            )
        })
    }

    fn render_channels(
        &self,
        channels: &[VoiceChannel],
        window: &mut Window,
        cx: &mut App,
    ) -> impl Iterator<Item = Stateful<Div>> {
        let muted = cx.theme().muted;
        let secondary = cx.theme().secondary;

        channels.iter().map(move |channel| {
            let channel_id = channel.id;
            let is_active = channel.is_active;

            let members = self.render_memebers(&channel.members, cx);

            div()
                .id(ElementId::Integer(channel.id.value as u64))
                .v_flex()
                // Clickable channel title
                .child(
                    div()
                        .id("channel-title")
                        .child(
                            div()
                                .rounded_lg()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .py_2()
                                        .px_3()
                                        .child(
                                            Icon::new(IconName::VolumeFull)
                                                .mr_2()
                                                .with_size(Size::Medium),
                                        )
                                        .child(Label::new(channel.name.clone()).mt(px(0.5))),
                                )
                                .with_hover_animation(
                                    "hover-bg",
                                    Animation::new(Duration::from_millis(200))
                                        .with_easing(ease_in_out),
                                    move |this, delta| {
                                        if is_active {
                                            this.bg(muted.opacity(1. - delta.min(0.2)))
                                        } else {
                                            this.bg(secondary.opacity(delta))
                                        }
                                    },
                                ),
                        )
                        .on_click(window.listener_for(
                            &self.streaming_state,
                            move |state, _, window, cx| {
                                state.join_voice_channel(&channel_id, window, cx);
                            },
                        )),
                )
                .child(div().id("members").mt_1().ml_4().children(members))
        })
    }
}

impl RenderOnce for VoiceChannelsComponent {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let channels = self.streaming_state.read(cx).voice_channels.clone();

        CollapsableCard::new("voice-channels", self.card_state.clone())
            .title("Voice channels")
            .content(
                div()
                    .v_flex()
                    .children(self.render_channels(&channels, window, cx)),
            )
    }
}
