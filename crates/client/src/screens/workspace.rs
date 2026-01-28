use gpui::{AppContext, Context, Entity, IntoElement as _, ParentElement as _, Render, Window, div, px};
use gpui_component::resizable::{
    ResizablePanel, ResizablePanelEvent, ResizablePanelGroup, ResizableState, h_resizable,
    resizable_panel, v_resizable,
};

use crate::components::{chat_state::ChatState, streaming_state::StreamingState};

pub struct WorkspaceScreen {
    text_channels_collapsed: bool,
    voice_channels_collapsed: bool,

    chat: Entity<ChatState>,
    streaming: Entity<StreamingState>,
}

impl WorkspaceScreen {
    pub fn init<C: AppContext>(&self, cx: &mut C) {
        self.streaming.update(cx, |this, cx| {
            this.fetch_voice_channels(cx);

            this.watch_voice_channel_updates(cx);
            this.watch_streaming_state_updates(cx);
        });
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let chat = cx.new(|cx| ChatState::new(window, cx));
        let streaming = cx.new(|_| StreamingState::default());

        Self {
            chat,
            streaming,

            text_channels_collapsed: false,
            voice_channels_collapsed: false,
        }
    }
}

impl Render for WorkspaceScreen {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        h_resizable("my-layout")
            .on_resize(|state, window, cx| {
                // Handle resize event
                // You can read the panel sizes from the state.
                let state = state.read(cx);
                let sizes = state.sizes();
            })
            .child(
                // Use resizable_panel() to create a sized panel.
                resizable_panel().size(px(200.)).child("Left Panel"),
            )
            .child(
                // Or you can just add AnyElement without a size.
                div().child("Right Panel").into_any_element(),
            )
    }
}
