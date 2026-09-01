use std::{
    rc::Rc,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use atomic_enum::atomic_enum;
use capture::audio::{AudioDevice, playback::AudioStreamingClientSharedState};

#[cfg(target_os = "linux")]
use capture::video::frames::{FrameRecv, frame_channel};

use gpui::{
    AppContext, AsyncApp, AsyncWindowContext, Context, Entity, SharedString, Subscription, Task,
    WeakEntity, Window,
};

#[cfg(target_os = "linux")]
use gpui::DMABuffer;

use gpui_component::{
    WindowExt,
    notification::Notification,
    slider::{SliderEvent, SliderState, SliderValue},
};
use rpc::{
    client::ClientConnection,
    common::Empty,
    models::{
        auth::{GetUserInfo, GetUserPayload},
        common::RPCMethod as _,
        markers::{UserId, VoiceChannelId},
        voice_channels::{
            GetVoiceChannels, JoinScreenCast, JoinScreenCastRequest, JoinVoiceChannel,
            JoinVoiceChannelPayload, LeaveScreenCast, LeaveScreenCastRequest, LeaveVoiceChannel,
            RequestIDRFrame, RequestIDRFramePayload, StartScreenCast, StartScreenCastRequest,
            StopScreenCast, UpdateVoiceChannelUserState, VoiceChannelUpdate,
            VoiceChannelUpdateMessage, VoiceChannelUserState,
        },
    },
};
use smol::{channel, stream::StreamExt as _};

use crate::streaming::StreamingState;

#[derive(Clone)]
pub struct VoiceChannel {
    pub id: VoiceChannelId,
    pub name: SharedString,

    pub is_active: bool,
    pub members: Vec<VoiceChannelMember>,
}

struct VoiceChannelMemberState {
    playback: Arc<AudioStreamingClientSharedState>,

    _subscription: Subscription,
}

#[derive(Clone)]
pub struct VoiceChannelMember {
    pub id: UserId,
    pub name: SharedString,

    pub state: VoiceChannelUserState,
    pub is_talking: bool,

    pub output_volume: Entity<SliderState>,

    shared: Option<Entity<VoiceChannelMemberState>>,
}

impl VoiceChannelMember {
    pub fn new(
        id: UserId,
        name: SharedString,
        state: VoiceChannelUserState,
        cx: &mut Context<ServerConnectionState>,
    ) -> Self {
        let output_volume = cx.new(|_cx| {
            SliderState::new()
                .min(0.)
                .max(200.)
                .step(1.)
                .default_value(100.)
        });

        VoiceChannelMember {
            id,
            name,
            state,
            is_talking: false,
            output_volume,
            shared: None,
        }
    }

    pub fn fetch_is_talking(
        &mut self,
        connected_user: UserId,
        streaming: &Rc<StreamingState>,
        cx: &Context<ServerConnectionState>,
    ) -> bool {
        let was_talking = self.is_talking;

        self.is_talking = if connected_user == self.id {
            streaming.is_talking()
        } else if let Some(state) = self.shared.as_ref() {
            state.read(cx).playback.is_talking.load(Ordering::Relaxed)
        } else {
            false
        };

        self.is_talking != was_talking
    }

    pub fn register(
        &mut self,
        streaming: &Rc<StreamingState>,
        cx: &mut Context<ServerConnectionState>,
    ) {
        let playback_state = Arc::new(AudioStreamingClientSharedState::new(self.id.value));

        let subscription = cx.subscribe(&self.output_volume, {
            let playback_state = playback_state.clone();

            move |_, _, ev, _| {
                let value = match ev {
                    SliderEvent::Change(SliderValue::Single(value)) => *value,
                    SliderEvent::Release(SliderValue::Single(value)) => *value,
                    _ => return,
                };

                playback_state
                    .volume
                    .store((value / 100.).powf(3.), Ordering::Relaxed);
            }
        });

        let shared = cx.new({
            let playback_state = playback_state.clone();

            move |_cx| VoiceChannelMemberState {
                playback: playback_state.clone(),
                _subscription: subscription,
            }
        });

        self.shared = Some(shared);
        streaming.add_voice_member(Arc::downgrade(&playback_state));
    }

    pub fn unregister(&mut self) {
        self.shared = None;
    }
}

#[atomic_enum]
#[derive(PartialEq)]
pub enum NoiseReductionAlgorithm {
    Disabled = 0,
    RNNoise,
    DeepFilterNet,
}

impl NoiseReductionAlgorithm {
    pub fn id(&self) -> &'static str {
        match self {
            NoiseReductionAlgorithm::Disabled => "disabled",
            NoiseReductionAlgorithm::RNNoise => "rnnoise",
            NoiseReductionAlgorithm::DeepFilterNet => "deepfilternet",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            NoiseReductionAlgorithm::Disabled => "Disabled",
            NoiseReductionAlgorithm::RNNoise => "RNNoise",
            NoiseReductionAlgorithm::DeepFilterNet => "DeepFilterNet",
        }
    }
}

