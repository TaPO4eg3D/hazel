use std::time::Duration;

use gpui::{
    Animation, App, ElementId, Entity, InteractiveElement, IntoElement, ParentElement as _,
    RenderOnce, StatefulInteractiveElement, Styled, Window, div, ease_in_out,
    prelude::FluentBuilder, px, rgb,
};
use gpui_component::{
    ActiveTheme, Icon, Sizable, Size, StyledExt,
    label::Label,
    slider::{Slider, SliderState},
};

use crate::{
    ConnectionManger,
    assets::IconName,
    components::{
        animation::HoverAnimationExt,
        collapsable_card::{CollapsableCard, CollapsableCardState},
        context_popover::ContextPopover as _,
        streaming_state::{StreamingState, VoiceChannel, VoiceChannelMember},
    },
};

#[derive(IntoElement)]
struct VolumeSlider {
    volume: Entity<SliderState>,
}

impl VolumeSlider {
    fn new(volume: Entity<SliderState>) -> Self {
        Self { volume }
    }
}

impl RenderOnce for VolumeSlider {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .p_2()
            .v_flex()
            .child(
                div().flex().child(Label::new("Volume").text_xs()).child(
                    Label::new(format!("{}%", self.volume.read(cx).value()))
                        .text_xs()
                        .ml_auto(),
                ),
            )
            .child(Slider::new(&self.volume))
    }
}

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

#[derive(IntoElement)]
struct VoiceMemberComponent {
    streaming_state: Entity<StreamingState>,
    member: VoiceChannelMember,
}

impl RenderOnce for VoiceMemberComponent {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let current_user = ConnectionManger::get_user_id(cx);

        let (is_capture_disabled, is_playback_disabled) = {
            let state = self.streaming_state.read(cx);

            (!state.is_capture_enabled, !state.is_playback_enabled)
        };

        let secondary = cx.theme().secondary;

        let is_me = current_user.is_some_and(|id| self.member.id == id);

        let is_mic_off = if is_me {
            is_capture_disabled
        } else {
            self.member.is_mic_off
        };

        let is_sound_off = if is_me {
            is_playback_disabled
        } else {
            self.member.is_sound_off
        };

        // `is_talking` is special and managed internally
        let is_talking = self.member.is_talking && (!is_mic_off && !is_sound_off);

        let is_selected = window.use_keyed_state(
            format!("voice-member-{}-selected", self.member.id.value),
            cx,
            |_, _| false,
        );

        div()
            .id(ElementId::Integer(self.member.id.value as u64))
            .child(
                div()
                    .rounded_lg()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .py_2()
                            .px_3()
                            .child(Icon::new(IconName::User).mr_2().with_size(Size::Medium))
                            .child(Label::new(self.member.name.clone()).mt(px(0.5)))
                            // Status icons
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .ml_auto()
                                    .when(is_mic_off, |this| {
                                        this.child(
                                            Icon::new(IconName::MicOff)
                                                .text_color(cx.theme().danger)
                                                .with_size(Size::XSmall),
                                        )
                                    })
                                    .when(is_sound_off, |this| {
                                        this.child(
                                            Icon::new(IconName::HeadphoneOff)
                                                .text_color(cx.theme().danger)
                                                .with_size(Size::XSmall),
                                        )
                                    })
                                    .when(is_talking, |this| {
                                        this.child(div().size_2().rounded_full().bg(rgb(0x00C950)))
                                    }),
                            ),
                    )
                    .with_hover_animation(
                        "hover-bg",
                        Animation::new(Duration::from_millis(200)).with_easing(ease_in_out),
                        move |this, delta| this.bg(secondary.opacity(delta)),
                    )
                    .when(*is_selected.read(cx), |this| this.bg(secondary)),
            )
            .context_menu(format!("context-voice-{}", self.member.id.value), {
                let output_volume = self.member.output_volume.clone();

                move |this, _, _cx| {
                    this.v_flex()
                        .w_48()
                        .px_2()
                        .child(VolumeSlider::new(output_volume.clone()))
                }
            })
            .on_toggle(move |&opened, _, cx| {
                is_selected.update(cx, |this, _| {
                    *this = opened;
                })
            })
    }
}

impl VoiceChannelsComponent {
    fn render_channels(
        &self,
        channels: Vec<VoiceChannel>,
        window: &mut Window,
        cx: &mut App,
    ) -> impl Iterator<Item = impl IntoElement> {
        let muted = cx.theme().muted;
        let secondary = cx.theme().secondary;

        channels.into_iter().map(move |channel| {
            let channel_id = channel.id;
            let is_active = channel.is_active;

            let members = channel
                .members
                .into_iter()
                .map(|member| VoiceMemberComponent {
                    streaming_state: self.streaming_state.clone(),
                    member,
                });

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
                    .children(self.render_channels(channels, window, cx)),
            )
    }
}
