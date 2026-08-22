use gpui::{
    Anchor, App, Bounds, ElementId, Entity, InteractiveElement, IntoElement, MouseDownEvent,
    ParentElement as _, Pixels, RenderOnce, StatefulInteractiveElement, StyleRefinement, Styled,
    Window, div, prelude::FluentBuilder, relative, rgb, white,
};
use gpui_component::{
    ActiveTheme, ElementExt, Icon, StyledExt,
    button::{Button, ButtonVariants},
    label::Label,
    popover::Popover,
    separator::Separator,
    slider::Slider,
};

use crate::{
    assets::IconName,
    components::connection_state::{NoiseReductionAlgorithm, ServerConnectionState},
};

pub mod text_channels;
pub mod voice_channels;

#[derive(IntoElement)]
pub struct ControlPanel {
    connection_state: Entity<ServerConnectionState>,
}

impl ControlPanel {
    pub fn new(state: &Entity<ServerConnectionState>) -> Self {
        Self {
            connection_state: state.clone(),
        }
    }
}

impl RenderOnce for ControlPanel {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let active_channel_name = {
            self.connection_state
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
                                .ghost()
                                .on_click({
                                    let state = self.connection_state.clone();

                                    move |_, _, cx| {
                                        state.update(cx, |this, cx| {
                                            this.leave_voice_channel(cx);
                                        });
                                    }
                                }),
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
                        &self.connection_state,
                        AudioDeviceType::Capture,
                    ))
                    .child(AudioDeviceControl::new(
                        &self.connection_state,
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
    id: ElementId,

    name: &'static str,
    active: bool,

    #[allow(clippy::type_complexity)]
    on_click: Option<Box<dyn Fn(&mut Window, &mut App)>>,

    style: StyleRefinement,
}

impl NoiseReductionItem {
    fn new(id: impl Into<ElementId>, name: &'static str) -> Self {
        Self {
            id: id.into(),
            name,
            active: false,
            style: StyleRefinement::default(),
            on_click: None,
        }
    }

    fn active(mut self, value: bool) -> Self {
        self.active = value;
        self
    }

    fn on_click(mut self, value: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(value));
        self
    }
}

impl Styled for NoiseReductionItem {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for NoiseReductionItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .id(self.id)
            .p_2()
            .hover(|this| this.bg(cx.theme().secondary))
            .flex()
            .items_center()
            .rounded(cx.theme().radius)
            .child(
                div().pl_1().child(
                    div()
                        .size_2()
                        .rounded_full()
                        .flex_none()
                        .when(self.active, |this| this.bg(white())),
                ),
            )
            .child(Label::new(self.name).pl_4().pr_2().text_sm())
            .when_some(self.on_click, |this, on_click| {
                this.on_click(move |_, window, cx| on_click(window, cx))
            })
            .refine_style(&self.style)
    }
}

#[derive(IntoElement)]
struct NoiseReductionSelector {
    connection_state: Entity<ServerConnectionState>,
    capture_state: Entity<CaptureControlState>,
}

impl NoiseReductionSelector {
    fn new(
        connection_state: Entity<ServerConnectionState>,
        capture_state: Entity<CaptureControlState>,
    ) -> Self {
        Self {
            capture_state,
            connection_state,
        }
    }
}

impl RenderOnce for NoiseReductionSelector {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let active_algorithm = self.connection_state.read(cx).noise_reduction();
        let is_hovered = self.capture_state.read(cx).displaying;

