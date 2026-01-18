use gpui::{
    AppContext, AsyncApp, Context, Entity, ParentElement, Render, Styled, WeakEntity, Window, div,
    px, rgb, white,
};
use gpui_component::{
    Icon, Sizable, Size, StyledExt, accordion::Accordion, scroll::ScrollableElement, v_flex,
};
use rpc::{
    common::Empty,
    models::{
        common::{APIResult, RPCMethod},
        markers::{Id, VoiceChannelId},
        voice::{GetVoiceChannels, JoinVoiceChannel, JoinVoiceChannelError, JoinVoiceChannelPayload, VoiceChannelUpdate, VoiceChannelUpdateMessage},
    },
};

use crate::{
    ConnectionManger, MainWindow,
    assets::IconName,
    components::{
        chat::Chat,
        left_sidebar::{
            CollapasableCard, ControlPanel, TextChannel, TextChannelsComponent, VoiceChannel,
            VoiceChannelMember, VoiceChannelsComponent,
        },
    },
};

pub struct WorkspaceScreen {
    text_channels: Vec<TextChannel>,
    voice_channels: Vec<VoiceChannel>,

    text_channels_collapsed: bool,
    voice_channels_collapsed: bool,

    chat: Entity<Chat>,
}

impl WorkspaceScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let chat = cx.new(|cx| Chat::new(window, cx));

        Self {
            chat,
            text_channels: (0..2)
                .map(|i| TextChannel {
                    id: i,
                    name: format!("Text Channel {i}").into(),
                    is_active: i == 0,
                    is_muted: i % 2 == 0,
                    has_unread: i % 3 == 0,
                })
                .collect(),
            voice_channels: vec![],

            text_channels_collapsed: false,
            voice_channels_collapsed: false,
        }
    }

    pub fn get_voice_channel(&self, id: VoiceChannelId) -> Option<&VoiceChannel> {
        self.voice_channels
            .iter()
            .find(|channel| channel.id == id)
    }

    pub fn get_voice_channel_mut(&mut self, id: VoiceChannelId) -> Option<&mut VoiceChannel> {
        self.voice_channels
            .iter_mut()
            .find(|channel| channel.id == id)
    }

    pub fn on_voice_channel_select(
        &mut self,
        id: &VoiceChannelId,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = *id;

        cx.spawn(async move |this, cx| {
            let connection = ConnectionManger::get(cx);

            let response = JoinVoiceChannel::execute(&connection, &JoinVoiceChannelPayload {
                channel_id: id
            }).await;

            Self::fetch_channels_inner(&this, cx).await;
            this.update(cx, |this, cx| {
                if let Some(channel) = this.get_voice_channel_mut(id) {
                    channel.is_active = true;
                }

                cx.notify();
            }).ok();
        })
        .detach();
    }

    pub fn watch_for_voice_channels(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |_this, cx| {
            let connection = ConnectionManger::get(cx);

            let mut subscription = connection.subscribe::<VoiceChannelUpdate>("VoiceChannelUpdate");
            while let Some(event) = subscription.recv().await {
                let channel_id = event.channel_id;

                match event.message {
                    VoiceChannelUpdateMessage::UserConnected(user_id) => {
                    },
                    VoiceChannelUpdateMessage::UserDisconnected(user_id) => {
                    },
                }
            }
        })
        .detach();
    }

    async fn fetch_channels_inner(this: &WeakEntity<Self>, cx: &mut AsyncApp) {
        let connection = ConnectionManger::get(cx);

        let response = GetVoiceChannels::execute(&connection, &Empty {})
            .await;

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
                        .map(|member| VoiceChannelMember {
                            id: member.id,
                            name: member.name.into(),
                            is_muted: false,
                            is_talking: false,
                            is_streaming: false,
                        })
                        .collect(),
                })
                .collect();
        })
        .ok();
    }

    pub fn fetch_channels(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async |this, cx| {
            Self::fetch_channels_inner(&this, cx).await;
        }).detach();
    }
}

const CARD_BG: u32 = 0x0F111A;

impl Render for WorkspaceScreen {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        div()
            .bg(rgb(0x24283D))
            .size_full()
            .flex()
            .font_family("Inter")
            .text_color(white())
            .text_size(px(16.))
            .font_bold()
            // Left sidebar
            .child(
                div()
                    .bg(rgb(0x181B25))
                    .size_full()
                    .max_w(px(380.))
                    .v_flex()
                    // Server name header
                    .child(
                        div()
                            .bg(rgb(CARD_BG))
                            .py_4()
                            .px_6()
                            .flex()
                            .items_center()
                            .child("HAZEL OFFICIAL")
                            .child(div().ml_auto().child(Icon::new(IconName::Settings))),
                    )
                    // Main area
                    .child(
                        div().size_full().overflow_hidden().child(
                            v_flex()
                                .px_6()
                                .overflow_y_scrollbar()
                                .child(
                                    CollapasableCard::new("text-channels-card")
                                        .title("TEXT CHANNELS")
                                        .collapsed(self.text_channels_collapsed)
                                        .on_toggle_click(cx.listener(
                                            |this, is_collapsed: &bool, _, cx| {
                                                this.text_channels_collapsed = *is_collapsed;
                                                cx.notify();
                                            },
                                        ))
                                        .content(TextChannelsComponent::new(
                                            self.text_channels.clone(),
                                        ))
                                        .pt_6(),
                                )
                                .child(
                                    CollapasableCard::new("voice-channels-card")
                                        .title("VOICE CHANNELS")
                                        .collapsed(self.voice_channels_collapsed)
                                        .on_toggle_click(cx.listener(
                                            |this, is_collapsed: &bool, _, cx| {
                                                this.voice_channels_collapsed = *is_collapsed;
                                                cx.notify();
                                            },
                                        ))
                                        .content(
                                            VoiceChannelsComponent::new(
                                                self.voice_channels.clone(),
                                            )
                                            .on_select(cx.listener(Self::on_voice_channel_select)),
                                        )
                                        .pt_6()
                                        .mb_2(),
                                ),
                        ),
                    )
                    .child(ControlPanel::new()),
            )
            // Message area
            .child(div().w_full().child("456"))
            // Right sidebar
            .child(
                div()
                    .bg(rgb(0x181B25))
                    .w_full()
                    .max_w(px(220.))
                    .child("789"),
            )
    }
}
