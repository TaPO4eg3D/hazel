use std::path::PathBuf;

use capture::video::linux::file::FileStreamer;
use client::gpui_tokio::{self};
use gpui::WindowOptions;
use gpui_platform::application;
use server::{config::Config, start_server};
use tokio::runtime::Builder;

use crate::screencast::view::{ConnectionState, ScreenCastView};

mod view;
mod vulkan;

pub fn run(file_path: Option<PathBuf>) {
    let file_path = file_path.expect("Live capture is not yet supported for this scenario");

    let streamer = FileStreamer::new(&file_path, 60.);
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
        voice_channels: vec![server::config::VoiceChannel {
            name: "simulation".to_string(),
            max_participants: 2,
        }],
    }));

    let app = application();
    app.run(move |cx| {
        gpui_component::init(cx);
        gpui_tokio::init_with_runtime(cx, tokio_runtime);

        cx.spawn(async move |cx| {
            let host_connection = ConnectionState::new("host", "host", cx).await;
            let client_connection = ConnectionState::new("client", "client", cx).await;

            host_connection.join_voice_channel().await;
            client_connection.join_voice_channel().await;

            cx.open_window(WindowOptions::default(), move |window, cx| {
                ScreenCastView::new(streamer, host_connection, client_connection, window, cx)
            })
        })
        .detach();
    });
}
