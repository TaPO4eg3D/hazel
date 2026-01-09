use gpui::{ParentElement, Render, Styled, div, px, rgb, white};
use gpui_component::{StyledExt, scroll::ScrollableElement};

use crate::components::left_sidear::{TextChannel, TextChannelsComponent};

pub struct WorkspaceScreen {
    text_channels: Vec<TextChannel>,
    voice_channels: Vec<u8>,
}

impl WorkspaceScreen {
    pub fn new() -> Self {
        Self {
            text_channels: (0..3).map(|i| {
                TextChannel {
                    name: format!("Text Channel {i}").into(),
                    is_muted: i % 2 == 0,
                    has_unread: i % 3 == 0,
                }
            }).collect(),
            voice_channels: Vec::new(),
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
    fn render(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
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
                    .w_full()
                    .max_w(px(340.))
                    // Server name header
                    .child(
                        div()
                            .bg(rgb(CARD_BG))
                            .py_4()
                            .px_6()
                            .flex()
                            .child("SERVER NAME")
                            .child(
                                div()
                                    .ml_auto()
                                    .child("+")
                            )
                    )
                    // Main area
                    .child(
                        div()
                            .px_6()
                            .child(
                                TextChannelsComponent::new(self.text_channels.clone())
                                    .pt(px(35.))
                            )
                            .size_full()
                            .overflow_y_scrollbar()
                    )
            )
            // Message area
            .child(
                div()
                    .w_full()
                    .child("456")
            )
            // Right sidebar
            .child(
                div()
                    .bg(rgb(0x181B25))
                    .w_full()
                    .max_w(px(220.))
                    .child("789")
            )
    }
}