#[derive(Clone)]
pub struct RpcConnectionInfo {
    pub server_ip: String,
    // pub port: String,
}

/// Connection per one dedicated server. Ideally we want to support
/// multiple active connections to different servers
pub struct ServerConnectionState {
    pub rpc: ClientConnection,
    pub user_id: UserId,
    pub rpc_info: RpcConnectionInfo,

    pub streaming: Rc<StreamingState>,

    pub voice_channels: Vec<VoiceChannel>,

    pub capture_volume: Entity<SliderState>,
    pub playback_volume: Entity<SliderState>,

    pub is_capture_enabled: bool,
    pub is_playback_enabled: bool,

    pub input_devices: Vec<AudioDevice>,
    pub output_devices: Vec<AudioDevice>,

    noise_reduction: NoiseReductionAlgorithm,

    screencast_preview_task: Option<Task<()>>,
    watching_frame_task: Option<Task<()>>,

    #[cfg(target_os = "linux")]
    pub preview_frame: Option<DMABuffer>,
    #[cfg(target_os = "linux")]
    pub watching_frame: Option<DMABuffer>,
}

impl ServerConnectionState {
    pub fn new(
        cx: &mut Context<Self>,
        user_id: UserId,
        rpc: ClientConnection,
        rpc_info: RpcConnectionInfo,
    ) -> Self {
        let state = Self {
            user_id,

            rpc,
            rpc_info,

            screencast_preview_task: None,
            watching_frame_task: None,

            #[cfg(target_os = "linux")]
            preview_frame: None,
            #[cfg(target_os = "linux")]
            watching_frame: None,

            voice_channels: vec![],

            capture_volume: cx.new(|_| {
                SliderState::new()
                    .min(0.)
                    .max(200.)
                    .default_value(100.)
                    .step(1.)
            }),
            playback_volume: cx.new(|_| {
                SliderState::new()
                    .min(0.)
                    .max(200.)
                    .default_value(100.)
                    .step(1.)
            }),

            input_devices: vec![],
            output_devices: vec![],

            is_playback_enabled: true,
            is_capture_enabled: true,

            streaming: Rc::new(StreamingState::new()),
            noise_reduction: NoiseReductionAlgorithm::RNNoise,
        };

        cx.subscribe(&state.capture_volume, |this, state, _, cx| {
            let state = state.read(cx);

            if let SliderValue::Single(value) = state.value() {
                this.streaming
                    .set_input_volume_modifier((value / 100.).powf(3.));
            }
        })
        .detach();

        cx.subscribe(&state.playback_volume, |this, state, _, cx| {
            let state = state.read(cx);

            if let SliderValue::Single(value) = state.value() {
                this.streaming
                    .set_output_volume_modifier((value / 100.).powf(3.));
            }
        })
        .detach();

        state
    }
}

impl ServerConnectionState {
    pub fn noise_reduction(&self) -> NoiseReductionAlgorithm {
        self.noise_reduction
    }

    pub fn set_noise_reduction(
        &mut self,
        noise_reduction: NoiseReductionAlgorithm,
        cx: &mut Context<Self>,
    ) {
        self.noise_reduction = noise_reduction;
        self.streaming.set_noise_reduction(noise_reduction);

        cx.notify();
    }

    pub fn get_active_channel(&self) -> Option<&VoiceChannel> {
        self.voice_channels.iter().find(|channel| channel.is_active)
    }

    pub fn get_active_channel_mut(&mut self) -> Option<&mut VoiceChannel> {
        self.voice_channels
            .iter_mut()
            .find(|channel| channel.is_active)
    }

    pub fn get_voice_channel(&self, id: VoiceChannelId) -> Option<&VoiceChannel> {
        self.voice_channels.iter().find(|channel| channel.id == id)
    }

    pub fn get_voice_channel_mut(&mut self, id: VoiceChannelId) -> Option<&mut VoiceChannel> {
        self.voice_channels
            .iter_mut()
            .find(|channel| channel.id == id)
    }

