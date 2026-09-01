use std::sync::{Arc, RwLock};

use rpc::server::{RpcRouter, serve};

use crate::{
    config::Config,
    state::{ConnectionState, ConnectionStateInner, init_state},
    streaming::open_udp_socket,
};

pub mod api;
pub mod config;
pub mod entity;
pub mod state;
pub mod streaming;

pub async fn start_server(config: Config) {
    let state = init_state().await;
    state
        .create_channels_from_config(&config)
        .await
        .expect("Failed to create configured channels");

    let router = RpcRouter::new(state.clone(), move |writer| {
        ConnectionState(Arc::new(RwLock::new(ConnectionStateInner {
            user: None,
            active_voice_channel: None,
            active_stream: None,
            writer,
        })))
    });

    let router = crate::api::auth::register(router);
    let router = crate::api::voice_channels::register(router);

    let tcp_addr = config.tcp_addr.clone();
    tokio::spawn(async move {
        serve(&tcp_addr, router, |state, conn_state| {
            // This function runs *after* the user is disconnected
            // aka we waited a bit for a reconnect but it didn't happen

            Box::pin(async move {
                let conn_state = conn_state.read::<()>().ok().map(|value| value.clone());

                if let Some(conn_state) = conn_state {
                    conn_state.disconnect(&state.clone()).await;
                }
            })
        })
        .await;
    });

    tokio::spawn(async move {
        open_udp_socket(state, &config.udp_addr).await.unwrap();
    })
    .await
    .unwrap();
}
