use std::sync::mpsc::channel;

use rpc::common::Empty;
use rpc::models::common::{APIError, APIResult, RPCMethod, RPCNotification};
use rpc::models::markers::TaggedEntity;
use rpc::models::voice::{
    GetVoiceChannels, JoinVoiceChannel, JoinVoiceChannelError, JoinVoiceChannelPayload, LeaveVoiceChannel, UpdateVoiceUserState, VoiceUserState, VoiceChannelMember, VoiceChannelUpdate, VoiceChannelUpdateMessage
};
use rpc::server::RpcRouter;

use rpc::{self, check_auth, models};

use crate::api::common::{DbErrReponseCompat, RPCHandle};
use crate::entity::{user::Entity as User, voice_channel::Entity as VoiceChannel};
use crate::{AppState, ConnectionState, VoiceUser, register_endpoints};

use sea_orm::prelude::*;

impl RPCHandle for GetVoiceChannels {
    async fn handle(
        app_state: AppState,
        connection_state: ConnectionState,
        _req: Empty,
    ) -> APIResult<Vec<models::voice::VoiceChannel>, ()> {
        check_auth!(connection_state);

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

                            is_muted: false,
                            is_sound_off: false,
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

impl RPCHandle for UpdateVoiceUserState {
    async fn handle(
        app_state: AppState,
        connection_state: ConnectionState,
        req: VoiceUserState,
    ) -> APIResult<(), ()> {
        check_auth!(connection_state);

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
            let Some(mut voice_users) = app_state.channels.voice_channels.get_mut(&active_channel) else {
                return Ok(());
            };

            for voice_user in voice_users.iter_mut() {
                if voice_user.id != current_user_id {
                    continue;
                }

                voice_user.is_muted = req.is_mic_off;
                voice_user.is_sound_off = req.is_sound_off;

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
                message: VoiceChannelUpdateMessage::UserStateUpdated((current_user_id, req)),
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
        check_auth!(connection_state);

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
            let mut state = connection_state.write().unwrap();

            state.active_voice_channel = None;
            state.active_stream = None;
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
        check_auth!(connection_state);

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
                    v.push(VoiceUser::new(current_user_id));
                })
                .or_insert_with(|| vec![VoiceUser::new(current_user_id)]);
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

pub fn merge(router: RpcRouter<AppState, ConnectionState>) -> RpcRouter<AppState, ConnectionState> {
    register_endpoints!(
        router,
        GetVoiceChannels,
        JoinVoiceChannel,
        LeaveVoiceChannel,
        UpdateVoiceUserState,
    )
}
