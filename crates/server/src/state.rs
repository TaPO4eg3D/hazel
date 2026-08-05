use std::{
    fmt::Debug,
    future::Future,
    net::SocketAddr,
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use dashmap::DashMap;

use rpc::{
    models::{
        common::{APIError, RPCNotification, ServerErr},
        general::{UserConnectionUpdate, UserConnectionUpdateMessage},
        markers::{TaggedEntity, TextChannelId, UserId, VoiceChannelId},
        voice_channels::{
            VideoSessionParams, VoiceChannelUpdate, VoiceChannelUpdateMessage,
            VoiceChannelUserState,
        },
    },
    server::{RpcRouter, RpcWriter},
};

use sea_orm::{Database, DatabaseConnection};

use crate::entity::user::Model as User;

pub type AppRouter = RpcRouter<AppState, ConnectionState>;

pub struct VideoSession {
    pub params: VideoSessionParams,
    pub connected_clients: Vec<UserId>,
}

impl VideoSession {
    pub fn new(params: VideoSessionParams) -> Self {
        Self {
            params,
            connected_clients: vec![],
        }
    }
}

pub struct VoiceChannelUser {
    pub id: UserId,
    pub state: VoiceChannelUserState,

    pub screencast_session: Option<VideoSession>,
    pub joined_streams: Vec<UserId>,
}

impl VoiceChannelUser {
    pub fn new(id: UserId) -> Self {
        Self {
            id,
            state: VoiceChannelUserState::default(),
            screencast_session: None,
            joined_streams: vec![],
        }
    }
}

/// This state holds connected users to respective channels
pub struct ChannelsState {
    pub text_channels: DashMap<TextChannelId, Vec<UserId>>,
    pub voice_channels: DashMap<VoiceChannelId, Vec<VoiceChannelUser>>,
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

    pub async fn for_each_user<F, Fut>(&self, f: F)
    where
        F: Fn(ConnectionStateInner) -> Fut,
        Fut: Future<Output = ()> + Send,
    {
        for client in self.connected_clients.iter() {
            let Ok(state) = client.read::<()>().map(|state| state.clone()) else {
                return;
            };

            f(state).await
        }
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
    pub async fn disconnect(&self, state: &AppState) {
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
            .filter_map(|user| user.read::<()>().ok().map(|user| user.writer.clone()))
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

#[derive(Clone)]
pub struct ConnectionState(pub Arc<RwLock<ConnectionStateInner>>);

impl ConnectionState {
    pub fn read<T: Debug>(&self) -> Result<RwLockReadGuard<'_, ConnectionStateInner>, APIError<T>> {
        let Ok(conn_state) = self.0.read() else {
            log::error!("Poisoned lock for the connection state");

            return Err(APIError::ServerErr(ServerErr::InternalErr));
        };

        Ok(conn_state)
    }

    pub fn write<T: Debug>(
        &self,
    ) -> Result<RwLockWriteGuard<'_, ConnectionStateInner>, APIError<T>> {
        let Ok(conn_state) = self.0.write() else {
            log::error!("Poisoned lock for the connection state");

            return Err(APIError::ServerErr(ServerErr::InternalErr));
        };

        Ok(conn_state)
    }
}

pub async fn init_state() -> AppState {
    let db = Database::connect("sqlite://db.sqlite?mode=rwc")
        .await
        .unwrap();

    db.get_schema_registry("server::entity::*")
        .sync(&db)
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
