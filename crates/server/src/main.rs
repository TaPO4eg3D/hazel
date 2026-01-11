use std::sync::{Arc, RwLock};

use dashmap::DashMap;

use rpc::{
    models::messages::{TextChannelId, UserId, VoiceChannelId},
    server::{serve, RpcRouter, RpcWriter},
};

use sea_orm::{Database, DatabaseConnection};

use entity::user::Model as User;

use crate::{api::{auth, messages}, config::Config};

mod api;
mod config;
mod entity;

/// This state holds connected users to respective channels
struct ChannelsState {
    text_channels: DashMap<TextChannelId, Vec<UserId>>,
    voice_channels: DashMap<VoiceChannelId, Vec<UserId>>,
}

#[derive(Clone)]
pub struct AppState {
    db: DatabaseConnection,
    /// This HashMap holds every connected client
    /// with the respective writer you can use to send messages
    connected_clients: Arc<DashMap<UserId, RpcWriter>>,
    channels: Arc<ChannelsState>,
}

/// State specific for a single connection.
/// This is the place where it makes sense to store auth data
/// and anything like this
#[derive(Debug)]
pub struct ConnectionStateInner {
    user: Option<User>,

    /// This is mostly used to send notifications to the user
    writer: RpcWriter,
}

pub type ConnectionState = Arc<RwLock<ConnectionStateInner>>;

async fn init_state() -> AppState {
    let db = Database::connect("sqlite://db.sqlite?mode=rwc")
        .await
        .unwrap();

    AppState {
        db,
        channels: Arc::new(ChannelsState {
            text_channels: DashMap::new(),
            voice_channels: DashMap::new(),
        }),
        connected_clients: Arc::new(DashMap::new()),
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let config = std::fs::read_to_string("./config.toml")
        .expect("Config is not provided");

    let config = toml::from_str::<Config>(&config)
        .expect("Invalid config");

    let state = init_state().await;

    let router = RpcRouter::new(state, |writer| {
        Arc::new(RwLock::new(ConnectionStateInner {
            user: None,
            writer,
        }))
    });

    let router = messages::merge(router);
    let router = auth::merge(router);

    serve(&config.addr, router).await;
}
