use std::{cell::RefCell, time::Duration};

use gpui::{
    Animation, AnimationExt as _, App, AppContext, ElementId, Entity, InteractiveElement, IntoElement, ParentElement as _, RenderOnce, StatefulInteractiveElement, Styled, Window, bounce, div, ease_in_out, linear, prelude::FluentBuilder, px, red, relative, rgb, white
};
use gpui_component::{
    ActiveTheme, Anchor, Icon, Selectable, Sizable, Size, StyledExt, button::{Button, ButtonVariants}, divider::Divider, label::Label, popover::Popover
};

use crate::{
    assets::IconName,
    components::{
        animation::HoverAnimationExt, chat_state::ChatState, streaming_state::StreamingState,
    },
};

type EventCallback<T> = Box<dyn Fn(&T, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct TextChannelsComponent {
    chat_state: Entity<ChatState>,

    is_collapsed: bool,
    on_toggle_click: Option<EventCallback<bool>>,
}

impl TextChannelsComponent {
    pub fn new(chat_state: &Entity<ChatState>) -> Self {
        Self {
            chat_state: chat_state.clone(),

            is_collapsed: false,
            on_toggle_click: None,
        }
    }

    pub fn is_collapsed(mut self, value: bool) -> Self {
        self.is_collapsed = value;

        self
    }

    pub fn on_toggle_click(
        mut self,
        on_toggle_click: impl Fn(&bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle_click = Some(Box::new(on_toggle_click));

        self
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
                            .icon({
                                if self.is_collapsed {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronDown
                                }
                            })
                            .ghost()
                            .when_some(self.on_toggle_click, |this, on_toggle_click| {
                                this.on_click(move |_, window, cx| {
                                    on_toggle_click(&!self.is_collapsed, window, cx);
                                })
                            }),
                    ),
            )
            .when(!self.is_collapsed, |this| {
                this.child(div().v_flex().children(channels))
            })
    }
}

#[derive(IntoElement)]
pub struct VoiceChannelsComponent {
    streaming_state: Entity<StreamingState>,

    is_collapsed: bool,
    on_toggle_click: Option<EventCallback<bool>>,
}

impl VoiceChannelsComponent {
    pub fn new(streaming_state: &Entity<StreamingState>) -> Self {
        Self {
            streaming_state: streaming_state.clone(),

            is_collapsed: false,
            on_toggle_click: None,
        }
    }

    pub fn is_collapsed(mut self, value: bool) -> Self {
        self.is_collapsed = value;

        self
    }

    pub fn on_toggle_click(
        mut self,
        on_toggle_click: impl Fn(&bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle_click = Some(Box::new(on_toggle_click));

        self
    }
}

impl RenderOnce for VoiceChannelsComponent {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let voice_channels = { self.streaming_state.read(cx).voice_channels.clone() };

        let secondary = cx.theme().secondary;

        let channels = voice_channels.iter().map(|channel| {
            let muted = cx.theme().muted;

            let members = channel.members.iter().map(|member| {
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
                                .child(div().flex().gap_1().ml_auto().when(
                                    member.is_talking,
                                    |this| {
                                        this.child(div().size_2().rounded_full().bg(rgb(0x00C950)))
                                    },
                                )),
                        )
                        .with_hover_animation(
                            "hover-bg",
                            Animation::new(Duration::from_millis(200)).with_easing(ease_in_out),
                            move |this, delta| this.bg(secondary.opacity(delta)),
                        ),
                )
            });

            let channel_id = channel.id;
            let is_active = channel.is_active;

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
                // Members of the channel
                .child(div().id("members").mt_1().ml_4().children(members))
        });

        div()
            .id("voice-channels")
            .p_3()
            .w_full()
            .v_flex()
            .child(
                div()
                    .mb_2()
                    .w_full()
                    .flex()
                    .items_center()
                    .child(Label::new("Voice channels").text_sm().font_semibold())
                    .child(
                        Button::new("collapse")
                            .ml_auto()
                            .cursor_pointer()
                            .icon({
                                if self.is_collapsed {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronDown
                                }
                            })
                            .ghost()
                            .when_some(self.on_toggle_click, |this, on_toggle_click| {
                                this.on_click(move |_, window, cx| {
                                    on_toggle_click(&!self.is_collapsed, window, cx);
                                })
                            }),
                    ),
            )
            .when(!self.is_collapsed, |this| {
                this.child(div().v_flex().children(channels))
            })
    }
}

