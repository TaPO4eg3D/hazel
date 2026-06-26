use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use atomic_enum::atomic_enum;
use capture::{
    audio::{AudioDevice, playback::AudioStreamingClientSharedState},
    video::linux::screengrab::ScreencastPreview,
};
use gpui::{
    AppContext, AsyncApp, Context, DMABuffer, Entity, SharedString, Subscription, Task, WeakEntity,
    Window,
};
use gpui_component::{
    WindowExt,
    notification::Notification,
    slider::{SliderEvent, SliderState, SliderValue},
};
use rpc::{
    common::Empty,
    models::{
        auth::{GetUserInfo, GetUserPayload},
        common::RPCMethod as _,
        markers::{UserId, VoiceChannelId},
        voice::{
            GetVoiceChannels, JoinVoiceChannel, JoinVoiceChannelPayload, LeaveVoiceChannel,
            StartScreenCast, StopScreenCast, UpdateVoiceChannelUserState, VoiceChannelUpdate,
            VoiceChannelUpdateMessage, VoiceChannelUserState,
        },
    },
};
use smol::stream::StreamExt as _;

use crate::{ConnectionManger, streaming::Streaming};

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
        cx: &mut Context<StreamingState>,
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

    pub fn fetch_is_talking(&mut self, cx: &Context<StreamingState>) -> bool {
        let current = self.is_talking;
        let current_user = ConnectionManger::get_user_id(cx);

        self.is_talking = if let Some(user) = current_user
            && user == self.id
        {
            Streaming::is_talking(cx)
        } else if let Some(state) = self.shared.as_ref() {
            state.read(cx).playback.is_talking.load(Ordering::Relaxed)
        } else {
            false
        };

        self.is_talking != current
    }

    pub fn register(&mut self, cx: &mut Context<StreamingState>) {
        let playback_state = Arc::new(AudioStreamingClientSharedState::new(self.id.value));

        let subscription = cx.subscribe(&self.output_volume, {
            let playback_state = playback_state.clone();

            move |_, _, ev, _| {
                let SliderEvent::Change(value) = ev;
                let SliderValue::Single(value) = value else {
                    return;
                };

                playback_state
                    .volume
                    .store((*value / 100.).powf(3.), Ordering::Relaxed);
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

        Streaming::add_voice_member(cx, Arc::downgrade(&playback_state));
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

pub struct StreamingState {
    pub voice_channels: Vec<VoiceChannel>,

    pub capture_volume: Entity<SliderState>,
    pub playback_volume: Entity<SliderState>,

    pub is_capture_enabled: bool,
    pub is_playback_enabled: bool,

    pub input_devices: Vec<AudioDevice>,
    pub output_devices: Vec<AudioDevice>,

    noise_reduction: NoiseReductionAlgorithm,

    screencast_preview_task: Option<Task<()>>,
    pub preview_frame: Option<DMABuffer>,
}

impl StreamingState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let state = Self {
            screencast_preview_task: None,
            preview_frame: None,

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

            noise_reduction: NoiseReductionAlgorithm::RNNoise,
        };

        cx.subscribe(&state.capture_volume, |_, state, _, cx| {
            let state = state.read(cx);

            if let SliderValue::Single(value) = state.value() {
                Streaming::set_input_volume_modifier(cx, (value / 100.).powf(3.));
            }
        })
        .detach();

        cx.subscribe(&state.playback_volume, |_, state, _, cx| {
            let state = state.read(cx);

            if let SliderValue::Single(value) = state.value() {
                Streaming::set_output_volume_modifier(cx, (value / 100.).powf(3.));
            }
        })
        .detach();

        state
    }
}

impl StreamingState {
    pub fn noise_reduction(&self) -> NoiseReductionAlgorithm {
        self.noise_reduction
    }

    pub fn set_noise_reduction(
        &mut self,
        noise_reduction: NoiseReductionAlgorithm,
        cx: &mut Context<Self>,
    ) {
        self.noise_reduction = noise_reduction;
        Streaming::set_noise_reduction(noise_reduction, cx);

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
            let connection = ConnectionManger::get(cx);

            let Some((is_sound_off, is_mic_off, is_streaming)) = this
                .read_with(cx, |this, _cx| {
                    (
                        !this.is_playback_enabled,
                        !this.is_capture_enabled,
                        this.preview_frame.is_some(),
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

            let playback = Streaming::get_playback(cx);
            playback.set_enabled(true);
        }

        let capture = Streaming::get_capture(cx);
        capture.set_enabled(self.is_capture_enabled);

        self.sync_server_state(cx);
    }

    pub fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        self.is_playback_enabled = !self.is_playback_enabled;

        if !self.is_playback_enabled {
            self.is_capture_enabled = false;

            let capture = Streaming::get_capture(cx);
            capture.set_enabled(false);
        }

        let playback = Streaming::get_playback(cx);
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
            let user_id = ConnectionManger::get_user_id(cx);
            let connection = ConnectionManger::get(cx);

            let _response =
                JoinVoiceChannel::execute(&connection, &JoinVoiceChannelPayload { channel_id: id })
                    .await;

            Self::fetch_channels_inner(&this, cx).await;
            this.update(cx, |this, cx| {
                if let Some(channel) = this.get_voice_channel_mut(id) {
                    channel.is_active = true;

                    for member in channel.members.iter_mut() {
                        if let Some(user_id) = user_id {
                            if member.id == user_id {
                                continue;
                            }
                        }

                        member.register(cx);
                    }
                }

                cx.notify();
            })
            .ok();

            let user_id = ConnectionManger::get_user_id(cx).unwrap();
            let server_ip = ConnectionManger::get_server_ip(cx).unwrap();

            Streaming::connect(cx, user_id, format!("{server_ip}:9899").parse().unwrap());

            this.update(cx, |this, cx| {
                let capture = Streaming::get_capture(cx);
                capture.set_enabled(this.is_capture_enabled);

                let playback = Streaming::get_playback(cx);
                playback.set_enabled(this.is_playback_enabled);

                this.sync_server_state(cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn leave_voice_channel(&mut self, cx: &mut Context<Self>) {
        let Some(channel) = self.get_active_channel_mut() else {
            return;
        };

        let Some(user_id) = ConnectionManger::get_user_id(cx) else {
            return;
        };

        channel.is_active = false;
        channel.members.retain(|member| member.id != user_id);

        Streaming::disconnect(cx);

        cx.spawn(async |_, cx| {
            let connection = ConnectionManger::get(cx);
            let _ = LeaveVoiceChannel::execute(&connection, &Empty {}).await;
        })
        .detach();
    }

    async fn fetch_channels_inner(this: &WeakEntity<Self>, cx: &mut AsyncApp) {
        let connection = ConnectionManger::get(cx);

        let response = GetVoiceChannels::execute(&connection, &Empty {}).await;

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
            let connection = ConnectionManger::get(cx);

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
                                member.register(cx);
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
            let mut subscription = Streaming::get_device_registry(cx).subscribe();

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
            let self_id = ConnectionManger::get_user_id(cx);

            // Because we don't need to fetch this status very often
            let mut timer = smol::Timer::interval(Duration::from_millis(100));

            loop {
                timer.next().await;

                this.update(cx, |this, cx| {
                    let mut updated = false;
                    let capture_enabled = this.is_capture_enabled;

                    if let Some(channel) = this.get_active_channel_mut() {
                        for member in channel.members.iter_mut() {
                            if Some(member.id) == self_id && !capture_enabled {
                                member.is_talking = false;
                            } else {
                                updated = member.fetch_is_talking(cx) || updated;
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

    pub fn start_screencast(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async |this, cx| {
            let connection = ConnectionManger::get(cx);

            let Ok(params) = StartScreenCast::execute(&connection, &Empty::default()).await else {
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

            if let Some(preview) = Streaming::start_screencast(cx, params).await {
                this.update(cx, move |this, cx| {
                    this.set_screencast_preview(preview, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    pub fn stop_screencast(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async |this, cx| {
            let connection = ConnectionManger::get(cx);

            Streaming::stop_screencast(&mut *cx).await;

            this.update(cx, |this, _cx| {
                this.preview_frame = None;
                this.screencast_preview_task = None;
            })
            .ok();

            if StopScreenCast::execute(&connection, &Empty::default())
                .await
                .is_err()
            {
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

                return;
            };
        })
        .detach();
    }

    pub fn set_screencast_preview(&mut self, preview: ScreencastPreview, cx: &mut Context<Self>) {
        self.screencast_preview_task = Some(cx.spawn(async move |this, cx| {
            while let Some(frame) = preview.recv().await {
                this.update(cx, |this, cx| {
                    this.preview_frame = Some(frame);
                    this.sync_server_state(cx);

                    cx.notify();
                })
                .ok();
            }
        }));
    }
}
