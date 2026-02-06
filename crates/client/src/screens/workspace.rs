use gpui::{
    AppContext, Context, Entity, IntoElement as _, ParentElement as _, Render, Styled, Window, div,
    px,
};
use gpui_component::{
    StyledExt, divider::Divider, resizable::{
        h_resizable,
        resizable_panel,
    }
};

use crate::components::{
    chat_state::ChatState,
    left_sidebar::{ControlPanel, TextChannelsComponent, VoiceChannelsComponent},
    streaming_state::StreamingState,
};

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
        let streaming = cx.new(|cx| StreamingState::new(cx));

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
            .on_resize(|state, _window, cx| {
                // Handle resize event
                // You can read the panel sizes from the state.
                let state = state.read(cx);
                let _sizes = state.sizes();
            })
            .child(
                resizable_panel().size_range(px(288.)..px(384.)).child(
                    div()
                        .size_full()
                        .v_flex()
                        .child(
                            TextChannelsComponent::new(&self.chat)
                                .is_collapsed(self.text_channels_collapsed)
                                .on_toggle_click(cx.listener(|this, ev, _, _cx| {
                                    this.text_channels_collapsed = *ev;
                                })),
                        )
                        .child(Divider::horizontal().mx_3())
                        .child(
                            VoiceChannelsComponent::new(&self.streaming)
                                .is_collapsed(self.voice_channels_collapsed)
                                .on_toggle_click(cx.listener(|this, ev, _, _cx| {
                                    this.voice_channels_collapsed = *ev;
                                })),
                        )
                        .child(Divider::horizontal().mx_3().mt_auto())
                        .child(
                            ControlPanel::new(&self.streaming)
                        )
                ),
            )
            .child(
                div()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_center()
                    .child("CHAT IS IN PROGRESS")
                    .into_any_element(),
            )
    }
}
