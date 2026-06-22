use rpc::common::Empty;
use rpc::models::common::{APIError, APIResult, RPCMethod, RPCNotification};
use rpc::models::markers::TaggedEntity;
use rpc::models::voice::{
    GetVoiceChannels, JoinScreenCast, JoinScreenCastRequest, JoinVoiceChannel,
    JoinVoiceChannelError, JoinVoiceChannelPayload, LeaveVoiceChannel, StartScreenCast,
    StopScreenCast, UpdateVoiceChannelUserState, VideoSessionParams, VoiceChannelMember,
    VoiceChannelUpdate, VoiceChannelUpdateMessage, VoiceChannelUserState, WatchedScreenCastUpdate,
    WatchedScreenCastUpdateMessage,
};

use rpc::{self, check_auth, models, register_endpoints};

use crate::api::common::{DbErrReponseCompat, RPCHandle};
use crate::entity::{user::Entity as User, voice_channel::Entity as VoiceChannel};
use crate::{AppRouter, AppState, ConnectionState, VideoSession, VoiceChannelUser};

use sea_orm::prelude::*;

impl RPCHandle for GetVoiceChannels {
    async fn handle(
        app_state: AppState,
        _connection_state: ConnectionState,
        _req: Empty,
    ) -> APIResult<Vec<models::voice::VoiceChannel>, ()> {
        let voice_channels = VoiceChannel::find()
            .all(&app_state.db)
            .await
            .map_err(DbErr::into_api_error)?;

        let mut result = Vec::new();
        for channel in voice_channels.into_iter() {
            let connected_users = app_state.channels.voice_channels.get(&channel.tagged_id());

            let members = {
                if let Some(voice_users) = connected_users {
                    let mut members = vec![];

                    for voice_user in voice_users.iter() {
                        let user = User::find_by_id(voice_user.id.value)
                            .one(&app_state.db)
                            .await
                            .map_err(DbErr::into_api_error)?;

                        let Some(user) = user else {
                            log::error!(
                                "Connected (ChannelID: {}) user (ID {}) does not exist in the DB!",
                                channel.id,
                                voice_user.id.value,
                            );

                            continue;
                        };

                        members.push(VoiceChannelMember {
                            id: voice_user.id,
                            name: user.username,
                            state: voice_user.state,
                        });
                    }

                    members
                } else {
                    vec![]
                }
            };

            let item = models::voice::VoiceChannel {
                id: channel.tagged_id(),
                name: channel.name,
                members,
            };
            result.push(item);
        }

        Ok(result)
    }
}

impl RPCHandle for UpdateVoiceChannelUserState {
    async fn handle(
        app_state: AppState,
        connection_state: ConnectionState,
        user_state: VoiceChannelUserState,
    ) -> APIResult<(), ()> {
        let active_channel = {
            let state = connection_state.read().unwrap();

            state.active_voice_channel
        };

        let Some(active_channel) = active_channel else {
            return Ok(());
        };

        let current_user_id = {
            connection_state
                .read()
                .unwrap()
                .get_user_id()
                .expect("We checked auth above")
        };

        {
            let Some(mut voice_users) = app_state.channels.voice_channels.get_mut(&active_channel)
            else {
                return Ok(());
            };

            for voice_user in voice_users.iter_mut() {
                if voice_user.id != current_user_id {
                    continue;
                }

                voice_user.state = user_state;

                break;
            }
        }

        for value in app_state.connected_clients.iter() {
            let Some(user_id) = value.read().unwrap().get_user_id() else {
                continue;
            };

            if user_id == current_user_id {
                continue;
            }

            let writer = value.read().unwrap().writer.clone();

            VoiceChannelUpdate {
                channel_id: active_channel,
                message: VoiceChannelUpdateMessage::UserStateUpdated((current_user_id, user_state)),
            }
            .notify(&writer)
            .await;
        }

        Ok(())
    }
}

impl RPCHandle for LeaveVoiceChannel {
    async fn handle(
        app_state: AppState,
        connection_state: ConnectionState,
        _req: Empty,
    ) -> APIResult<(), ()> {
        let active_channel = {
            let state = connection_state.read().unwrap();

            state.active_voice_channel
        };

        let Some(active_channel) = active_channel else {
            return Ok(());
        };

        let current_user_id = {
            connection_state
                .read()
                .unwrap()
                .get_user_id()
                .expect("We checked auth above")
        };

        // Cleanup user state
        {
            let mut state = connection_state.write().unwrap();

            state.active_voice_channel = None;
            state.active_stream = None;
        }

        // Cleanup channel state
        if let Some(mut users) = app_state.channels.voice_channels.get_mut(&active_channel) {
            users.retain(|user| user.id != current_user_id);
        }

        for value in app_state.connected_clients.iter() {
            let Some(user_id) = value.read().unwrap().get_user_id() else {
                continue;
            };

            if user_id == current_user_id {
                continue;
            }

            let writer = value.read().unwrap().writer.clone();

            VoiceChannelUpdate {
                channel_id: active_channel,
                message: VoiceChannelUpdateMessage::UserDisconnected(current_user_id),
            }
            .notify(&writer)
            .await;
        }

        Ok(())
    }
}

