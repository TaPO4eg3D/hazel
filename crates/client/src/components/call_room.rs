use gpui::{
    App, AppContext, ClickEvent, ElementId, Entity, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, Window, div, prelude::FluentBuilder, surface,
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IndexPath, Sizable, Size, StyledExt as _,
    button::{Button, ButtonVariants},
    label::Label,
    select::{Select, SelectState},
};

use crate::{assets::IconName, components::streaming_state::StreamingState};

struct CallRoomState {
    show_configuration: bool,
    select_state: Entity<SelectState<Vec<SharedString>>>,
}

#[derive(IntoElement)]
pub struct CallRoom {
    id: ElementId,
    streaming: Entity<StreamingState>,
}

impl CallRoom {
    pub fn new(id: impl Into<ElementId>, streaming: &Entity<StreamingState>) -> Self {
        Self {
            id: id.into(),
            streaming: streaming.clone(),
        }
    }
}

impl RenderOnce for CallRoom {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl gpui::IntoElement {
        let room_state = window.use_keyed_state(self.id, cx, |window, cx| CallRoomState {
            show_configuration: false,
            select_state: cx.new(|cx| {
                SelectState::new(
                    vec!["Low latency: 60 FPS".into(), "High quality: 30 FPS".into()],
                    Some(IndexPath::default()), // Select first item
                    window,
                    cx,
                )
            }),
        });

        let CallRoomState {
            show_configuration,
            select_state,
        } = room_state.read(cx);

        div()
            .p_3()
            .v_flex()
            .gap_4()
            .size_full()
            .child(
                ScreenSpace::new(&self.streaming, select_state.clone())
                    .show_configuration(*show_configuration)
                    .on_config({
                        let room_state = room_state.clone();

                        window.listener_for(
                            &self.streaming,
                            move |streaming_state, _: &(), window, cx| {
                                room_state.update(cx, |this, _cx| {
                                    this.show_configuration = false;
                                });

                                streaming_state.start_screencast(window, cx);
                            },
                        )
                    }),
            )
            .child(ControlPanel::new(&self.streaming).on_click({
                move |_, _, cx| {
                    room_state.update(cx, |this, _cx| {
                        this.show_configuration = true;
                    });
                }
            }))
    }
}

#[derive(IntoElement)]
struct ScreenSpace {
    streaming: Entity<StreamingState>,
    show_configuration: bool,
    select_state: Entity<SelectState<Vec<SharedString>>>,
    on_config: Option<Box<dyn Fn(&(), &mut Window, &mut App)>>,
}

impl ScreenSpace {
    pub fn new(
        streaming: &Entity<StreamingState>,
        select_state: Entity<SelectState<Vec<SharedString>>>,
    ) -> Self {
        Self {
            show_configuration: false,
            select_state,
            streaming: streaming.clone(),
            on_config: None,
        }
    }

    fn show_configuration(mut self, value: bool) -> Self {
        self.show_configuration = value;
        self
    }

    fn on_config(mut self, value: impl Fn(&(), &mut Window, &mut App) + 'static) -> Self {
        self.on_config = Some(Box::new(value));
        self
    }
}

impl RenderOnce for ScreenSpace {
    fn render(self, _window: &mut gpui::Window, cx: &mut gpui::App) -> impl gpui::IntoElement {
        let state = self.streaming.read(cx);

        div()
            .v_flex()
            .rounded_xl()
            .border_1()
            .size_full()
            .border_color(cx.theme().secondary)
            .when(self.show_configuration, |this| this)
            .when_else(
                self.show_configuration,
                |this| {
                    this.child(
                        div()
                            .v_flex()
                            .size_full()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .v_flex()
                                    .max_size_80()
                                    .w_full()
                                    .p_4()
                                    .gap_4()
                                    .child(Select::new(&self.select_state).w_full())
                                    .child(
                                        div()
                                            .flex()
                                            .gap_2()
                                            .child(
                                                Button::new("screencast-decline-config")
                                                    .label("Cancel")
                                                    .danger()
                                                    .flex_1(),
                                            )
                                            .child(
                                                Button::new("screencast-accept-config")
                                                    .label("Confirm")
                                                    .flex_1(),
                                            ),
                                    ),
                            ),
                    )
                },
                |this| {
                    // When we do not display configuration
                    this.when_some(state.preview_frame.as_ref(), |this, frame| {
                        this.child(surface(frame.clone()).size_full())
                    })
                    .when_some(state.watching_frame.as_ref(), |this, frame| {
                        this.child(surface(frame.clone()).size_full())
                    })
                    .when(!state.is_stream_playing(), |this| {
                        this.child(
                            div()
                                .v_flex()
                                .size_full()
                                .items_center()
                                .justify_center()
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
                                ),
                        )
                    })
                },
            )
    }
}

#[derive(IntoElement)]
struct ControlPanel {
    streaming: Entity<StreamingState>,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
}

impl ControlPanel {
    pub fn new(streaming: &Entity<StreamingState>) -> Self {
        Self {
            streaming: streaming.clone(),
            on_click: None,
        }
    }

    fn on_click(mut self, value: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(value));
        self
    }
}

impl RenderOnce for ControlPanel {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        let (can_stream, is_streaming) = self.streaming.read_with(cx, |state, _| {
            (
                state.get_active_channel().is_some(),
                state.preview_frame.is_some(),
            )
        });

        div()
            .p_4()
            .flex()
            .items_center()
            .rounded_xl()
            .border_1()
            .w_full()
            .border_color(cx.theme().secondary)
            .when_else(
                is_streaming,
                |this| {
                    this.child(
                        Button::new("stop-streaming")
                            .icon(IconName::ScreenShare)
                            .label("Stop streaming")
                            .max_w_64()
                            .w_full()
                            .danger()
                            .on_click(window.listener_for(
                                &self.streaming,
                                |this, _, window, cx| {
                                    this.stop_screencast(window, cx);
                                },
                            )),
                    )
                },
                |this| {
                    this.child(
                        Button::new("start-streaming")
                            .icon(IconName::ScreenShare)
                            .label("Share screen")
                            .max_w_64()
                            .w_full()
                            .when(!can_stream, |this| {
                                this.disabled(!can_stream)
                                    .tooltip("Join a voice channel first")
                            })
                            .when(can_stream, |this| {
                                this.when_some(self.on_click, |this, on_click| {
                                    this.on_click(on_click)
                                })
                            })
                            .primary(),
                    )
                },
            )
            .child(
                Label::new(if is_streaming {
                    "Your screen preview is playing"
                } else {
                    "No stream is playing"
                })
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .ml_auto(),
            )
    }
}