    pub fn sync_server_state(&mut self, cx: &mut Context<Self>) {
        if self.get_active_channel().is_none() {
            return;
        }

        cx.spawn(async move |this, cx| {
            let Some(connection) = this.read_with(cx, |this, _cx| this.rpc.clone()).ok() else {
                return;
            };

            let Some((is_sound_off, is_mic_off, is_streaming)) = this
                .read_with(cx, |this, _cx| {
                    (
                        !this.is_playback_enabled,
                        !this.is_capture_enabled,
                        cfg_select! {
                            target_os = "linux" => this.preview_frame.is_some(),
                            _ => false
                        },
                    )
                })
                .ok()
            else {
                return;
            };

            let _response = UpdateVoiceChannelUserState::execute(
                &connection,
                &VoiceChannelUserState {
                    is_sound_off,
                    is_mic_off,
                    is_streaming,
                },
            )
            .await;
        })
        .detach();
    }

    pub fn toggle_capture(&mut self, cx: &mut Context<Self>) {
        self.is_capture_enabled = !self.is_capture_enabled;

        if !self.is_playback_enabled && self.is_capture_enabled {
            self.is_playback_enabled = true;

            let playback = self.streaming.get_playback_controller();
            playback.set_enabled(true);
        }

        let capture = self.streaming.get_capture_controller();
        capture.set_enabled(self.is_capture_enabled);

        self.sync_server_state(cx);
    }

    pub fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        self.is_playback_enabled = !self.is_playback_enabled;

        if !self.is_playback_enabled {
            self.is_capture_enabled = false;

            let capture = self.streaming.get_capture_controller();
            capture.set_enabled(false);
        }

        let playback = self.streaming.get_playback_controller();
        playback.set_enabled(self.is_playback_enabled);

