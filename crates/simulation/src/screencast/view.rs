use std::{os::fd::OwnedFd, path::Path};

use client::gpui_tokio::Tokio;
use gpui::{
    App, AppContext, AsyncApp, Context, Entity, InteractiveElement as _, ParentElement as _,
    Render, Styled as _, Window, div, prelude::FluentBuilder, surface,
};
use gpui_component::StyledExt as _;
use rpc::client::ClientConnection;

use crate::screencast::FileStreamer;

pub struct ScreenCastView {
    frame: Option<gpui::DMABuffer>,

    host_connection: ClientConnection,
    client_connection: ClientConnection,

    _streaming_task: gpui::Task<()>,
}

impl ScreenCastView {
    pub fn new(
        mut streamer: FileStreamer,
        host_connection: ClientConnection,
        client_connection: ClientConnection,
        _window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let task = cx.spawn(async move |this, cx| {
                loop {
                    let frame = streamer.recv_frame().await;

                    this.update(cx, |this: &mut ScreenCastView, cx| {
                        this.frame = Some(frame);
                        cx.notify();
                    })
                    .unwrap();
                }
            });

            Self {
                frame: None,
                host_connection,
                client_connection,
                _streaming_task: task,
            }
        })
    }
}

impl Render for ScreenCastView {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::prelude::Context<Self>,
    ) -> impl gpui::prelude::IntoElement {
        div()
            .v_flex()
            .size_full()
            // Video block
            .child(
                div()
                    .flex()
                    .size_full()
                    // Preview of screen capture
                    .child(
                        div()
                            .size_full()
                            .when_some(self.frame.clone(), |this, frame| {
                                this.child(surface(frame).size_full())
                            }),
                    )
                    // Reciever
                    .child(div().size_full()),
            )
            // Control panel
            .child(div())
    }
}
