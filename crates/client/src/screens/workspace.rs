use gpui::{
    AppContext, Context, Entity, IntoElement as _, ParentElement as _, Render, Styled,
    Window, div, px,
};
use gpui_component::{
    StyledExt,
    resizable::{h_resizable, resizable_panel},
    separator::Separator,
};
use rpc::{client::ClientConnection, models::markers::UserId};

use crate::components::{
    call_room::CallRoom,
    chat_state::ChatState,
    collapsable_card::CollapsableCardState,
    connection_state::{RpcConnectionInfo, ServerConnectionState},
    left_sidebar::{
        ControlPanel, text_channels::TextChannelsComponent, voice_channels::VoiceChannelsComponent,
    },
};

pub struct WorkspaceScreen {
    chat: Entity<ChatState>,
    streaming: Entity<ServerConnectionState>,

    text_card: Entity<CollapsableCardState>,
    voice_card: Entity<CollapsableCardState>,
}

impl WorkspaceScreen {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        user_id: UserId,
        connection: ClientConnection,
        connection_info: RpcConnectionInfo,
    ) -> Self {
        let chat = cx.new(|cx| ChatState::new(window, cx));
        let connection_state =
            cx.new(|cx| ServerConnectionState::new(cx, user_id, connection, connection_info));

        let text_card = cx.new(|_| CollapsableCardState::new(true));
        let voice_card = cx.new(|_| CollapsableCardState::new(false));

        connection_state.update(cx, |this, cx| {
            this.fetch_voice_channels(cx);

            this.watch_voice_channel_updates(cx);
            this.watch_streaming_state_updates(cx);
        });

        Self {
            chat,
            streaming: connection_state,

            text_card,
            voice_card,
        }
    }
}

impl Render for WorkspaceScreen {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
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
                        .child(Separator::horizontal().mx_3())
                        .child(VoiceChannelsComponent::new(
                            &self.voice_card,
                            &self.streaming,
                        ))
                        .child(Separator::horizontal().mx_3().mt_auto())
                        .child(ControlPanel::new(&self.streaming)),
                ),
            )
            .child(
                div()
                    .v_flex()
                    .size_full()
                    .child(CallRoom::new(&self.streaming))
                    .into_any_element(),
            )
    }
}
