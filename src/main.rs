use std::path::PathBuf;

use gpui::*;
use gpui_component::{button::*, *};
use gpui_component_assets::Assets;

mod screens;
mod gpui_tokio;

use screens::login::LoginScreen;

enum Screen {
    Login(Entity<LoginScreen>),
}

pub struct MainWindow {
    current_screen: Screen,
}

impl Render for MainWindow {
    fn render(&mut self, _: &mut Window, ctx: &mut Context<Self>) -> impl IntoElement {
        match &self.current_screen {
            Screen::Login(screen) => screen.clone(),
            _ => todo!(),
        }
    }
}

pub fn init_theme(cx: &mut App) {
    let theme_name = SharedString::from("Tokyo Night");

    if let Err(err) = ThemeRegistry::watch_dir(PathBuf::from("./themes"), cx, move |cx| {
        if let Some(theme) = ThemeRegistry::global(cx)
            .themes()
            .get(&theme_name)
            .cloned()
        {
            Theme::global_mut(cx).apply_config(&theme);
        }
    }) {
        tracing::error!("Failed to watch themes directory: {}", err);
    }
}

fn main() {
    let app = Application::new()
        .with_assets(Assets);

    app.run(move |cx| {
        gpui_component::init(cx);
        gpui_tokio::init(cx);

        init_theme(cx);

        cx.spawn(async move |cx| {
            cx.open_window(WindowOptions::default(), |window, cx| {
                let login_screen = cx.new(|cx| LoginScreen::new(window, cx));
                let view = cx.new(|_| MainWindow {
                    current_screen: Screen::Login(login_screen)
                });
                // This first level on the window, should be a Root.
                cx.new(|cx| Root::new(view, window, cx))
            })?;

            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
}
