// Disable command line from opening on release mode
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::rc::Rc;

use clap::Parser;

use gpui::*;
use gpui_platform::application;
use gpui_component::{Root, Theme, ThemeRegistry, WindowExt};

use anyhow::Result as AResult;
use rpc::{
    client::Connection,
    models::{
        auth::{Login, LoginPayload, SessionKey},
        common::RPCMethod,
        markers::{Id, UserId},
    },
};

pub mod assets;
pub mod components;
pub mod db;
pub mod screens;

pub mod gpui_audio;
pub mod gpui_tokio;

use screens::login::LoginScreen;

use crate::{
    assets::Assets, db::DBConnectionManager, gpui_tokio::Tokio, screens::workspace::WorkspaceScreen,
};

enum Screen {
    Login,
    MainWorkspace,
}

pub struct MainWindow {
    current_screen: Screen,

    login_screen: Entity<LoginScreen>,
    workspace_screen: Entity<WorkspaceScreen>,
}

impl MainWindow {
    fn set_workspace_screen(&mut self, cx: &mut Context<Self>) {
        self.current_screen = Screen::MainWorkspace;
        self.workspace_screen.update(cx, |this, cx| {
            this.init(cx);
        });

        cx.notify();
    }
}

pub struct ConnectionManger {
    conn: Option<Connection>,

    user_id: Option<UserId>,
    server_ip: Option<String>,
}

impl ConnectionManger {
    fn new() -> Self {
        Self {
            conn: None,
            user_id: None,
            server_ip: None,
        }
    }

    pub fn get_user_id<C: AppContext>(cx: &C) -> Option<UserId> {
        cx.read_global(|g: &Self, _| g.user_id)
    }

    pub fn get_server_ip(cx: &mut AsyncApp) -> Option<String> {
        cx.read_global(|g: &Self, _| g.server_ip.clone())
    }

    pub fn set_user_id(cx: &mut AsyncApp, id: UserId) {
        cx.update_global(|g: &mut Self, _| {
            g.user_id = Some(id);
        });
    }

    fn is_connected(&self) -> bool {
        self.conn.is_some()
    }

    fn get(cx: &mut AsyncApp) -> Connection {
        cx.read_global(|this: &Self, _| this.conn.as_ref().unwrap().clone())
    }

    async fn connect(cx: &mut AsyncApp, mut server_ip: String) -> AResult<()> {
        if server_ip == "localhost" {
            server_ip = "127.0.0.1".into();
        }

        let connected = cx.read_global(|g: &Self, _| g.is_connected());

        if connected {
            // TODO: Change how we handle it
            return Ok(());
        }

        let connection = Tokio::spawn(cx, Connection::new(format!("{server_ip}:9898"))).await??;

        cx.update_global(move |g: &mut Self, _| {
            g.server_ip = Some(server_ip);
            g.conn = Some(connection);
        });

        Ok(())
    }
}

impl Global for ConnectionManger {}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let notification_layer = Root::render_notification_layer(window, cx);

        let mut root = div().size_full();

        match &self.current_screen {
            Screen::Login => root = root.child(self.login_screen.clone()),
            Screen::MainWorkspace => root = root.child(self.workspace_screen.clone()),
        };

        root.children(notification_layer)
    }
}

pub fn init_theme(cx: &mut App) {
    Assets::load_fonts(cx).expect("Font load should not fail");

    let config = ThemeRegistry::global(cx)
        .themes()
        .get("Default Dark")
        .unwrap()
        .clone();

    let mut config = (*config).clone();
    config.font_family = Some("Geist".into());

    let config = Rc::new(config);
    Theme::global_mut(cx).apply_config(&config);
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    profile: Option<String>,

    #[arg(long, default_value = "false")]
    audio_debug: bool,
}

fn main() {
    let args = Args::parse();
    let app = application().with_assets(Assets);

    app.run(move |cx| {
        gpui_component::init(cx);

        gpui_tokio::init(cx);
        gpui_audio::init(cx, args.audio_debug);

        init_theme(cx);
        cx.set_global(ConnectionManger::new());

        // Check if we're already authorized
        cx.spawn(async move |cx| {
            let profile = args.profile.unwrap_or("default".into());

            db::init(cx, profile).await.unwrap();

            let db = DBConnectionManager::get(cx);
            let registry = Tokio::spawn(
                cx,
                async move { DBConnectionManager::get_registry(&db).await },
            )
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

                let workspace_screen = cx.new(|cx| WorkspaceScreen::new(window, cx));

                let view = cx.new(|cx| {
                    cx.subscribe(&login_screen, |this: &mut MainWindow, _, _: &(), cx| {
                        this.set_workspace_screen(cx);
                    })
                    .detach();

                    MainWindow {
                        current_screen: Screen::Login,

                        login_screen: login_screen.clone(),
                        workspace_screen,
                    }
                });

                let (tx, rx) = smol::channel::bounded::<String>(1);

                window
                    .spawn(cx, async move |window| {
                        let message = rx.recv().await?;

                        window
                            .update(|window, cx| {
                                window.push_notification(message, cx);
                            })
                            .ok();

                        Ok::<_, anyhow::Error>(())
                    })
                    .detach();

                cx.spawn({
                    let view = view.clone();

                    async move |cx| {
                        if let (Some(session_key), Some(server_ip)) =
                            (registry.session_key, registry.connected_server)
                        {
                            if ConnectionManger::connect(cx, server_ip.clone())
                                .await
                                .is_err()
                            {
                                // TODO: That's not how it works unfortunately, change it.
                                // ConnectionManger would try to connect infinitely and will never
                                // time out
                                tx.send(format!("failed to connect to: {server_ip}"))
                                    .await
                                    .ok();

                                return;
                            }

                            let connection = ConnectionManger::get(cx);

                            match rmp_serde::from_slice::<SessionKey>(&session_key) {
                                Ok(session_key) => {
                                    let result = Login::execute(
                                        &connection,
                                        &LoginPayload {
                                            session_key: session_key.clone(),
                                        },
                                    )
                                    .await;

                                    ConnectionManger::set_user_id(
                                        cx,
                                        Id::new(session_key.body.user_id),
                                    );

                                    if result.is_ok() {
                                        view.update(cx, |this, cx| {
                                            this.set_workspace_screen(cx);
                                        });
                                    } else {
                                        login_screen.update(cx, |this, _| {
                                            this.is_connecting = false;
                                        });

                                        tx.send("Stale session, please log in".into()).await.ok();
                                    }
                                }
                                Err(_) => {
                                    login_screen.update(cx, |this, _| {
                                        this.is_connecting = false;
                                    });

                                    tx.send("Corrupted data, please log in again".into())
                                        .await
                                        .ok();
                                }
                            };
                        }
                    }
                })
                .detach();

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