        div()
            .id("noise-reduction")
            .p_2()
            .rounded(cx.theme().radius)
            .on_hover({
                let state = self.capture_state.clone();

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
                                Label::new(active_algorithm.label())
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
                        .top(relative(-0.5))
                        .occlude()
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
                            self.capture_state.update(cx, |this, _cx| {
                                this.bounds = Some(bounds);
                            })
                        })
                        .child(
                            div()
                                .id("noise-supression-algo")
                                .v_flex()
                                .child(div().v_flex().p_2().child({
                                    let state = self.connection_state.clone();
                                    let algorithm = NoiseReductionAlgorithm::Disabled;

                                    NoiseReductionItem::new(algorithm.id(), algorithm.label())
                                        .active(active_algorithm == algorithm)
                                        .on_click(move |_, cx| {
                                            state.update(cx, |state, cx| {
                                                state.set_noise_reduction(algorithm, cx);
                                            });
                                        })
                                }))
                                .child(Separator::horizontal())
                                .child(
                                    div()
                                        .v_flex()
                                        .gap_1()
                                        .p_2()
                                        .child({
                                            let state = self.connection_state.clone();
                                            let algorithm = NoiseReductionAlgorithm::RNNoise;

                                            NoiseReductionItem::new(
                                                algorithm.id(),
                                                algorithm.label(),
                                            )
                                            .active(active_algorithm == algorithm)
                                            .on_click(
                                                move |_, cx| {
                                                    state.update(cx, |state, cx| {
                                                        state.set_noise_reduction(algorithm, cx);
                                                    });
                                                },
                                            )
                                        })
                                        .child({
                                            let algorithm = NoiseReductionAlgorithm::DeepFilterNet;

                                            NoiseReductionItem::new(
                                                algorithm.id(),
                                                algorithm.label(),
                                            )
                                            .active(active_algorithm == algorithm)
                                            .on_click(
                                                move |_, cx| {
                                                    self.connection_state.update(
                                                        cx,
                                                        |state, cx| {
                                                            state
                                                                .set_noise_reduction(algorithm, cx);
                                                        },
                                                    );
                                                },
                                            )
                                        }),
                                ),
                        ),
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
    connection_state: Entity<ServerConnectionState>,
}

impl AudioDeviceControl {
    fn new(state: &Entity<ServerConnectionState>, device_type: AudioDeviceType) -> Self {
        Self {
            device_type,
            connection_state: state.clone(),
        }
    }
}

impl RenderOnce for AudioDeviceControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let devices = match self.device_type {
            AudioDeviceType::Playback => self.connection_state.read(cx).output_devices.clone(),
            AudioDeviceType::Capture => self.connection_state.read(cx).input_devices.clone(),
        };

        let device_volume = {
            match self.device_type {
                AudioDeviceType::Capture => self.connection_state.read(cx).capture_volume.clone(),
                AudioDeviceType::Playback => self.connection_state.read(cx).playback_volume.clone(),
            }
        };

        let is_enabled = match self.device_type {
            AudioDeviceType::Capture => self.connection_state.read(cx).is_capture_enabled,
            AudioDeviceType::Playback => self.connection_state.read(cx).is_playback_enabled,
        };

        let streaming = self.connection_state.read(cx).streaming.clone();

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
                            &self.connection_state,
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
                    .flex_grow_1(),
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

                        let available_devices =
                            devices.clone().into_iter().map(|device| {
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
                                        div().pl_4().pr_2().w_full().child(
                                            Label::new(device.display_name.clone()).text_sm(),
                                        ),
                                    )
                                    .when(!device.is_active, |this| {
                                        let streaming = streaming.clone();

                                        this.on_click(move |_, _, cx| {
                                            let registry = streaming.get_device_registry();

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
                            .child(Separator::horizontal())
                            .child(
                                div()
                                    .id("devices-list")
                                    .v_flex()
                                    .w_full()
                                    .gap_1()
                                    .p_2()
                                    .children(available_devices),
                            )
                            .child(Separator::horizontal())
                            .when(
                                matches!(self.device_type, AudioDeviceType::Capture),
                                |this| {
                                    this.child(div().p_2().child(NoiseReductionSelector::new(
                                        self.connection_state.clone(),
                                        capture_state.clone(),
                                    )))
                                    .child(Separator::horizontal())
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
            .flex_grow_1()
    }
}
