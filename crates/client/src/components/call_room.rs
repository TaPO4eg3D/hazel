use gpui::{
    Entity, IntoElement, ParentElement, RenderOnce, Styled, div, prelude::FluentBuilder, surface,
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, Sizable, Size, StyledExt as _,
    button::{Button, ButtonVariants},
    label::Label,
};

use crate::{assets::IconName, components::streaming_state::StreamingState, gpui_audio::Streaming};

#[derive(IntoElement)]
pub struct CallRoom {
    streaming: Entity<StreamingState>,
}

impl CallRoom {
    pub fn new(streaming: &Entity<StreamingState>) -> Self {
        Self {
            streaming: streaming.clone(),
        }
    }
}

impl RenderOnce for CallRoom {
    fn render(self, _window: &mut gpui::Window, _cx: &mut gpui::App) -> impl gpui::IntoElement {
        div()
            .p_3()
            .v_flex()
            .gap_4()
            .size_full()
            .child(ScreenSpace::new(&self.streaming))
            .child(ControlPanel::new(&self.streaming))
    }
}

#[derive(IntoElement)]
struct ScreenSpace {
    streaming: Entity<StreamingState>,
}

impl ScreenSpace {
    pub fn new(streaming: &Entity<StreamingState>) -> Self {
        Self {
            streaming: streaming.clone(),
        }
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
            .when_some(state.preview_frame.as_ref(), |this, frame| {
                this.child(surface(frame.clone()).size_full())
            })
            // No stream placeholder
            .when_none(&state.preview_frame, |this| {
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
    }
}

#[derive(IntoElement)]
struct ControlPanel {
    streaming: Entity<StreamingState>,
}

impl ControlPanel {
    pub fn new(streaming: &Entity<StreamingState>) -> Self {
        Self {
            streaming: streaming.clone(),
        }
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
                            .danger(),
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
                                this.on_click(window.listener_for(
                                    &self.streaming,
                                    |_, _, _, cx| {
                                        cx.spawn(async |state, cx| {
                                            if let Some(preview) =
                                                Streaming::start_screencast(cx).await
                                            {
                                                state
                                                    .update(cx, move |this, cx| {
                                                        this.set_screencast_preview(preview, cx);
                                                    })
                                                    .ok();
                                            }
                                        })
                                        .detach();
                                    },
                                ))
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
