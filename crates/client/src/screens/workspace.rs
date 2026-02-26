use gpui::{
    AppContext, Context, Entity, IntoElement as _, ParentElement as _, Render, Styled, Window, div,
    px,
};
use gpui_component::{
    StyledExt,
    divider::Divider,
    resizable::{h_resizable, resizable_panel},
};

use crate::components::{
    chat_state::ChatState,
    collapsable_card::CollapsableCardState,
    left_sidebar::{
        ControlPanel, text_channels::TextChannelsComponent, voice_channels::VoiceChannelsComponent,
    },
    streaming_state::StreamingState,
};

pub struct WorkspaceScreen {
    chat: Entity<ChatState>,
    streaming: Entity<StreamingState>,

    text_card: Entity<CollapsableCardState>,
    voice_card: Entity<CollapsableCardState>,
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
        let streaming = cx.new(StreamingState::new);

        let text_card = cx.new(|_| CollapsableCardState::new());
        let voice_card = cx.new(|_| CollapsableCardState::new());

        Self {
            chat,
            streaming,

            text_card,
            voice_card,
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
                        .child(TextChannelsComponent::new(&self.text_card, &self.chat))
                        .child(Divider::horizontal().mx_3())
                        .child(VoiceChannelsComponent::new(
                            &self.voice_card,
                            &self.streaming,
                        ))
                        .child(Divider::horizontal().mx_3().mt_auto())
                        .child(ControlPanel::new(&self.streaming)),
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
