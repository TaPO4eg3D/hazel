use gpui::{
    App, AppContext, ClickEvent, Context, Div, Element, Entity, IntoElement, ParentElement, Render, RenderOnce, Rgba, Styled, Window, div, prelude::FluentBuilder, px, red, rgb, white
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, Root, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    label,
    spinner::Spinner,
    text,
};
use rpc::models::auth::{GetSessionKeyError, GetSessionKeyPayload, GetSessionKeyResponse};

use crate::{ConnectionManger, assets::IconName, gpui_tokio::Tokio};

pub struct LoginScreen {
    username: Entity<InputState>,
    password: Entity<InputState>,
    server_address: Entity<InputState>,

    /// Indicates if we're in the process
    /// of connecting to a server
    is_connecting: bool,
    is_form_valid: bool,
}

enum ConnectionResult {
    Connected,
    Failed(String),
}

impl LoginScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let username = cx.new(|cx| InputState::new(window, cx).default_value("admin"));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true).default_value("admin"));
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
            is_form_valid: true,
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

            entity.is_form_valid =
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
                                window.push_notification("Successfull Login!", cx);
                            }
                            ConnectionResult::Failed(err) => {
                                window.push_notification(format!("Failed to connect: {err}!"), cx);
                            }
                        };
                    })
                    .ok();

                Ok::<_, anyhow::Error>(())
            })
            .detach();

        cx.spawn(async move |this, cx| {
            // TODO: Properly handle a case when we can't connect
            ConnectionManger::connect(cx, server_ip.into())
                .await?;

            let (login, password) = this.read_with(cx, |this, cx| {
                (
                    this.username.read(cx).value(),
                    this.password.read(cx).value()
                )
            })?;
            let connection = ConnectionManger::get(cx);

            let data: Result<
                GetSessionKeyResponse,
                GetSessionKeyError,
            > = connection.execute("GetSessionKey", &GetSessionKeyPayload {
                login: login.into(),
                password: password.into(),
            }).await?;

            match data {
                Ok(value) => {
                    println!("{value:?}");

                    tx.send(ConnectionResult::Connected).await?;
                }
                Err(err) => tx.send(ConnectionResult::Failed(format!("{err:?}"))).await?,
            }

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
                            .disabled(self.is_connecting)
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
                            .disabled(self.is_connecting)
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
                                    .disabled(self.is_connecting)
                                    .text_decoration_color(white())
                                    .min_h(px(55.))
                                    .mt(px(12.))
                                    .mb(px(30.))
                                    .prefix(Icon::new(IconName::Server)),
                            )
                            .child(
                                Button::new("ok")
                                    .h(px(55.))
                                    .disabled(!self.is_form_valid || self.is_connecting)
                                    .primary()
                                    .mt_auto()
                                    .text_color(white())
                                    .font_bold()
                                    .loading(self.is_connecting)
                                    .loading_icon(Icon::new(IconName::Loader))
                                    .label("LOG IN")
                                    .when(self.is_connecting, |this| this.label("Connecting..."))
                                    .on_click(cx.listener(Self::login_btn_click)),
                            ),
                    ),
            )
    }
}
