use gpui::{ParentElement, Render, Styled, div, px, rgb, white};
use gpui_component::{
    Icon, Sizable, Size, StyledExt, accordion::Accordion, scroll::ScrollableElement, v_flex,
};

use crate::{
    assets::IconName,
    components::left_sidear::{
        CollapasableCard, ControlPanel, TextChannel, TextChannelsComponent, VoiceChannel,
        VoiceChannelMember, VoiceChannelsComponent,
    },
};

pub struct WorkspaceScreen {
    text_channels: Vec<TextChannel>,
    voice_channels: Vec<VoiceChannel>,

    text_channels_collapsed: bool,
    voice_channels_collapsed: bool,
}

impl WorkspaceScreen {
    pub fn new() -> Self {
        Self {
            text_channels: (0..10)
                .map(|i| TextChannel {
                    name: format!("Text Channel {i}").into(),
                    is_active: i == 0,
                    is_muted: i % 2 == 0,
                    has_unread: i % 3 == 0,
                })
                .collect(),
            voice_channels: vec![
                VoiceChannel {
                    id: 1,
                    is_active: true,
                    name: "Counter Stike 2".into(),
                    members: vec![
                        VoiceChannelMember {
                            id: 1,
                            name: "TaPO4eg3D".into(),
                            is_muted: false,
                            is_talking: true,
                            is_streaming: false,
                        },
                        VoiceChannelMember {
                            id: 2,
                            name: "syliakPro".into(),
                            is_muted: false,
                            is_talking: false,
                            is_streaming: true,
                        },
                        VoiceChannelMember {
                            id: 3,
                            name: "PARAZAKATAFANNY".into(),
                            is_muted: true,
                            is_talking: false,
                            is_streaming: false,
                        },
                        VoiceChannelMember {
                            id: 4,
                            name: "daniql1".into(),
                            is_muted: false,
                            is_talking: true,
                            is_streaming: true,
                        },
                    ],
                },
                VoiceChannel {
                    id: 2,
                    name: "Default".into(),
                    is_active: false,
                    members: vec![],
                },
            ],

            text_channels_collapsed: false,
            voice_channels_collapsed: false,
        }
    }
}

impl Default for WorkspaceScreen {
    fn default() -> Self {
        Self::new()
    }
}

const CARD_BG: u32 = 0x0F111A;

impl Render for WorkspaceScreen {
    fn render(
        &mut self,
        window: &mut gpui::Window,
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
                        div()
                            .size_full()
                            .overflow_hidden()
                            .child(
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
                                            .content(VoiceChannelsComponent::new(
                                                self.voice_channels.clone(),
                                            ))
                                            .pt_6()
                                            .mb_2()
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
