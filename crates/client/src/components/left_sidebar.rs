use std::{fmt::format, sync::Arc};

use gpui::{
    AnyElement, App, ElementId, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    RenderOnce, SharedString, StatefulInteractiveElement, Style, StyleRefinement, Styled, Window,
    div, percentage, prelude::FluentBuilder, px, red, rgb, white,
};
use gpui_component::{ActiveTheme, Icon, Sizable, Size, StyledExt, button::Button};
use rpc::models::markers::{UserId, VoiceChannelId};

use crate::assets::IconName;

#[derive(Clone)]
pub struct TextChannel {
    pub id: u64,
    pub name: SharedString,

    pub is_active: bool,
    pub is_muted: bool,
    pub has_unread: bool,
}

#[derive(IntoElement)]
pub struct TextChannelsComponent {
    channels: Vec<TextChannel>,
    style: StyleRefinement,
}

impl TextChannelsComponent {
    pub fn new(channels: Vec<TextChannel>) -> Self {
        Self {
            channels,
            style: StyleRefinement::default(),
        }
    }
}

impl Styled for TextChannelsComponent {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

#[derive(IntoElement)]
pub struct CollapasableCard {
    id: ElementId,

    title: Option<SharedString>,
    content: Option<AnyElement>,

    style: StyleRefinement,

    is_collapsed: bool,

    #[allow(clippy::type_complexity)]
    on_toggle_click: Option<Arc<dyn Fn(&bool, &mut Window, &mut App) + Send + Sync>>,
}

impl Styled for CollapasableCard {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl CollapasableCard {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            title: None,
            content: None,
            style: StyleRefinement::default(),
            is_collapsed: false,
            on_toggle_click: None,
        }
    }

    pub fn title(mut self, value: impl Into<SharedString>) -> Self {
        self.title = Some(value.into());
        self
    }

    pub fn content(mut self, element: impl IntoElement) -> Self {
        self.content = Some(element.into_any_element());
        self
    }

    pub fn collapsed(mut self, value: bool) -> Self {
        self.is_collapsed = value;
        self
    }

    pub fn on_toggle_click(
        mut self,
        f: impl Fn(&bool, &mut Window, &mut App) + Send + Sync + 'static,
    ) -> Self {
        self.on_toggle_click = Some(Arc::new(f));
        self
    }
}

impl RenderOnce for CollapasableCard {
    fn render(self, _: &mut gpui::Window, _: &mut App) -> impl IntoElement {
        div()
            .refine_style(&self.style)
            .child(
                div()
                    .flex()
                    .items_center()
                    .when_some(self.title, |this, title| this.child(title))
                    .child(
                        Button::new(self.id)
                            .icon({
                                if self.is_collapsed {
                                    Icon::new(IconName::ChevronsUpDown)
                                } else {
                                    Icon::new(IconName::ChevronsDownUp)
                                }
                            })
                            .cursor_pointer()
                            .when_some(self.on_toggle_click, {
                                let is_collapsed = !self.is_collapsed;

                                move |this, on_toggle_click| {
                                    this.on_click(move |_, window, app| {
                                        on_toggle_click(&is_collapsed, window, app)
                                    })
                                }
                            })
                            .ml_auto(),
                    ),
            )
            .when(!self.is_collapsed, |this| {
                this.when_some(self.content, |this, content| {
                    this.child(div().mt_4().child(content))
                })
            })
    }
}

impl RenderOnce for TextChannelsComponent {
    fn render(self, _: &mut gpui::Window, _: &mut gpui::App) -> impl gpui::IntoElement {
        let channels = self.channels.iter().map(|channel| {
            div()
                .child(
                    div()
                        .id(ElementId::Integer(channel.id))
                        .bg(rgb(0x0F111A))
                        .border_2()
                        .hover(|style| style.border_color(rgb(0x7B5CFF)))
                        .cursor_pointer()
                        .when(channel.is_active, |this| {
                            this.border_color(rgb(0x7B5CFF)).border_2()
                        })
                        .items_center()
                        .rounded_lg()
                        .flex()
                        .child(Icon::new(IconName::Hash).ml_3())
                        .child(
                            div()
                                .py_3()
                                .ml_3()
                                .font_normal()
                                .text_size(px(14.))
                                .child(channel.name.clone()),
                        )
                        .when(channel.is_muted, |this| {
                            this.child(Icon::new(IconName::MessageCircleOff).ml_auto().mr_3())
                        }),
                )
                // cuz we need to draw this dot above border
                .when(channel.has_unread, |this| {
                    this.relative().child(
                        div()
                            .bg(rgb(0xFF8800))
                            .absolute()
                            .top_neg_1()
                            .right_0()
                            .w_2()
                            .h_2()
                            .rounded_xl(),
                    )
                })
        });

        div()
            .id("text-channels")
            .v_flex()
            .gap_2()
            .children(channels)
    }
}

#[derive(Clone)]
pub struct VoiceChannelMember {
    pub id: UserId,
    pub name: SharedString,

    pub is_muted: bool,
    pub is_talking: bool,
    pub is_streaming: bool,
}

#[derive(Clone)]
pub struct VoiceChannel {
    pub id: VoiceChannelId,
    pub name: SharedString,

    pub is_active: bool,
    pub members: Vec<VoiceChannelMember>,
}

#[derive(IntoElement)]
pub struct VoiceChannelsComponent {
    channels: Vec<VoiceChannel>,
    style: StyleRefinement,

    #[allow(clippy::type_complexity)]
    on_select: Option<Arc<dyn Fn(&VoiceChannelId, &mut Window, &mut App) + Send + Sync>>,
}

