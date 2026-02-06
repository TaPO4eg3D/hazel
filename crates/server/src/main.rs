use std::{
    net::SocketAddr,
    sync::{Arc, RwLock},
};

use dashmap::DashMap;

use rpc::{
    models::{
        common::RPCNotification,
        general::{UserConnectionUpdate, UserConnectionUpdateMessage},
        markers::{TaggedEntity, TextChannelId, UserId, VoiceChannelId},
        voice::{VoiceChannelUpdate, VoiceChannelUpdateMessage},
    },
    server::{RpcRouter, RpcWriter, serve},
};

use sea_orm::{Database, DatabaseConnection};

use entity::user::Model as User;

use crate::{
    api::{auth, messages, voice},
    config::Config,
    streaming::open_udp_socket,
};

mod api;
mod config;
mod entity;
mod streaming;

pub type GlobalRouter = RpcRouter<AppState, ConnectionState>;

pub struct VoiceUser {
    id: UserId,

    is_muted: bool,
    is_sound_off: bool,
}

impl VoiceUser {
    pub fn new(id: UserId) -> Self {
        Self {
            id,

            is_muted: false,
            is_sound_off: false,
        }
    }
}

/// This state holds connected users to respective channels
pub struct ChannelsState {
    pub text_channels: DashMap<TextChannelId, Vec<UserId>>,
    pub voice_channels: DashMap<VoiceChannelId, Vec<VoiceUser>>,
}

impl ChannelsState {
    fn disonnect_user_from_voice_channel(
        &self,
        user_id: Option<UserId>,
        channel_id: Option<VoiceChannelId>,
    ) -> bool {
        let (Some(user_id), Some(channel_id)) = (user_id, channel_id) else {
            return false;
        };

        let Some(mut users) = self.voice_channels.get_mut(&channel_id) else {
            return false;
        };

        users.retain(|user| user.id != user_id);

        true
    }
}

#[derive(Clone)]
pub struct UDPStreamState {
    pub voice_channel: VoiceChannelId,
    pub addr: SocketAddr,
}

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,

    pub channels: Arc<ChannelsState>,
    pub connected_clients: Arc<DashMap<UserId, ConnectionState>>,
}

impl AppState {
    fn disconnect(&self, user_id: Option<UserId>) {
        let Some(user_id) = user_id else {
            return;
        };

        self.connected_clients.remove(&user_id);
    }
}

/// State specific for a single connection.
/// This is the place where it makes sense to store auth data
/// and anything like this
#[derive(Debug, Clone)]
pub struct ConnectionStateInner {
    pub user: Option<User>,
    pub active_voice_channel: Option<VoiceChannelId>,
    pub active_stream: Option<SocketAddr>,

    /// This is mostly used to send notifications to the user
    pub writer: RpcWriter,
}

impl ConnectionStateInner {
    /// Disconnect the user from the server and notify everyone involved
    async fn disconnect(&self, state: &AppState) {
        let user_id = self.get_user_id();
        let channel_id = self.active_voice_channel;

        state.disconnect(self.get_user_id());
        self.disconnect_from_voice_channel(state);

        let (Some(user_id), Some(channel_id)) = (user_id, channel_id) else {
            return;
        };

        let writers = state
            .connected_clients
            .iter()
            .map(|user| user.read().unwrap().writer.clone())
            .collect::<Vec<_>>();

        for writer in writers {
            VoiceChannelUpdate {
                channel_id,
                message: VoiceChannelUpdateMessage::UserDisconnected(user_id),
            }
            .notify(&writer)
            .await;

            UserConnectionUpdate {
                user_id,
                message: UserConnectionUpdateMessage::UserDisconnected,
            }
            .notify(&writer)
            .await;
        }
    }

    pub fn get_user_id(&self) -> Option<UserId> {
        self.user.as_ref().map(|user| user.tagged_id())
    }

    pub fn disconnect_from_voice_channel(&self, state: &AppState) {
        _ = state
            .channels
            .disonnect_user_from_voice_channel(self.get_user_id(), self.active_voice_channel);
    }
}

impl ConnectionStateInner {
    pub fn is_authenticated(&self) -> bool {
        self.user.is_some()
    }
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

    let config = std::fs::read_to_string("./config.toml").expect("Config is not provided");
    let config = toml::from_str::<Config>(&config).expect("Invalid config");

    let state = init_state().await;
    let router = RpcRouter::new(state.clone(), move |writer| {
        Arc::new(RwLock::new(ConnectionStateInner {
            user: None,
            active_voice_channel: None,
            active_stream: None,
            writer,
        }))
    });

    let router = messages::merge(router);
    let router = auth::merge(router);
    let router = voice::merge(router);

    let tcp_addr = config.tcp_addr.clone();
    tokio::spawn(async move {
        serve(&tcp_addr, router, |state, conn_state| {
            // This function runs *after* the user is disconnected
            // aka we waited a bit for a reconnect but it didn't happen

            Box::pin(async move {
                let conn_state = conn_state.read().unwrap().clone();

                conn_state.disconnect(&state).await;
            })
        })
        .await;
    });

    open_udp_socket(state, &config.udp_addr).await.unwrap();
}
