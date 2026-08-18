// Disable command line from opening on release mode
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::rc::Rc;

use clap::Parser;

use gpui::*;
use gpui_component::{Root, Theme, ThemeRegistry, WindowExt};
use gpui_platform::application;

use rpc::{
    client::ClientConnection,
    models::{
        auth::{Login, LoginPayload, SessionKey},
        common::RPCMethod,
        markers::{Id, UserId},
    },
};

use client::{
    assets::Assets,
    components::connection_state::RpcConnectionInfo,
    db::{self, DBConnectionManager},
    gpui_tokio::{self, Tokio},
    screens::{login::LoginScreen, workspace::WorkspaceScreen},
    streaming,
};

pub struct MainWindow {
    login_screen: Entity<LoginScreen>,
    workspace_screen: Option<Entity<WorkspaceScreen>>,
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dialogue_layer = Root::render_dialog_layer(window, cx);
        let notifications_layer = Root::render_notification_layer(window, cx);

        let mut root = div().size_full();

        if let Some(workspace_screen) = self.workspace_screen.as_ref() {
            root = root.child(workspace_screen.clone())
        } else {
            root = root.child(self.login_screen.clone())
        }

        root.children(dialogue_layer).children(notifications_layer)
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

        init_theme(cx);

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

                let view = cx.new(|cx| {
                    cx.subscribe_in(
                        &login_screen,
                        window,
                        |this: &mut MainWindow,
                         _,
                         (user_id, connection, connection_info): &(
                            UserId,
                            ClientConnection,
                            RpcConnectionInfo,
                        ),
                         window,
                         cx| {
                            this.workspace_screen = Some(cx.new(|cx| {
                                WorkspaceScreen::new(
                                    window,
                                    cx,
                                    *user_id,
                                    connection.clone(),
                                    connection_info.clone(),
                                )
                            }));
                        },
                    )
                    .detach();

                    MainWindow {
                        workspace_screen: None,
                        login_screen: login_screen.clone(),
                    }
                });

                window
                    .spawn(cx, {
                        let view = view.clone();

                        async move |cx| {
                            if let (Some(session_key), Some(server_ip)) =
                                (registry.session_key, registry.connected_server)
                            {
                                // TODO: Refeactor how we manage connection,
                                // we currently hang indefinetly while waiting for the connection.
                                let connection =
                                    Tokio::spawn(cx, {
                                        let server_ip = server_ip.clone();

                                        async move {
                                            ClientConnection::new(&format!("{server_ip}:9898"))
                                        }
                                    })
                                    .await
                                    .expect("todo: we currently can't fail and just hang");

                                match rmp_serde::from_slice::<SessionKey>(&session_key) {
                                    Ok(session_key) => {
                                        let result = Login::execute(
                                            &connection,
                                            &LoginPayload {
                                                session_key: session_key.clone(),
                                            },
                                        )
                                        .await;

                                        if result.is_ok() {
                                            view.update_in(cx, |this, window, cx| {
                                                this.workspace_screen = Some(cx.new(|cx| {
                                                    WorkspaceScreen::new(
                                                        window,
                                                        cx,
                                                        UserId::new(session_key.body.user_id),
                                                        connection,
                                                        RpcConnectionInfo { server_ip },
                                                    )
                                                }));
                                            })
                                            .ok();
                                        } else {
                                            login_screen.update(cx, |this, _| {
                                                this.is_connecting = false;
                                            });

                                            cx.window_handle()
                                                .update(cx, |_, window, cx| {
                                                    window.push_notification(
                                                        "Stale session, please log in again",
                                                        cx,
                                                    );
                                                })
                                                .ok();
                                        }
                                    }
                                    Err(_) => {
                                        login_screen.update(cx, |this, _| {
                                            this.is_connecting = false;
                                        });

                                        cx.window_handle()
                                            .update(cx, |_, window, cx| {
                                                window.push_notification(
                                                    "Stale session, please log in again",
                                                    cx,
                                                );
                                            })
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