#[derive(IntoElement)]
pub struct ControlPanel {
    streaming_state: Entity<StreamingState>,
}

impl ControlPanel {
    pub fn new(state: &Entity<StreamingState>) -> Self {
        Self {
            streaming_state: state.clone(),
        }
    }
}

impl RenderOnce for ControlPanel {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .id("control-panel")
            .p_3()
            .v_flex()
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .v_flex()
                            .child(
                                Label::new("VOICE CONNECTED")
                                    .text_xs()
                                    .text_color(rgb(0x00C950))
                                    .font_bold(),
                            )
                            .child(Label::new("Gaming").text_sm()),
                    )
                    .child(
                        Button::new("disconnect")
                            .ml_auto()
                            .cursor_pointer()
                            .icon(IconName::PhoneOff)
                            .ghost(),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .mt_2()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .child(
                                Button::new("capture-toggle")
                                    .cursor_pointer()
                                    .outline()
                                    .border_r_0()
                                    .rounded_r_none()
                                    .icon(IconName::Mic)
                                    .flex_grow(),
                            )
                            .child(
                                Popover::new("capture-popover")
                                    .max_w(px(600.))
                                    .anchor(Anchor::BottomCenter)
                                    .trigger(
                                        Button::new("capture-select")
                                            .outline()
                                            .rounded_l_none()
                                            .icon(IconName::ChevronUp)
                                    )
                                    .child("This is a Popover on the Top Center."),
                            )
                            .flex_grow(),
                    )
                    .child(
                        PlaybackControl::new(&self.streaming_state)
                    ),
            )
    }
}

#[derive(IntoElement)]
struct PlaybackControl {
    streaming_state: Entity<StreamingState>,
}

impl PlaybackControl {
    fn new(state: &Entity<StreamingState>) -> Self {
        Self {
            streaming_state: state.clone(),
        }
    }
}

impl RenderOnce for PlaybackControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let output_devices = (0..5).map(|i| {
            div()
                .id(ElementId::Integer(i))
                .w_full()
                .rounded_md()
                .hover(|this| {
                    this.bg(cx.theme().secondary)
                })
                .p_2()
                .flex()
                .items_center()
                .child(
                    div()
                        .pl_1()
                        .child(
                            div()
                                .size_2()
                                .rounded_full()
                                .flex_none()
                                .when(i == 0, |this| {
                                    this.bg(white())
                                })
                        )
                )
                .child(
                    // An additional container to force the label to wrap
                    div()
                        .pl_4()
                        .w_full()
                        .child(
                            Label::new("Long name of an output device ( quite long indeed )")
                                .text_sm()
                        )
                )
        });

        div()
            .flex()
            .child(
                Button::new("playback-toggle")
                    .cursor_pointer()
                    .outline()
                    .border_r_0()
                    .rounded_r_none()
                    .icon(IconName::Headphones)
                    .flex_grow(),
            )
            .child(
                Popover::new("playback-popover")
                    .w_64()
                    .anchor(Anchor::BottomCenter)
                    .trigger(
                        Button::new("playback-select")
                            .outline()
                            .rounded_l_none()
                            .icon(IconName::ChevronUp),
                    ).child(
                        div()
                            .v_flex()
                            .w_full()
                            .child(
                                Label::new("Output Control")
                                    .text_sm()
                            )
                            .child(Divider::horizontal().my_2())
                            .child(
                                div()
                                    .id("output-devices")
                                    .v_flex()
                                    .gap_1()
                                    .w_full()
                                    .children(output_devices)
                            )
                    )
            )
            .flex_grow()
    }
}
