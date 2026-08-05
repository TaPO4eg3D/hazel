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

pub async fn run() {
    let config = std::fs::read_to_string("./config.toml").expect("Config is not provided");
    let config = toml::from_str::<Config>(&config).expect("Invalid config");

    let state = init_state().await;
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

    open_udp_socket(state, &config.udp_addr).await.unwrap();
}
