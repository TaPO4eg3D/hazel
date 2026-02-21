use std::time::Duration;

use gpui::{
    Animation, App, Bounds, ElementId, Entity, InteractiveElement, IntoElement, MouseDownEvent,
    ParentElement as _, Pixels, RenderOnce, StatefulInteractiveElement, Styled, Window, div,
    ease_in_out, prelude::FluentBuilder, px, red, rgb, white,
};
use gpui_component::{
    ActiveTheme, Anchor, ElementExt, Icon, Sizable, Size, StyledExt,
    button::{Button, ButtonVariants},
    divider::Divider,
    label::Label,
    popover::{Popover, PopoverState},
    slider::Slider,
};

use crate::{
    ConnectionManger,
    assets::IconName,
    components::{
        animation::HoverAnimationExt, chat_state::ChatState, streaming_state::StreamingState,
    },
    gpui_audio::Streaming,
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

        let current_user = ConnectionManger::get_user_id(cx);
        let (is_mic_off, is_sound_off) = {
            let state = self.streaming_state.read(cx);

            (!state.is_capture_enabled, !state.is_playback_enabled)
        };

        let channels = voice_channels.iter().map(|channel| {
            let muted = cx.theme().muted;

            let members = channel.members.iter().map(|member| {
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
                                        .when(
                                            member.is_sound_off || is_me && is_sound_off,
                                            |this| {
                                                this.child(
                                                    Icon::new(IconName::HeadphoneOff)
                                                        .text_color(cx.theme().danger)
                                                        .with_size(Size::XSmall),
                                                )
                                            },
                                        )
                                        // `is_talking` is special since it's managed internally
                                        .when(member.is_talking, |this| {
                                            this.child(
                                                div().size_2().rounded_full().bg(rgb(0x00C950)),
                                            )
                                        }),
                                ),
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
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let active_channel_name = {
            self.streaming_state
                .read(cx)
                .get_active_channel()
                .map(|channel| channel.name.clone())
        };
        let is_connected = active_channel_name.is_some();

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
                            .when(!is_connected, |this| {
                                this.child(
                                    Label::new("VOICE DISCONNECTED")
                                        .text_xs()
                                        .text_color(rgb(0xBF242C))
                                        .font_bold(),
                                )
                                .child(
                                    Label::new("Select a channel")
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .font_medium(),
                                )
                            })
                            .when_some(active_channel_name, |this, name| {
                                this.child(
                                    Label::new("VOICE CONNECTED")
                                        .text_xs()
                                        .text_color(rgb(0x00C950))
                                        .font_bold(),
                                )
                                .child(Label::new(name).text_sm().font_medium())
                            }),
                    )
                    .when(is_connected, |this| {
                        this.child(
                            Button::new("disconnect")
                                .ml_auto()
                                .cursor_pointer()
                                .icon(IconName::PhoneOff)
                                .ghost(),
                        )
                    }),
            )
            .child(
                div()
                    .w_full()
                    .mt_2()
                    .flex()
                    .gap_2()
                    .child(AudioDeviceControl::new(
                        &self.streaming_state,
                        AudioDeviceType::Capture,
                    ))
                    .child(AudioDeviceControl::new(
                        &self.streaming_state,
                        AudioDeviceType::Playback,
                    )),
            )
    }
}

#[derive(Default)]
struct CaptureControlState {
    bounds: Option<Bounds<Pixels>>,
    displaying: bool,
}

#[derive(IntoElement)]
struct NoiseReductionItem {
    name: &'static str,
    active: bool,
}

impl NoiseReductionItem {
    fn active(mut self, value: bool) -> Self {
        self.active = value;
        self
    }
}

impl RenderOnce for NoiseReductionItem {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
    }
}

#[derive(IntoElement)]
struct NoiseReductionSelector {
    state: Entity<CaptureControlState>,
}

impl NoiseReductionSelector {
    fn new(state: Entity<CaptureControlState>) -> Self {
        Self { state }
    }
}