        self.sync_server_state(cx);
    }

    pub fn join_voice_channel(
        &mut self,
        id: &VoiceChannelId,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = *id;

        if let Some(channel) = self.get_active_channel()
            && channel.id == id
        {
            return;
        }

        cx.spawn(async move |this, cx| {
            let Some((user_id, server_ip, connection)) = this
                .read_with(cx, |this, _cx| {
                    (
                        this.user_id,
                        this.rpc_info.server_ip.clone(),
                        this.rpc.clone(),
                    )
                })
                .ok()
            else {
                return;
            };

            let _response =
                JoinVoiceChannel::execute(&connection, &JoinVoiceChannelPayload { channel_id: id })
                    .await;

            Self::fetch_channels_inner(&this, cx).await;
            this.update(cx, |this, cx| {
                let streaming = this.streaming.clone();

                if let Some(channel) = this.get_voice_channel_mut(id) {
                    channel.is_active = true;

                    for member in channel.members.iter_mut() {
                        if member.id == user_id {
                            continue;
                        }

                        member.register(&streaming, cx);
                    }
                }

                this.streaming
                    .connect(user_id, format!("{server_ip}:9899").parse().unwrap());

                let capture = this.streaming.get_capture_controller();
                capture.set_enabled(this.is_capture_enabled);

                let playback = this.streaming.get_playback_controller();
                playback.set_enabled(this.is_playback_enabled);

                this.sync_server_state(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn leave_voice_channel(&mut self, cx: &mut Context<Self>) {
        let user_id = self.user_id;
        let Some(channel) = self.get_active_channel_mut() else {
            return;
        };

        channel.is_active = false;
        channel.members.retain(|member| member.id != user_id);

        self.streaming.disconnect();

        cx.spawn(async |this, cx| {
            let Some(connection) = this.read_with(cx, |this, _cx| this.rpc.clone()).ok() else {
                return;
            };

            let _ = LeaveVoiceChannel::execute(&connection, &Empty).await;
        })
        .detach();
    }

    async fn fetch_channels_inner(this: &WeakEntity<Self>, cx: &mut AsyncApp) {
        let Some(connection) = this.read_with(cx, |this, _cx| this.rpc.clone()).ok() else {
            return;
        };

        let response = GetVoiceChannels::execute(&connection, &Empty).await;

        let Ok(channels) = response else {
            // TODO: Send notification with an error
            return;
        };

        this.update(cx, move |this, cx| {
            this.voice_channels = channels
                .into_iter()
                .map(|channel| VoiceChannel {
                    id: channel.id,
                    name: channel.name.into(),
                    is_active: false,
                    members: channel
                        .members
                        .into_iter()
                        .map(|member| {
                            VoiceChannelMember::new(member.id, member.name.into(), member.state, cx)
                        })
                        .collect(),
                })
                .collect();
        })
        .ok();
    }

    pub fn fetch_voice_channels(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async |this, cx| {
            Self::fetch_channels_inner(&this, cx).await;
        })
        .detach();
    }

    pub fn watch_voice_channel_updates(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let Some(connection) = this.read_with(cx, |this, _cx| this.rpc.clone()).ok() else {
                return;
            };

            let mut subscription = connection.subscribe::<VoiceChannelUpdate>();
            while let Some(event) = subscription.recv().await {
                let channel_id = event.channel_id;
                let channel = this
                    .read_with(cx, |this, _cx| this.get_voice_channel(channel_id).cloned())
                    .unwrap();

                let Some(channel) = channel else {
                    // If there's no such channel, fetch updates
                    // and skip processing
                    let active_channel = this
                        .read_with(cx, |this, _cx| this.get_active_channel().cloned())
                        .unwrap();

                    Self::fetch_channels_inner(&this, cx).await;

                    if let Some(channel) = active_channel {
                        this.update(cx, move |this, cx| {
                            if let Some(channel) = this.get_voice_channel_mut(channel.id) {
                                channel.is_active = true;

                                cx.notify();
                            }
                        })
                        .ok();
                    }

                    continue;
                };

                match event.message {
                    VoiceChannelUpdateMessage::UserConnected(user_id) => {
                        // If user is already present, skip processing
                        let is_present = channel.members.iter().any(|user| user.id == user_id);

                        if is_present {
                            continue;
                        }

                        let user =
                            GetUserInfo::execute(&connection, &GetUserPayload { id: user_id })
                                .await;

                        let Ok(Some(user)) = user else {
                            continue;
                        };

                        this.update(cx, |this, cx| {
                            let streaming = this.streaming.clone();
                            let Some(channel) = this.get_voice_channel_mut(channel_id) else {
                                return;
                            };

                            let mut member = VoiceChannelMember::new(
                                user.id,
                                user.username.into(),
                                VoiceChannelUserState::default(),
                                cx,
                            );

                            if channel.is_active {
                                member.register(&streaming, cx);
                            }

                            channel.members.push(member);

                            cx.notify();
                        })
                        .ok();
                    }
                    VoiceChannelUpdateMessage::UserDisconnected(user_id) => {
                        this.update(cx, |this, cx| {
                            let Some(channel) = this.get_voice_channel_mut(channel_id) else {
                                return;
                            };

                            channel.members.retain(|user| user.id != user_id);

                            cx.notify();
                        })
                        .ok();
                    }
                    VoiceChannelUpdateMessage::UserStateUpdated((user_id, state)) => {
                        this.update(cx, |this, cx| {
                            let Some(channel) = this.get_voice_channel_mut(channel_id) else {
                                return;
                            };

                            if let Some(user) =
                                channel.members.iter_mut().find(|user| user.id == user_id)
                            {
                                user.state = state;

                                cx.notify();
                            }
                        })
                        .ok();
                    }
                }
            }
        })
        .detach();
    }

    pub fn watch_streaming_state_updates(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut subscription = this
                .read_with(cx, |this, _cx| this.streaming.get_device_registry())
                .unwrap()
                .subscribe();

            loop {
                let registry = subscription.recv().await;

                let input = registry.get_input_devices();
                let output = registry.get_output_devices();

                this.update(cx, move |this, cx| {
                    this.input_devices = input;
                    this.output_devices = output;

                    cx.notify();
                })
                .ok();
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            let Some(self_id) = this.read_with(cx, |this, _cx| this.user_id).ok() else {
                return;
            };

            // Because we don't need to fetch this status very often
            let mut timer = smol::Timer::interval(Duration::from_millis(100));

            loop {
                timer.next().await;

                this.update(cx, |this, cx| {
                    let mut updated = false;
                    let capture_enabled = this.is_capture_enabled;

                    let streaming = this.streaming.clone();
                    if let Some(channel) = this.get_active_channel_mut() {
                        for member in channel.members.iter_mut() {
                            if member.id == self_id && !capture_enabled {
                                member.is_talking = false;
                            } else {
                                updated =
                                    member.fetch_is_talking(self_id, &streaming, cx) || updated;
                            }
                        }
                    }

                    if updated {
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    #[cfg(target_os = "linux")]
    pub fn start_screencast(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async |this, cx| {
            let Some((connection, streaming)) = this
                .read_with(cx, |this, _cx| (this.rpc.clone(), this.streaming.clone()))
                .ok()
            else {
                return;
            };

            if let Some(mut preview) = streaming.start_screencast().await {
                // Wait for the first frame to get width and height
                // TODO: Do it properly. Portal should report the size?
                let (width, height) = loop {
                    if let Some(frame) = preview.recv().await {
                        break (frame.width, frame.height);
                    };
                };

                let Ok(_params) = StartScreenCast::execute(
                    &connection,
                    &StartScreenCastRequest { width, height },
                )
                .await
                else {
                    streaming.stop_screencast().await;

                    cx.window_handle()
                        .update(cx, |_, window, cx| {
                            window.push_notification(
                                Notification::error(
                                    "Unable to start the screencast, the server returned an error",
                                ),
                                cx,
                            );
                        })
                        .ok();

                    return;
                };

                // TODO: Update stream params

                this.update(cx, move |this, cx| {
                    this.set_screencast_preview(preview, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    #[cfg(target_os = "linux")]
    async fn stop_screencast_inner(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext) {
        let is_streaming = this
            .read_with(cx, |this, _cx| this.screencast_preview_task.is_some())
            .unwrap();

        if !is_streaming {
            return;
        }

        let Some((connection, streaming)) = this
            .read_with(cx, |this, _cx| (this.rpc.clone(), this.streaming.clone()))
            .ok()
        else {
            return;
        };

        streaming.stop_screencast().await;

        this.update(cx, |this, _cx| {
            this.preview_frame = None;
            this.screencast_preview_task = None;
        })
        .ok();

        if StopScreenCast::execute(&connection, &Empty).await.is_err() {
            cx.window_handle()
                .update(cx, |_, window, cx| {
                    window.push_notification(
                        Notification::error(
                            "Unable to stop the screencast, the server returned an error",
                        ),
                        cx,
                    );
                })
                .ok();
        };
    }

    #[cfg(target_os = "linux")]
    pub fn stop_screencast(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async |this, cx| {
            Self::stop_screencast_inner(&this, cx).await
        })
        .detach();
    }

    #[cfg(target_os = "linux")]
    pub fn set_screencast_preview(
        &mut self,
        mut preview: FrameRecv<gpui::DMABuffer>,
        cx: &mut Context<Self>,
    ) {
        self.screencast_preview_task = Some(cx.spawn(async move |this, cx| {
            while let Some(frame) = preview.recv().await {
                this.update(cx, |this, cx| {
                    this.preview_frame = Some(frame);

                    cx.notify();
                })
                .ok();
            }
        }));
    }

    #[cfg(target_os = "linux")]
    pub fn join_screencast(&self, user_id: UserId, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            Self::stop_screencast_inner(&this, cx).await;

            let Some((connection, streaming)) = this
                .read_with(cx, |this, _cx| (this.rpc.clone(), this.streaming.clone()))
                .ok()
            else {
                return;
            };

            match JoinScreenCast::execute(&connection, &JoinScreenCastRequest { user_id, mtu: 0 })
                .await
            {
                Ok(params) => {
                    let (frame_tx, mut frame_rx) = frame_channel();

                    let this = this.upgrade().unwrap();
                    this.update(cx, |this, cx| {
                        this.watching_frame_task = Some(cx.spawn(async move |this, cx| {
                            while let Some(frame) = frame_rx.recv().await {
                                this.update(cx, |this, cx| {
                                    this.watching_frame = Some(frame);

                                    cx.notify();
                                })
                                .ok();
                            }
                        }));
                    });

                    streaming.register_video_stream(user_id, frame_tx, params);

                    _ = RequestIDRFrame::execute(&connection, &RequestIDRFramePayload { user_id })
                        .await;
                }
                Err(err) => {
                    println!("Error: {err:?}");

                    cx.window_handle()
                        .update(cx, |_, window, cx| {
                            window.push_notification(
                                Notification::error(
                                    "Unable to join the screencast, the server returned an error",
                                ),
                                cx,
                            );
                        })
                        .ok();
                }
            }
        })
        .detach();
    }

    #[cfg(target_os = "linux")]
    pub fn leave_screencast(&self, user_id: UserId, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let Some(connection) = this.read_with(cx, |this, _cx| this.rpc.clone()).ok() else {
                return;
            };

            if LeaveScreenCast::execute(&connection, &LeaveScreenCastRequest { user_id })
                .await
                .is_err()
            {
                cx.window_handle()
                    .update(cx, |_, window, cx| {
                        window.push_notification(
                            Notification::error(
                                "Unable to leave the screencast, the server returned an error",
                            ),
                            cx,
                        );
                    })
                    .ok();
            };
        })
        .detach();
    }

    #[cfg(target_os = "linux")]
    pub fn is_stream_playing(&self) -> bool {
        self.screencast_preview_task.is_some() || self.watching_frame_task.is_some()
    }
}