impl RPCHandle for JoinVoiceChannel {
    async fn handle(
        app_state: AppState,
        connection_state: ConnectionState,
        JoinVoiceChannelPayload { channel_id }: JoinVoiceChannelPayload,
    ) -> APIResult<(), JoinVoiceChannelError> {
        let exists = VoiceChannel::find_by_id(channel_id.value)
            .exists(&app_state.db)
            .await
            .map_err(DbErr::into_api_error)?;

        if !exists {
            return Err(APIError::Err(JoinVoiceChannelError::DoesNotExist));
        }

        let current_user_id = {
            connection_state
                .read()
                .unwrap()
                .get_user_id()
                .expect("We checked auth above")
        };

        {
            app_state
                .channels
                .voice_channels
                .entry(channel_id)
                .and_modify(|v| {
                    v.push(VoiceChannelUser::new(current_user_id));
                })
                .or_insert_with(|| vec![VoiceChannelUser::new(current_user_id)]);
        }

        {
            let mut state = connection_state.write().unwrap();
            state.active_voice_channel = Some(channel_id);
        }

        for value in app_state.connected_clients.iter() {
            let user_id = value.read().unwrap().get_user_id();

            let Some(user_id) = user_id else {
                continue;
            };

            if user_id == current_user_id {
                continue;
            }

            let writer = value.read().unwrap().writer.clone();

            VoiceChannelUpdate {
                channel_id,
                message: VoiceChannelUpdateMessage::UserConnected(current_user_id),
            }
            .notify(&writer)
            .await;
        }

        Ok(())
    }
}

impl RPCHandle for StartScreenCast {
    async fn handle(
        app_state: AppState,
        connection_state: ConnectionState,
        _req: Self::Request,
    ) -> APIResult<VideoSessionParams, ()> {
        let (channel_id, host_id) = {
            let conn_state = connection_state.read().unwrap();
            let channel_id = conn_state
                .active_voice_channel
                .ok_or(APIError::ServerError)?;

            let user_id = conn_state.get_user_id().ok_or(APIError::ServerError)?;

            (channel_id, user_id)
        };

        let mut channel_users = app_state
            .channels
            .voice_channels
            .get_mut(&channel_id)
            .ok_or(APIError::ServerError)?;

        let user_state = channel_users
            .iter_mut()
            .find(|user| user.id == host_id)
            .ok_or(APIError::ServerError)?;

        let video_session = VideoSession::default();
        let params = video_session.params.clone();

        user_state.screencast_session = Some(video_session);

        Ok(params)
    }
}

impl RPCHandle for StopScreenCast {
    async fn handle(
        app_state: AppState,
        connection_state: ConnectionState,
        _req: Self::Request,
    ) -> APIResult<(), ()> {
        let (channel_id, host_id) = {
            let conn_state = connection_state.read().unwrap();
            let channel_id = conn_state
                .active_voice_channel
                .ok_or(APIError::ServerError)?;

            let user_id = conn_state.get_user_id().ok_or(APIError::ServerError)?;

            (channel_id, user_id)
        };

        let mut channel_users = app_state
            .channels
            .voice_channels
            .get_mut(&channel_id)
            .ok_or(APIError::ServerError)?;

        let host_state = channel_users
            .iter_mut()
            .find(|user| user.id == host_id)
            .ok_or(APIError::ServerError)?;

        if let Some(session) = host_state.screencast_session.as_ref() {
            for user in session.connected_clients.iter() {
                let Some(conn) = app_state.connected_clients.get(user) else {
                    continue;
                };

                let writer = conn.write().unwrap().writer.clone();

                WatchedScreenCastUpdate {
                    user_id: host_id,
                    message: WatchedScreenCastUpdateMessage::SessionEnded,
                }
                .notify(&writer)
                .await;
            }
        }

        host_state.screencast_session = None;

        Ok(())
    }
}

impl RPCHandle for JoinScreenCast {
    async fn handle(
        _app_state: AppState,
        connection_state: ConnectionState,
        JoinScreenCastRequest { user_id, mtu }: JoinScreenCastRequest,
    ) -> APIResult<VideoSessionParams, ()> {
        check_auth!(connection_state);

        Ok(VideoSessionParams::default())
    }
}

pub fn register(router: AppRouter) -> AppRouter {
    register_endpoints!(
        router,
        GetVoiceChannels,
        JoinVoiceChannel,
        LeaveVoiceChannel,
        UpdateVoiceChannelUserState,
        StartScreenCast,
        StopScreenCast,
        JoinScreenCast,
    )
}