impl Styled for VoiceChannelsComponent {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl VoiceChannelsComponent {
    pub fn new(channels: Vec<VoiceChannel>) -> Self {
        Self {
            channels,
            style: StyleRefinement::default(),
            on_select: None,
        }
    }

    pub fn on_select(
        mut self,
        f: impl Fn(&VoiceChannelId, &mut Window, &mut App) + Send + Sync + 'static,
    ) -> Self {
        self.on_select = Some(Arc::new(f));

        self
    }
}

#[derive(IntoElement)]
pub struct IconRoundedButton {
    id: ElementId,
    content: Option<Icon>,

    style: StyleRefinement,

    #[allow(clippy::type_complexity)]
    on_click: Option<Arc<dyn Fn(&(), &mut Window, &mut App) + Send + Sync>>,
}

impl Styled for IconRoundedButton {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl IconRoundedButton {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            content: None,
            style: StyleRefinement::default(),
            on_click: None,
        }
    }

    pub fn on_click(
        mut self,
        on_click: impl Fn(&(), &mut Window, &mut App) + Send + Sync + 'static,
    ) -> Self {
        self.on_click = Some(Arc::new(on_click));
        self
    }

    pub fn content(mut self, value: Icon) -> Self {
        self.content = Some(value);
        self
    }
}

impl RenderOnce for IconRoundedButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let mut hover_bg = cx.theme().secondary;
        hover_bg.a = 0.8;

        div()
            .id(self.id.clone())
            .p_1()
            .hover(|style| style.bg(hover_bg))
            .cursor_pointer()
            .rounded_3xl()
            .when_some(self.content, |this, content| this.child(content))
            .refine_style(&self.style)
    }
}

impl RenderOnce for VoiceChannelsComponent {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let channels = self.channels.iter().map(|channel| {
            let members = channel.members.iter().map(|member| {
                div()
                    .id(ElementId::Integer(member.id.value as u64))
                    .flex()
                    .items_center()
                    .child(div().child(Icon::new(IconName::User)))
                    .child(div().ml_1().mt(px(1.)).child(member.name.clone()))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .ml_1()
                            .when(member.is_talking, |this| {
                                this.child(Icon::new(IconName::Mic).text_color(rgb(0x4AC6FF)))
                            })
                            .when(member.is_streaming, |this| {
                                this.child(Icon::new(IconName::Cast).text_color(rgb(0x4AC6FF)))
                            }),
                    )
                    .child(
                        div().ml_auto().child(
                            IconRoundedButton::new("options")
                                .content(Icon::new(IconName::EllipsisVertical)),
                        ),
                    )
            });

            div().child(
                div()
                    .id(ElementId::Integer(channel.id.value as u64))
                    .p_3()
                    .bg(rgb(0x0F111A))
                    .text_size(px(16.))
                    .font_normal()
                    .rounded_lg()
                    .border_2()
                    .hover(|style| style.border_color(rgb(0x7B5CFF)))
                    .when(channel.is_active, |this| {
                        this.border_color(rgb(0x7B5CFF)).border_2()
                    })
                    .when(!channel.is_active, |this| {
                        this.cursor_pointer().when_some(self.on_select.clone(), {
                            let id = channel.id;

                            move |this, on_select| {
                                this.on_click(move |_, window, app| {
                                    on_select(&id, window, app);
                                })
                            }
                        })
                    })
                    .v_flex()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(Icon::new(IconName::AudioLines))
                            .child(div().ml_2().child(channel.name.clone()))
                            .child(div().ml_auto().child(Icon::new(IconName::Users)))
                            .child(div().ml_1().child(format!("{}", channel.members.len()))),
                    )
                    .child(div().py_2().v_flex().gap_1().children(members)),
            )
        });

        div()
            .id("voice-channels")
            .v_flex()
            .gap_3()
            .children(channels)
    }
}

#[derive(IntoElement)]
pub struct ControlPanel {
    is_connected: bool,
    style: StyleRefinement,
}

impl ControlPanel {
    pub fn new() -> Self {
        Self {
            is_connected: false,
            style: StyleRefinement::default(),
        }
    }

    pub fn is_connected(mut self, value: bool) -> Self {
        self.is_connected = value;
        self
    }
}

impl Default for ControlPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl Styled for ControlPanel {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for ControlPanel {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .id("control-panel")
            .bg(rgb(0x0F111A))
            .child(
                div()
                    .px_2()
                    .py_4()
                    .flex()
                    .when(self.is_connected, |this| {
                        this
                            .child(
                                IconRoundedButton::new("disconnect")
                                    .content(Icon::new(IconName::PhoneOff).with_size(Size::Large)),
                            )
                            .child(div().w(px(1.)).h(px(32.0)).bg(white()).mx_2())
                            .child(
                                IconRoundedButton::new("mic-mute")
                                    .content(Icon::new(IconName::MicOff).with_size(Size::Large)),
                            )
                            .child(
                                IconRoundedButton::new("sound-mute").content(
                                    Icon::new(IconName::HeadphoneOff).with_size(Size::Large),
                                ),
                            )
                            .child(
                                IconRoundedButton::new("cast")
                                    .content(Icon::new(IconName::Cast).with_size(Size::Large))
                                    .ml_auto(),
                            )
                            .child(div().w(px(1.)).h(px(32.0)).bg(white()).mx_2())
                    })
                    .child(
                        IconRoundedButton::new("settings")
                            .content(Icon::new(IconName::Settings).with_size(Size::Large))
                            .when(!self.is_connected, |this| {
                                this.ml_auto()
                            })
                    )
            )
            .refine_style(&self.style)
    }
}
