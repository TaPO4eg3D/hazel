use gpui::{
    App, AppContext, ClickEvent, Context, Div, Element, Entity, IntoElement, ParentElement, Render,
    RenderOnce, Rgba, Styled, Window, div, px, red, rgb, white,
};
use gpui_component::{
    Disableable, Icon, Root, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    label, text,
};

use crate::{ConnectionManger, assets::IconName, gpui_tokio::Tokio};

pub struct LoginScreen {
    username: Entity<InputState>,
    password: Entity<InputState>,
    server_address: Entity<InputState>,

    /// Indicates if we're in the process
    /// of connecting to a server
    is_connecting: bool,

    is_login_btn_active: bool,
}

enum ConnectionResult {
    Connected,
    Failed,
}

impl LoginScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let username = cx.new(|cx| InputState::new(window, cx));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let server_address =
            cx.new(|cx| InputState::new(window, cx).default_value("localhost:9898"));

        cx.subscribe_in(&username, window, Self::watch_for_inputs)
            .detach();
        cx.subscribe_in(&password, window, Self::watch_for_inputs)
            .detach();
        cx.subscribe_in(&server_address, window, Self::watch_for_inputs)
            .detach();

        Self {
            username,
            password,
            server_address,

            is_connecting: false,
            is_login_btn_active: false,
        }
    }

    fn watch_for_inputs(
        entity: &mut LoginScreen,
        _state: &Entity<InputState>,
        event: &InputEvent,
        _window: &mut Window,
        cx: &mut Context<LoginScreen>,
    ) {
        if let InputEvent::Change = event {
            let username = entity.username.read(cx).value();
            let password = entity.password.read(cx).value();
            let server_address = entity.server_address.read(cx).value();

            entity.is_login_btn_active =
                !username.is_empty() && !password.is_empty() && !server_address.is_empty();

            cx.notify();
        }
    }

    fn login_btn_click(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let server_ip = self.server_address.read(cx).value();

        self.is_connecting = true;
        cx.notify();

        let (tx, rx) = smol::channel::bounded::<ConnectionResult>(1);
        window
            .spawn(cx, async move |window| {
                let msg = rx.recv().await?;

                window
                    .update(|window, cx| {
                        match msg {
                            ConnectionResult::Connected => {
                                window.push_notification("Success!", cx);
                            }
                            ConnectionResult::Failed => {
                                window.push_notification("Failed to connect!", cx);
                            }
                        };
                    })
                    .ok();

                Ok::<_, anyhow::Error>(())
            })
            .detach();

        cx.spawn(async move |this, cx| {
            // TODO: Properly handle a case when we can't connect
            // ConnectionManger::connect(cx, server_ip.into())
            //     .await
            //     .unwrap();
            tx.send(ConnectionResult::Failed).await?;

            this.update(cx, |this, cx| {
                this.is_connecting = false;
                cx.notify();
            })
            .ok();


            Ok::<_, anyhow::Error>(())
        })
        .detach();
    }
}

impl Render for LoginScreen {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
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
                    .child(div().text_color(white()).font_bold().child("USERNAME"))
                    .child(
                        Input::new(&self.username)
                            .mt(px(12.))
                            .min_h(px(55.))
                            .prefix(Icon::new(IconName::UserAvatar)),
                    )
                    .child(
                        div()
                            .mt(px(25.))
                            .text_color(white())
                            .font_bold()
                            .child("PASSWORD"),
                    )
                    .child(
                        Input::new(&self.password)
                            .mt(px(12.))
                            .min_h(px(55.))
                            .prefix(Icon::new(IconName::PasswordLock))
                            .mask_toggle(),
                    )
                    .child(
                        div()
                            .mt_auto()
                            .v_flex()
                            .child(
                                div()
                                    .text_color(white())
                                    .font_bold()
                                    .child("SERVER ADDRESS"),
                            )
                            .child(
                                Input::new(&self.server_address)
                                    .text_decoration_color(white())
                                    .min_h(px(55.))
                                    .mt(px(12.))
                                    .mb(px(30.))
                                    .prefix(Icon::new(IconName::Server)),
                            )
                            .child(
                                Button::new("ok")
                                    .h(px(55.))
                                    .disabled(!self.is_login_btn_active)
                                    .primary()
                                    .mt_auto()
                                    .text_color(white())
                                    .font_bold()
                                    .label("LOG IN")
                                    .on_click(cx.listener(Self::login_btn_click)),
                            ),
                    ),
            )
    }
}
