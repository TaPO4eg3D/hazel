use std::path::PathBuf;

use gpui::*;
use gpui_component::{Root, Theme, ThemeRegistry};

use anyhow::{Result as AResult};
use rpc::client::Connection;

pub mod db;
pub mod assets;
pub mod gpui_tokio;
pub mod screens;

use screens::login::LoginScreen;

use crate::{assets::Assets, db::{DBConnectionManager, entity::registry}, gpui_tokio::Tokio};

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

    async fn login(cx: &mut AsyncApp) {
        Tokio::spawn(cx, async move {
        })
            .unwrap()
            .await
            .unwrap();
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
        let screen = match &self.current_screen {
            Screen::Login(screen) => screen.clone(),
            _ => todo!(),
        };

        let notification_layer = Root::render_notification_layer(window, cx);

        div().size_full().child(screen).children(notification_layer)
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
            db::init(cx).await
                .unwrap();

            let db = DBConnectionManager::get(cx);
            let registry = Tokio::spawn(cx, async move {
                DBConnectionManager::get_registry(&db).await
            })?.await?;

            // Open the window first to not block it if the server
            // is slow to response on login
            cx.open_window(WindowOptions::default(), |window, cx| {
                let login_screen = cx.new(|cx| LoginScreen::new(
                    window,
                    cx,
                    registry.session_key.is_some(),
                    registry.connected_server,
                ));
                let view = cx.new(|_| MainWindow {
                    current_screen: Screen::Login(login_screen),
                });

                // For notifications and stuff, this should be the first
                // element of the window (aka root)
                cx.new(|cx| Root::new(view, window, cx))
            }).unwrap();

            if let Some(_session_key) = registry.session_key {
                todo!("Implement Login");
            }

            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
}
