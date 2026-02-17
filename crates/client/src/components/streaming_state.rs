use std::{sync::Arc, time::Duration};

use capture::audio::{AudioDevice, playback::AudioStreamingClientSharedState};
use gpui::{AppContext, AsyncApp, Context, Entity, SharedString, WeakEntity, Window};
use gpui_component::slider::{SliderState, SliderValue};
use rpc::{
    common::Empty,
    models::{
        auth::{GetUserInfo, GetUserPayload},
        common::RPCMethod as _,
        markers::{UserId, VoiceChannelId},
        voice::{
            GetVoiceChannels, JoinVoiceChannel, JoinVoiceChannelPayload, UpdateVoiceUserState,
            VoiceChannelUpdate, VoiceChannelUpdateMessage, VoiceUserState,
        },
    },
};
use smol::stream::StreamExt as _;

use crate::{
    ConnectionManger,
    gpui_audio::Streaming,
};

#[derive(Clone)]
pub struct VoiceChannel {
    pub id: VoiceChannelId,
    pub name: SharedString,

    pub is_active: bool,
    pub members: Vec<VoiceChannelMember>,
}

#[derive(Clone)]
pub struct VoiceChannelMember {
    pub id: UserId,
    pub name: SharedString,

    pub is_muted: bool,
    pub is_mic_off: bool,
    pub is_sound_off: bool,
    pub is_streaming: bool,
    pub is_talking: bool,

    shared: Option<Arc<AudioStreamingClientSharedState>>,
}

impl VoiceChannelMember {
    pub fn new(id: UserId, name: SharedString) -> Self {
        VoiceChannelMember {
            id,
            name,
            is_muted: false,
            is_mic_off: false,
            is_sound_off: false,
            is_streaming: false,
            is_talking: false,
            shared: None,
        }
    }

    pub fn fetch_is_talking<C: AppContext>(&mut self, cx: &C) -> bool {
        let current = self.is_talking;
        let current_user = ConnectionManger::get_user_id(cx);

        self.is_talking = if let Some(user) = current_user
            && user == self.id
        {
            Streaming::is_talking(cx)
        } else if let Some(shared) = self.shared.as_ref() {
            // TODO: IMPLEMENT
            // shared.is_talking()
            false
        } else {
            false
        };

        self.is_talking != current
    }

    pub fn register<C: AppContext>(&mut self, cx: &C) {
        let shared = Arc::new(AudioStreamingClientSharedState::new(self.id.value));
        self.shared = Some(shared.clone());

        Streaming::add_voice_member(cx, Arc::downgrade(&shared));
    }

    pub fn unregister(&mut self) {
        self.shared = None;
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
}

impl StreamingState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let state = Self {
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
        };

        cx.subscribe(&state.capture_volume, |_, state, _, cx| {
            let state = state.read(cx);

            if let SliderValue::Single(value) = state.value() {
                Streaming::set_input_volume_modifier(cx, value / 100.);
            }
        })
        .detach();

        cx.subscribe(&state.playback_volume, |_, state, _, cx| {
            let state = state.read(cx);

            if let SliderValue::Single(value) = state.value() {
                Streaming::set_output_volume_modifier(cx, value / 100.);
            }
        })
        .detach();

        state
    }
}

impl StreamingState {
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

            let Some((is_sound_off, is_mic_off)) = this
                .read_with(cx, |this, _cx| {
                    (!this.is_playback_enabled, !this.is_capture_enabled)
                })
                .ok()
            else {
                return;
            };

            let _response = UpdateVoiceUserState::execute(
                &connection,
                &VoiceUserState {
                    is_sound_off,
                    is_mic_off,
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
            let connection = ConnectionManger::get(cx);

            let _response =
                JoinVoiceChannel::execute(&connection, &JoinVoiceChannelPayload { channel_id: id })
                    .await;

            Self::fetch_channels_inner(&this, cx).await;
            this.update(cx, |this, cx| {
                if let Some(channel) = this.get_voice_channel_mut(id) {
                    channel.is_active = true;

                    for member in channel.members.iter_mut() {
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

    async fn fetch_channels_inner(this: &WeakEntity<Self>, cx: &mut AsyncApp) {
        let connection = ConnectionManger::get(cx);

        let response = GetVoiceChannels::execute(&connection, &Empty {}).await;

        let Ok(channels) = response else {
            // TODO: Send notification with an error
            return;
        };

        this.update(cx, move |this, _cx| {
            this.voice_channels = channels
                .into_iter()
                .map(|channel| VoiceChannel {
                    id: channel.id,
                    name: channel.name.into(),
                    is_active: false,
                    members: channel
                        .members
                        .into_iter()
                        .map(|member| VoiceChannelMember::new(member.id, member.name.into()))
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

                            let mut member = VoiceChannelMember::new(user.id, user.username.into());

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
                                user.is_mic_off = state.is_mic_off;
                                user.is_sound_off = state.is_sound_off;

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
}
