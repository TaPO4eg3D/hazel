use std::{os::fd::OwnedFd, path::Path};

use gpui::{
    App, AppContext, DMABuffer, Entity, InteractiveElement as _, ParentElement as _, Render,
    Styled as _, Window, div, prelude::FluentBuilder, surface,
};
use tokio::time::Instant;

use crate::screencast::FileStreamer;

pub struct ScreenCastView {
    _streaming_task: gpui::Task<()>,
    frame: Option<gpui::DMABuffer>,
}

impl ScreenCastView {
    pub fn new(mut streamer: FileStreamer, window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let task = cx.spawn_in(window, async move |this, cx| {
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
                _streaming_task: task,
                frame: None,
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
            .flex()
            .size_full()
            .when_some(self.frame.clone(), |this, frame| {
                this.child(surface(frame).size_full())
            })
    }
}
