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
    assets::IconName,
    components::{
        animation::HoverAnimationExt,
        collapsable_card::{CollapsableCard, CollapsableCardState},
        connection_state::{ServerConnectionState, VoiceChannel, VoiceChannelMember},
        context_popover::{ContextPopover as _, CtxPopoverButton},
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
    connection_state: Entity<ServerConnectionState>,
}

impl VoiceChannelsComponent {
    pub fn new(
        card_state: &Entity<CollapsableCardState>,
        connection_state: &Entity<ServerConnectionState>,
    ) -> Self {
        Self {
            card_state: card_state.clone(),
            connection_state: connection_state.clone(),
        }
    }
}

#[derive(IntoElement)]
struct VoiceMemberComponent {
    connection_state: Entity<ServerConnectionState>,
    member: VoiceChannelMember,
}

impl RenderOnce for VoiceMemberComponent {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let connected_user = self.connection_state.read(cx).user_id;

        let (is_capture_disabled, is_playback_disabled, has_preview) = {
            let state = self.connection_state.read(cx);

            (
                !state.is_capture_enabled,
                !state.is_playback_enabled,
                cfg_select! {
                    target_os = "linux" => state.preview_frame.is_some(),
                    _ => false,
                },
            )
        };

        let secondary = cx.theme().secondary;

        let is_me = self.member.id == connected_user;

        let is_mic_off = if is_me {
            is_capture_disabled
        } else {
            self.member.state.is_mic_off
        };

        let is_sound_off = if is_me {
            is_playback_disabled
        } else {
            self.member.state.is_sound_off
        };

        let is_streaming = if is_me {
            has_preview
        } else {
            self.member.state.is_streaming
        };

        // `is_talking` is special and managed internally
        let is_talking = self.member.is_talking && (!is_mic_off && !is_sound_off);

        let is_selected = window.use_keyed_state(
            ElementId::named_usize("voice-member", self.member.id.value as usize),
            cx,
            |_, _| false,
        );

        let element = div()
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
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(Label::new(self.member.name.clone()).mt(px(0.5)))
                                    .when(is_streaming, |this| {
                                        this.child(
                                            Icon::new(IconName::ScreenShare)
                                                .text_color(cx.theme().info)
                                                .with_size(Size::XSmall),
                                        )
                                    }),
                            )
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
            );

        if !is_me {
            element
                .context_popover(
                    ElementId::named_usize("context-voice", self.member.id.value as usize),
                    {
                        let user_id = self.member.id;
                        let output_volume = self.member.output_volume.clone();

                        move |this, window, _cx| {
                            this.v_flex()
                                .w_48()
                                .p_2()
                                .gap_2()
                                .when(self.member.state.is_streaming, |this| {
                                    this.child(
                                        CtxPopoverButton::new("watch-stream")
                                            .label("Watch stream")
                                            .icon(IconName::ScreenShare)
                                            .on_click(window.listener_for(
                                                &self.connection_state,
                                                move |this, _, window, cx| {
                                                    #[cfg(target_os = "linux")]
                                                    this.join_screencast(user_id, window, cx);
                                                },
                                            )),
                                    )
                                })
                                .child(VolumeSlider::new(output_volume.clone()))
                        }
                    },
                )
                .on_toggle(move |&opened, _, cx| {
                    is_selected.update(cx, |this, _| {
                        *this = opened;
                    })
                })
                .into_any_element()
        } else {
            element.into_any_element()
        }
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
                    connection_state: self.connection_state.clone(),
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
                            &self.connection_state,
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
        let channels = self.connection_state.read(cx).voice_channels.clone();

        CollapsableCard::new("voice-channels", self.card_state.clone())
            .title("Voice channels")
            .content(
                div()
                    .v_flex()
                    .children(self.render_channels(channels, window, cx)),
            )
    }
}
