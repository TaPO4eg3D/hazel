use std::path::PathBuf;

use gpui::*;
use gpui_component::{Root, Theme, ThemeRegistry, WindowExt};

use anyhow::Result as AResult;
use rpc::{
    client::Connection,
    models::auth::{LoginError, LoginPayload, SessionKey},
};

pub mod assets;
pub mod db;
pub mod gpui_tokio;
pub mod screens;

use screens::login::LoginScreen;

use crate::{assets::Assets, db::DBConnectionManager, gpui_tokio::Tokio};

enum Screen {
    Login(Entity<LoginScreen>),
    MainWorkspace,
}

pub struct MainWindow {
    current_screen: Screen,
}

pub struct ConnectionManger {
    conn: Option<Connection>,
}

impl ConnectionManger {
    fn new() -> Self {
        Self { conn: None }
    }

    fn is_connected(&self) -> bool {
        self.conn.is_some()
    }

    fn update(&mut self, connection: Connection) {
        self.conn = Some(connection);
    }

    fn get(cx: &mut AsyncApp) -> Connection {
        cx.read_global(|this: &Self, _| this.conn.as_ref().unwrap().clone())
            .unwrap()
    }

    async fn connect(cx: &mut AsyncApp, server_ip: String) -> AResult<()> {
        let connected = cx.read_global(|g: &Self, _| g.is_connected())?;

        if connected {
            // TODO: Change how we handle it
            return Ok(());
        }

        let connection = Tokio::spawn(cx, Connection::new(server_ip))?.await??;

        cx.update_global(|g: &mut Self, _| {
            g.update(connection);
        })?;

        Ok(())
    }
}

impl Global for ConnectionManger {}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let notification_layer = Root::render_notification_layer(window, cx);

        let mut root = div().size_full();

        match &self.current_screen {
            Screen::Login(screen) => root = root.child(screen.clone()),
            Screen::MainWorkspace => root = root.child("WORKSPACE"),
        };

        root.children(notification_layer)
    }
}

pub fn init_theme(cx: &mut App) {
    let theme_name = SharedString::from("Hazel Default");

    if let Err(err) = ThemeRegistry::watch_dir(PathBuf::from("./themes"), cx, move |cx| {
        if let Some(theme) = ThemeRegistry::global(cx).themes().get(&theme_name).cloned() {
            Theme::global_mut(cx).apply_config(&theme);
        } else {
            panic!("Theme is not found! Are you running the app not inside the root folder?")
        }
    }) {
        panic!("Failed to watch themes directory: {}", err);
    }
}

fn main() {
    let app = Application::new().with_assets(Assets);

    app.run(move |cx| {
        gpui_component::init(cx);
        gpui_tokio::init(cx);

        init_theme(cx);
        cx.set_global(ConnectionManger::new());

        // Check if we're already authorized
        cx.spawn(async move |cx| {
            db::init(cx).await.unwrap();

            let db = DBConnectionManager::get(cx);
            let registry =
                Tokio::spawn(
                    cx,
                    async move { DBConnectionManager::get_registry(&db).await },
                )?
                .await?;

            cx.open_window(WindowOptions::default(), |window, cx| {
                let login_screen = cx.new(|cx| {
                    LoginScreen::new(
                        window,
                        cx,
                        registry.session_key.is_some(),
                        registry.connected_server.clone(),
                    )
                });

                let view = cx.new(|cx| {
                    cx.subscribe(&login_screen, |this: &mut MainWindow, _, _: &(), _| {
                        this.current_screen = Screen::MainWorkspace;
                    })
                    .detach();

                    MainWindow {
                        current_screen: Screen::Login(login_screen.clone()),
                    }
                });

                let (tx, rx) = smol::channel::bounded::<String>(1);

                window.spawn(cx, async move |window| {
                    let message = rx.recv().await?;

                    window.update(|window, cx| {
                        window.push_notification(message, cx);
                    }).ok();

                    Ok::<_, anyhow::Error>(())
                }).detach();

                cx.spawn({
                    let view = view.clone();

                    async move |cx| {
                        if let (Some(session_key), Some(server_ip)) = (registry.session_key, registry.connected_server) {
                            if ConnectionManger::connect(cx, server_ip.clone()).await.is_err() {
                                // TODO: That's not how it works unfortunately, change it.
                                // ConnectionManger would try to connect infinitely and will never
                                // time out
                                tx.send(format!("failed to connect to: {server_ip}")).await.ok();
                                
                                return;
                            }
                    
                            let connection = ConnectionManger::get(cx);

                            match rmp_serde::from_slice::<SessionKey>(&session_key) {
                                Ok(session_key) => {
                                    let result: Result<(), LoginError> = connection
                                        .execute("Login", &LoginPayload { session_key })
                                        .await
                                        .expect("invalid params");

                                    if result.is_ok() {
                                        view.update(cx, |this, _| {
                                            this.current_screen = Screen::MainWorkspace;
                                        }).ok();
                                    } else {
                                        login_screen.update(cx, |this, _| {
                                            this.is_connecting = false;
                                        }).ok();

                                        tx.send("Stale session, please log in".into())
                                            .await.ok();
                                    }
                                }
                                Err(_) => {
                                    login_screen.update(cx, |this, _| {
                                        this.is_connecting = false;
                                    }).ok();

                                    tx.send("Corrupted data, please log in again".into())
                                        .await.ok();
                                }
                            };
                        }
                    }
                }).detach();

                // For notifications and stuff, this should be the first
                // element of the window (aka root)
                cx.new(|cx| Root::new(view, window, cx))
            })
            .unwrap();

            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
}
