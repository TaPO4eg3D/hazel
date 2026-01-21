use gpui::{
    AppContext, ClickEvent, Context, Entity, EventEmitter, IntoElement, ParentElement, Render,
    Styled, Window, div, prelude::FluentBuilder, px, rgb, white,
};
use gpui_component::{
    Disableable, Icon, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
};
use rpc::models::{
    auth::{
        GetSessionKey, GetSessionKeyError, GetSessionKeyPayload, GetSessionKeyResponse, LoginError,
        LoginPayload,
    },
    common::{APIError, RPCMethod},
    markers::Id,
};
use sea_orm::{ActiveModelTrait, ActiveValue::Set};

use crate::{
    ConnectionManger,
    assets::IconName,
    db::{DBConnectionManager, entity::registry},
    gpui_tokio::Tokio,
};

pub struct LoginScreen {
    username: Entity<InputState>,
    password: Entity<InputState>,
    server_address: Entity<InputState>,

    /// Indicates if we're in the process
    /// of connecting to a server
    pub is_connecting: bool,
    is_form_valid: bool,
}

impl EventEmitter<()> for LoginScreen {}

enum ConnectionResult {
    NewUser,
    ExistingAcount,
    Failed(String),
}

impl LoginScreen {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        is_connecting: bool,
        server_address: Option<String>,
    ) -> Self {
        let username = cx.new(|cx| InputState::new(window, cx));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
        });
        let server_address = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(server_address.unwrap_or("localhost".to_string()))
        });

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

            is_connecting,
            is_form_valid: false,
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
                            ConnectionResult::NewUser => {
                                window.push_notification("Successfully registered!", cx);
                            }
                            ConnectionResult::ExistingAcount => {
                                window.push_notification("Successfully logged in!", cx);
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
            ConnectionManger::connect(cx, server_ip.clone().into()).await?;

            let (login, password) = this.read_with(cx, |this, cx| {
                (
                    this.username.read(cx).value(),
                    this.password.read(cx).value(),
                )
            })?;
            let connection = ConnectionManger::get(cx);

            let response = GetSessionKey::execute(
                &connection,
                &GetSessionKeyPayload {
                    login: login.into(),
                    password: password.into(),
                },
            )
            .await;

            match response {
                Ok(value) => {
                    let session_key = match value {
                        GetSessionKeyResponse::NewUser(key) => {
                            tx.send(ConnectionResult::NewUser).await?;
                            key
                        }
                        GetSessionKeyResponse::ExistingUser(key) => {
                            tx.send(ConnectionResult::ExistingAcount).await?;
                            key
                        }
                    };

                    let db = DBConnectionManager::get(cx);
                    let session_key_bytes = rmp_serde::to_vec(&session_key).unwrap();
                    Tokio::spawn(cx, async move {
                        let registry = DBConnectionManager::get_registry(&db).await;
                        let mut registry: registry::ActiveModel = registry.into();

                        registry.session_key = Set(Some(session_key_bytes));
                        registry.connected_server = Set(Some(server_ip.into()));

                        registry.update(&db).await.unwrap();
                    })
                    .await?;

                    let data: Result<(), LoginError> = connection
                        .execute(
                            "Login",
                            &LoginPayload {
                                session_key: session_key.clone(),
                            },
                        )
                        .await
                        .expect("invalid params");

                    data.expect("We just logged in, it should not fail");

                    ConnectionManger::set_user_id(cx, Id::new(session_key.body.user_id));

                    // Notify parent component that we're logged in
                    this.update(cx, |_, cx| {
                        cx.emit(());
                    })
                    .unwrap();
                }
                Err(err) => match err {
                    APIError::Err(GetSessionKeyError::UserAlreadyExists) => {
                        tx.send(ConnectionResult::Failed("incorrect password".to_string()))
                            .await?;
                    }
                    _ => {
                        tx.send(ConnectionResult::Failed(format!("{err:?}")))
                            .await?
                    }
                },
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
                            .prefix(Icon::new(IconName::User)),
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
                            .prefix(Icon::new(IconName::Lock))
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
