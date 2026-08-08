use std::path::PathBuf;

use client::gpui_tokio;
use gpui::{AppContext, WindowOptions};
use gpui_platform::application;
use server::{config::Config, start_server};
use tokio::runtime::Builder;

use crate::screencast::view::ScreenCastView;

mod view;

pub fn run(file: Option<PathBuf>) {
    let file = file.expect("Live capture is not yet supported for this scenario");

    let tokio_runtime = Builder::new_multi_thread()
        .worker_threads(4)
        .thread_name("Embedded server")
        .enable_all()
        .build()
        .unwrap();

    tokio_runtime.spawn(start_server(Config {
        tcp_addr: "0.0.0.0:9898".to_string(),
        udp_addr: "0.0.0.0:9899".to_string(),

        text_channels: vec![],
        voice_channels: vec![],
    }));

    let app = application();
    app.run(move |cx| {
        gpui_component::init(cx);
        gpui_tokio::init_with_runtime(cx, tokio_runtime);

        cx.spawn(async move |cx| {
            cx.open_window(WindowOptions::default(), move |window, cx| {
                cx.new(|cx| ScreenCastView::new(file, window, cx))
            })
        })
        .detach();
    });
}
