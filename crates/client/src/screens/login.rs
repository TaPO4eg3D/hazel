use gpui::{
    AppContext, ClickEvent, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, Window, div, prelude::FluentBuilder,
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    label::Label,
    separator::Separator,
};
use rpc::{
    client::ClientConnection,
    models::{
        auth::{
            GetSessionKey, GetSessionKeyError, GetSessionKeyPayload, GetSessionKeyResponse,
            LoginError, LoginPayload,
        },
        common::{APIError, RPCMethod},
        markers::{Id, UserId},
    },
};
use sea_orm::{ActiveModelTrait, ActiveValue::Set};

use crate::{
    assets::IconName,
    components::connection_state::RpcConnectionInfo,
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

impl EventEmitter<(UserId, ClientConnection, RpcConnectionInfo)> for LoginScreen {}

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
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let server_address = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(server_address.unwrap_or("127.0.0.1".to_string()))
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
            // TODO: Refeactor how we manage connection,
            // we currently hang indefinetly while waiting for the connection.
            let connection = Tokio::spawn(cx, {
                let server_ip = server_ip.clone();

                async move { ClientConnection::new(&format!("{server_ip}:9898")) }
            })
            .await
            .expect("todo: we currently can't fail and just hang");

            let (login, password) = this.read_with(cx, |this, cx| {
                (
                    this.username.read(cx).value(),
                    this.password.read(cx).value(),
                )
            })?;

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

                    Tokio::spawn(cx, {
                        let server_ip = server_ip.clone();

                        async move {
                            let registry = DBConnectionManager::get_registry(&db).await;
                            let mut registry: registry::ActiveModel = registry.into();

                            registry.session_key = Set(Some(session_key_bytes));
                            registry.connected_server = Set(Some(server_ip.into()));

                            registry.update(&db).await.unwrap();
                        }
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

                    // Notify parent component that we're logged in
                    this.update(cx, |_, cx| {
                        cx.emit((
                            UserId::new(session_key.body.user_id),
                            connection,
                            RpcConnectionInfo {
                                server_ip: server_ip.into(),
                            },
                        ));
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

impl LoginScreen {
    fn create_input(&self, state: &Entity<InputState>) -> Input {
        Input::new(state)
            .border_1()
            .rounded_md()
            .disabled(self.is_connecting)
    }
}

impl Render for LoginScreen {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        // Background container
        div()
            .id("login-card")
            .flex()
            .items_center()
            .justify_center()
            .h_full()
            .child(
                // Card
                div()
                    .w_96()
                    .border_1()
                    .rounded_lg()
                    .border_color(cx.theme().secondary_active)
                    .child(
                        // Card Content
                        div()
                            .p_4()
                            .child(
                                Label::new("HAZEL")
                                    .mt_4()
                                    .text_center()
                                    .text_xl()
                                    .font_bold(),
                            )
                            .child(
                                Label::new(
                                    "Enter any credentials if you're logging for the first time. \
                                Account will be created automatically",
                                )
                                .mt_4()
                                .mb_4()
                                .text_center()
                                .text_xs(),
                            )
                            .child(
                                div()
                                    .mb_2()
                                    .child(Label::new("Username").text_xs())
                                    .child(self.create_input(&self.username)),
                            )
                            .child(
                                div()
                                    .child(Label::new("Password").text_xs())
                                    .child(self.create_input(&self.password)),
                            )
                            .child(Separator::horizontal().mt_4().mb_4())
                            .child(
                                div()
                                    .mb_2()
                                    .child(Label::new("Server IP").text_xs())
                                    .child(self.create_input(&self.server_address)),
                            )
                            .child(
                                Button::new("login-btn")
                                    .mt_4()
                                    .label("Login")
                                    .primary()
                                    .disabled(!self.is_form_valid || self.is_connecting)
                                    .loading(self.is_connecting)
                                    .loading_icon(Icon::new(IconName::Loader))
                                    .when(self.is_connecting, |this| this.label("Connecting..."))
                                    .on_click(cx.listener(Self::login_btn_click)),
                            ),
                    ),
            )
    }
}
