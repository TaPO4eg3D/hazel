use gpui::{
    AppContext, Context, Div, Element, Entity, IntoElement, ParentElement, Render, RenderOnce,
    Rgba, Styled, Window, div, px, red, rgb, white,
};
use gpui_component::{Icon, IconName, StyledExt, button::{Button, ButtonVariants}, input::{Input, InputState}, label, text};

pub struct LoginScreen {
    username: Entity<InputState>,
    password: Entity<InputState>,
    server_ip: Entity<InputState>,
}

impl LoginScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            username: cx.new(|cx| InputState::new(window, cx)),
            password: cx.new(|cx| InputState::new(window, cx)),
            server_ip: cx.new(|cx| InputState::new(window, cx)),
        }
    }
}

impl Render for LoginScreen {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .bg(rgb(0x24283D))
            .size_full()
            .v_flex()
            .justify_center()
            .items_center()
            .gap(px(30.))
            .font_family("Inter")
            .text_size(px(18.))
            .child(
                div()
                    .text_color(white())
                    .text_size(px(64.))
                    .font_bold()
                    .child("HAZEL"),
            )
            .child(
                div()
                    .size(px(600.))
                    .bg(rgb(0x181B25))
                    .p(px(30.))
                    .v_flex()
                    .child(
                        div()
                            .text_color(white())
                            .font_bold()
                            .child("USERNAME")
                    )
                    .child(
                        Input::new(&self.username)
                            .mt(px(12.))
                            .min_h(px(55.))
                    )
                    .child(
                        div()
                            .mt(px(25.))
                            .text_color(white())
                            .font_bold()
                            .child("PASSWORD")
                    )
                    .child(
                        Input::new(&self.password)
                            .mt(px(12.))
                            .min_h(px(55.))
                    )
                    .child(
                        div()
                            .mt_auto()
                            .v_flex()
                            .child(
                                div()
                                    .text_color(white())
                                    .font_bold()
                                    .child("SERVER IP")
                            )
                            .child(
                                Input::new(&self.server_ip)
                                    .text_decoration_color(white())
                                    .min_h(px(55.))
                                    .mt(px(12.))
                                    .mb(px(30.))
                                    .prefix(
                                        Icon::new(IconName::User)
                                    )
                            )
                            .child(
                                Button::new("ok")
                                    .h(px(55.))
                                    .primary()
                                    .mt_auto()
                                    .text_color(white())
                                    .font_bold()
                                    .label("LOG IN")
                            )
                    )
            )
    }
}
