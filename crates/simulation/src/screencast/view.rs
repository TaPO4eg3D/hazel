use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    rc::Rc,
    str::FromStr as _,
};

use capture::video::frames::FrameRecv;
use client::{gpui_tokio::Tokio, streaming::StreamingState};
use gpui::{
    App, AppContext, AsyncApp, Context, Entity, ParentElement as _, Render, Styled as _, Window,
    div, prelude::FluentBuilder, surface,
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
        voice_channels::{
            GetVoiceChannels, JoinScreenCast, JoinScreenCastRequest, JoinVoiceChannel,
            JoinVoiceChannelPayload, RequestIDRFrame, RequestIDRFramePayload, StartScreenCast,
            StartScreenCastRequest,
        },
    },
};

pub struct ConnectionState {
    pub rpc: ClientConnection,
    pub streaming: Rc<StreamingState>,

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
            streaming: streaming.into(),
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

    pub fn start_screencast<T: AsRef<Path>>(
        &self,
        file_path: T,
    ) -> impl Future<Output = FrameRecv<gpui::DMABuffer>> + use<T> {
        let rpc = self.rpc.clone();
        let streaming = self.streaming.clone();

        async move {
            let Some(mut preview) = streaming.start_screencast_from_file(file_path).await else {
                panic!("Failed to start the file cast");
            };

            let (width, height) = loop {
                if let Some(frame) = preview.recv().await {
                    break (frame.width, frame.height);
                };
            };

            let Ok(_params) =
                StartScreenCast::execute(&rpc, &StartScreenCastRequest { width, height }).await
            else {
                streaming.stop_screencast().await;
                panic!("Unable to start the screencast, the server returned an error");
            };

            preview
        }
    }

    pub fn join_screencast(
        &self,
        id: UserId,
    ) -> impl Future<Output = smol::channel::Receiver<gpui::DMABuffer>> + use<> {
        let rpc = self.rpc.clone();
        let streaming = self.streaming.clone();

        async move {
            match JoinScreenCast::execute(
                &rpc,
                &JoinScreenCastRequest {
                    user_id: id,
                    mtu: 0,
                },
            )
            .await
            {
                Ok(params) => {
                    let (frame_tx, frame_rx) = smol::channel::bounded::<gpui::DMABuffer>(1);
                    _ = RequestIDRFrame::execute(&rpc, &RequestIDRFramePayload { user_id: id })
                        .await;

                    streaming.register_video_stream(id, frame_tx, params);

                    frame_rx
                }
                Err(err) => {
                    panic!("Error: {err:?}");
                }
            }
        }
    }
}

pub struct ScreenCastView {
    host: ConnectionState,
    client: ConnectionState,

    preview: Option<gpui::DMABuffer>,
    watch: Option<gpui::DMABuffer>,

    _streaming_task: gpui::Task<()>,
}

impl ScreenCastView {
    pub fn new(
        file_path: PathBuf,
        host: ConnectionState,
        client: ConnectionState,
        _window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let host_id = Id::new(host.session_key.body.user_id);

            let task = cx.spawn(async move |this, cx| {
                let mut preview = this
                    .read_with(cx, move |this: &ScreenCastView, _cx| {
                        this.host.start_screencast(file_path)
                    })
                    .unwrap()
                    .await;

                let watch = this
                    .read_with(cx, |this: &ScreenCastView, _cx| {
                        this.client.join_screencast(host_id)
                    })
                    .unwrap()
                    .await;

                cx.spawn({
                    let this = this.clone();

                    async move |cx| {
                        while let Some(frame) = preview.recv().await {
                            this.update(cx, |this, cx| {
                                this.preview = Some(frame);

                                cx.notify();
                            })
                            .ok();
                        }
                    }
                })
                .detach();

                cx.spawn({
                    let this = this.clone();

                    async move |cx| {
                        while let Ok(frame) = watch.recv().await {
                            this.update(cx, |this, cx| {
                                this.watch = Some(frame);
                                cx.notify();
                            })
                            .ok();
                        }
                    }
                })
                .detach();
            });

            Self {
                host,
                client,
                preview: None,
                watch: None,
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
                            .when_some(self.preview.clone(), |this, frame| {
                                this.child(surface(frame).size_full())
                            }),
                    )
                    // Reciever
                    .child(
                        div()
                            .size_full()
                            .when_some(self.watch.clone(), |this, frame| {
                                this.child(surface(frame).size_full())
                            }),
                    ),
            )
            // Control panel
            .child(div())
    }
}