impl RenderOnce for NoiseReductionSelector {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let is_hovered = self.state.read(cx).displaying;

        div()
            .id("noise-reduction")
            .p_2()
            .rounded(cx.theme().radius)
            .on_hover({
                let state = self.state.clone();

                move |hovered, _, cx| {
                    if *hovered {
                        state.update(cx, |state, cx| {
                            state.displaying = true;

                            cx.notify();
                        })
                    }
                }
            })
            .when(is_hovered, |this| this.bg(cx.theme().secondary))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .v_flex()
                            .child(Label::new("Noise Supression").text_sm())
                            .child(
                                Label::new("Disabled")
                                    .text_color(cx.theme().muted_foreground)
                                    .font_semibold()
                                    .text_xs(),
                            ),
                    )
                    .child(Icon::new(IconName::ChevronRight).ml_auto()),
            )
            .when(is_hovered, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_1()
                        .left_full()
                        .ml_3()
                        .min_w_24()
                        .text_color(cx.theme().popover_foreground)
                        .border_1()
                        .border_color(cx.theme().border)
                        .shadow_lg()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().background)
                        .on_prepaint(move |bounds, _window, cx| {
                            self.state.update(cx, |this, _cx| {
                                this.bounds = Some(bounds);
                            })
                        })
                        .child(div().v_flex().p_2().child(Divider::horizontal())),
                )
            })
    }
}

#[derive(Clone, Copy)]
enum AudioDeviceType {
    Capture,
    Playback,
}

#[derive(IntoElement)]
struct AudioDeviceControl {
    device_type: AudioDeviceType,
    streaming_state: Entity<StreamingState>,
}

impl AudioDeviceControl {
    fn new(state: &Entity<StreamingState>, device_type: AudioDeviceType) -> Self {
        Self {
            device_type,
            streaming_state: state.clone(),
        }
    }
}

