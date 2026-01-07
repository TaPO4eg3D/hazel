use std::path::PathBuf;

use gpui::*;
use gpui_component::{Root, Theme, ThemeRegistry};

use anyhow::{Result as AResult, bail};
use rpc::client::Connection;

pub mod assets;
pub mod gpui_tokio;
pub mod screens;

use screens::login::LoginScreen;

use crate::{assets::Assets, gpui_tokio::Tokio};

enum Screen {
    Login(Entity<LoginScreen>),
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
        let screen = match &self.current_screen {
            Screen::Login(screen) => screen.clone(),
            _ => todo!(),
        };

        let notification_layer = Root::render_notification_layer(window, cx);

        div().size_full().child(screen).children(notification_layer)
    }
}

pub fn init_theme(cx: &mut App) {
    let theme_name = SharedString::from("Tokyo Night");

    if let Err(err) = ThemeRegistry::watch_dir(PathBuf::from("./themes"), cx, move |cx| {
        if let Some(theme) = ThemeRegistry::global(cx).themes().get(&theme_name).cloned() {
            Theme::global_mut(cx).apply_config(&theme);
        } else {
            panic!("Theme is not found! Are you running the app not inside the root folder?")
        }
    }) {
        tracing::error!("Failed to watch themes directory: {}", err);
    }
}

fn main() {
    let app = Application::new().with_assets(Assets);

    app.run(move |cx| {
        gpui_component::init(cx);
        gpui_tokio::init(cx);

        init_theme(cx);

        cx.set_global(ConnectionManger::new());

        cx.spawn(async move |cx| {
            cx.open_window(WindowOptions::default(), |window, cx| {
                let login_screen = cx.new(|cx| LoginScreen::new(window, cx));
                let view = cx.new(|_| MainWindow {
                    current_screen: Screen::Login(login_screen),
                });
                // This first level on the window, should be a Root.
                cx.new(|cx| Root::new(view, window, cx))
            })?;

            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
}
