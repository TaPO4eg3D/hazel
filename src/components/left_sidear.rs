use std::sync::Arc;

use gpui::{
    AnyElement, App, Entity, IntoElement, ParentElement, Render, RenderOnce, SharedString, Style, StyleRefinement, Styled, Window, div, prelude::FluentBuilder, px, rgb
};
use gpui_component::{Icon, StyledExt, button::Button};

use crate::assets::IconName;

#[derive(Clone)]
pub struct TextChannel {
    pub name: SharedString,

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
    title: SharedString,
    content: Option<AnyElement>,

    style: StyleRefinement,

    is_collapsed: bool,

    #[allow(clippy::type_complexity)]
    on_toggle_click: Option<Arc<dyn Fn(&bool, &mut Window, &mut App) + Send + Sync>>,
}

impl Styled for CollapasableCard {
    fn style(&mut self) ->  &mut StyleRefinement {
        &mut self.style
    }
}

impl CollapasableCard {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            content: None,
            style: StyleRefinement::default(),
            is_collapsed: false,
            on_toggle_click: None,
        }
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
    fn render(self, window: &mut gpui::Window, cx: &mut App) -> impl IntoElement {
        div()
            .refine_style(&self.style)
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(self.title.clone())
                    .child(
                        Button::new("collapse")
                            .icon({
                                if self.is_collapsed {
                                    Icon::new(IconName::ChevronsUpDown)
                                } else {
                                    Icon::new(IconName::ChevronsDownUp)
                                }
                            })
                            .when_some(self.on_toggle_click, {
                                let is_collapsed = !self.is_collapsed;

                                move |this, on_toggle_click| {
                                    this.on_click(move |_, window, app| {
                                        on_toggle_click(&is_collapsed, window, app)
                                    })
                                }
                            })
                            .ml_auto()

                    )
            )
            .when(!self.is_collapsed, |this| {
                this.when_some(self.content, |this, content| {
                    this.child(content)
                })
            })
    }
}

impl RenderOnce for TextChannelsComponent {
    fn render(self, _: &mut gpui::Window, _: &mut gpui::App) -> impl gpui::IntoElement {
        let channels = self.channels.iter().map(|channel| {
            div()
                .bg(rgb(0x0F111A))
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
                .child(Icon::new(IconName::MessageCircleOff).ml_auto().mr_3())
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

        div().mt_4().v_flex().gap_2().children(channels)
    }
}
