use std::{net::SocketAddr, str::FromStr as _};

use client::{gpui_tokio::Tokio, streaming::StreamingState};
use gpui::{
    App, AppContext, AsyncApp, Entity, ParentElement as _, Render, Styled as _, Window, div,
    prelude::FluentBuilder, surface,
};
use gpui_component::StyledExt as _;
use rpc::{
    client::ClientConnection,
    common::Empty,
    models::{
        auth::{
            GetSessionKey, GetSessionKeyPayload, GetSessionKeyResponse, Login, LoginPayload,
            SessionKey,
        },
        common::RPCMethod as _,
        markers::{Id, UserId, VoiceChannelId},
        voice_channels::{GetVoiceChannels, JoinVoiceChannel, JoinVoiceChannelPayload},
    },
};

pub struct ConnectionState {
    pub rpc: ClientConnection,
    pub streaming: StreamingState,

    pub session_key: SessionKey,
    pub active_voice_channel: Option<VoiceChannelId>,
}

impl ConnectionState {
    pub async fn new(login: &str, password: &str, cx: &mut AsyncApp) -> Self {
        let rpc = Tokio::spawn(cx, async move { ClientConnection::new("127.0.0.1:9898") })
            .await
            .expect("todo: we currently can't fail and just hang");

        let response = GetSessionKey::execute(
            &rpc,
            &GetSessionKeyPayload {
                login: login.to_string(),
                password: password.to_string(),
            },
        )
        .await
        .expect("auth failed");

        let session_key = match response {
            GetSessionKeyResponse::NewUser(session_key) => session_key,
            GetSessionKeyResponse::ExistingUser(session_key) => session_key,
        };

        Login::execute(
            &rpc,
            &LoginPayload {
                session_key: session_key.clone(),
            },
        )
        .await
        .expect("auth failed");

        let streaming = StreamingState::new();
        streaming.connect(
            Id::new(session_key.body.user_id),
            SocketAddr::from_str("127.0.0.1:9899").unwrap(),
        );

        ConnectionState {
            rpc,
            streaming,
            session_key,
            active_voice_channel: None,
        }
    }

    pub async fn join_voice_channel(&mut self) {
        let channels = GetVoiceChannels::execute(&self.rpc, &Empty).await.unwrap();
        let channel = channels.first().unwrap();

        JoinVoiceChannel::execute(
            &self.rpc,
            &JoinVoiceChannelPayload {
                channel_id: channel.id,
            },
        )
        .await
        .expect("Failed to join a voice channel");

        self.active_voice_channel = Some(channel.id);
    }

    pub async fn start_screencast(&self) {}

    pub async fn join_screencast(id: UserId) {}
}

pub struct ScreenCastView {
    frame: Option<gpui::DMABuffer>,

    host: ConnectionState,
    client: ConnectionState,

    _streaming_task: gpui::Task<()>,
}

impl ScreenCastView {
    pub fn new(
        host: ConnectionState,
        client: ConnectionState,
        _window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let task = cx.spawn(async move |this, cx| {});

            Self {
                frame: None,
                host,
                client,
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