impl RenderOnce for AudioDeviceControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let devices = match self.device_type {
            AudioDeviceType::Playback => self.streaming_state.read(cx).output_devices.clone(),
            AudioDeviceType::Capture => self.streaming_state.read(cx).input_devices.clone(),
        };

        let device_volume = {
            match self.device_type {
                AudioDeviceType::Capture => self.streaming_state.read(cx).capture_volume.clone(),
                AudioDeviceType::Playback => self.streaming_state.read(cx).playback_volume.clone(),
            }
        };

        let is_enabled = match self.device_type {
            AudioDeviceType::Capture => self.streaming_state.read(cx).is_capture_enabled,
            AudioDeviceType::Playback => self.streaming_state.read(cx).is_playback_enabled,
        };

        div()
            .id(match self.device_type {
                AudioDeviceType::Capture => "capture-control",
                AudioDeviceType::Playback => "playback-control",
            })
            .flex()
            .child(
                Button::new("active-toggle")
                    .cursor_pointer()
                    .border_r_0()
                    .rounded_r_none()
                    .when_else(is_enabled, |this| this.outline(), |this| this.danger())
                    .icon(match self.device_type {
                        AudioDeviceType::Capture if is_enabled => IconName::Mic,
                        AudioDeviceType::Capture => IconName::MicOff,
                        AudioDeviceType::Playback if is_enabled => IconName::Headphones,
                        AudioDeviceType::Playback => IconName::HeadphoneOff,
                    })
                    .on_click(
                        window.listener_for(
                            &self.streaming_state,
                            move |this, _, _, cx| match self.device_type {
                                AudioDeviceType::Capture => {
                                    this.toggle_capture(cx);
                                }
                                AudioDeviceType::Playback => {
                                    this.toggle_playback(cx);
                                }
                            },
                        ),
                    )
                    .flex_grow(),
            )
            .child(
                Popover::new("popover")
                    .w_64()
                    .overlay_closable(false)
                    .anchor(Anchor::BottomCenter)
                    .trigger(
                        Button::new("device-select")
                            .outline()
                            .rounded_l_none()
                            .icon(IconName::ChevronUp),
                    )
                    .p_0()
                    .content(move |_, window, cx| {
                        let capture_state =
                            window.use_keyed_state("popover-capture", cx, |_, _| {
                                CaptureControlState::default()
                            });

                        let available_devices = devices.clone().into_iter().map(|device| {
                            div()
                                .id(device.id.clone())
                                .w_full()
                                .rounded_md()
                                .hover(|this| this.bg(cx.theme().secondary))
                                .when(matches!(self.device_type, AudioDeviceType::Capture), {
                                    let capture_state = capture_state.clone();

                                    move |this| {
                                        this.on_hover(move |&hovered, _, cx| {
                                            if hovered {
                                                capture_state.update(cx, |state, cx| {
                                                    state.displaying = false;

                                                    cx.notify();
                                                });
                                            }
                                        })
                                    }
                                })
                                .p_2()
                                .flex()
                                .items_center()
                                .child(
                                    div().pl_1().child(
                                        div()
                                            .size_2()
                                            .rounded_full()
                                            .flex_none()
                                            .when(device.is_active, |this| this.bg(white())),
                                    ),
                                )
                                .child(
                                    // An additional container to force the label to wrap
                                    div().pl_4().w_full().child(
                                        Label::new("fdsf sdfsd fsdf sdf sdf sdfsd fdsf sdf ds")
                                            .text_sm(),
                                    ),
                                )
                                .when(!device.is_active, |this| {
                                    this.on_click(move |_, _, cx| {
                                        let registry = Streaming::get_device_registry(cx);

                                        match self.device_type {
                                            AudioDeviceType::Capture => {
                                                registry.set_active_input(&device);
                                            }
                                            AudioDeviceType::Playback => {
                                                registry.set_active_output(&device);
                                            }
                                        }
                                    })
                                })
                        });

                        div()
                            .id("popover-content")
                            .w_full()
                            .on_mouse_down_out(cx.listener({
                                let capture_state = capture_state.clone();

                                move |popover, e: &MouseDownEvent, window, cx| {
                                    let state = capture_state.read(cx);

                                    if let Some(bounds) = state.bounds
                                        && state.displaying
                                    {
                                        if !bounds.contains(&e.position) {
                                            popover.dismiss(window, cx);
                                        }
                                    } else {
                                        popover.dismiss(window, cx);
                                    }
                                }
                            }))
                            .v_flex()
                            .child(
                                Label::new(match self.device_type {
                                    AudioDeviceType::Capture => "Input Control",
                                    AudioDeviceType::Playback => "Output Control",
                                })
                                .p_2()
                                .text_sm(),
                            )
                            .child(Divider::horizontal())
                            .child(
                                div()
                                    .id("devices-list")
                                    .v_flex()
                                    .w_full()
                                    .gap_1()
                                    .p_2()
                                    .children(available_devices),
                            )
                            .child(Divider::horizontal())
                            .when(
                                matches!(self.device_type, AudioDeviceType::Capture),
                                |this| {
                                    this.child(
                                        div().p_2().child(NoiseReductionSelector::new(
                                            capture_state.clone(),
                                        )),
                                    )
                                    .child(Divider::horizontal())
                                },
                            )
                            .child(
                                div()
                                    .id("volume-control")
                                    .p_2()
                                    .when(matches!(self.device_type, AudioDeviceType::Capture), {
                                        let capture_state = capture_state.clone();

                                        move |this| {
                                            this.on_hover(move |&hovered, _, cx| {
                                                if hovered {
                                                    capture_state.update(cx, |state, cx| {
                                                        state.displaying = false;

                                                        cx.notify();
                                                    });
                                                }
                                            })
                                        }
                                    })
                                    .v_flex()
                                    .child(
                                        div().flex().child(Label::new("Volume").text_xs()).child(
                                            Label::new(format!(
                                                "{}%",
                                                device_volume.read(cx).value()
                                            ))
                                            .text_xs()
                                            .ml_auto(),
                                        ),
                                    )
                                    .child(Slider::new(&device_volume)),
                            )
                    }),
            )
            .flex_grow()
    }
}
