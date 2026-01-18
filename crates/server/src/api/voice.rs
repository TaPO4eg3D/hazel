use std::sync::mpsc::channel;

use rpc::common::Empty;
use rpc::models::common::{APIError, APIResult, RPCMethod};
use rpc::models::markers::TaggedEntity;
use rpc::models::voice::{
    GetVoiceChannels, JoinVoiceChannel, JoinVoiceChannelError, JoinVoiceChannelPayload,
    VoiceChannelMember, VoiceChannelUpdate, VoiceChannelUpdateMessage,
};
use rpc::server::RpcRouter;

use rpc::{self, check_auth, models};

use crate::api::common::{DbErrReponseCompat, RPCServer};
use crate::entity::{user::Entity as User, voice_channel::Entity as VoiceChannel};
use crate::{AppState, ConnectionState, register_endpoints};

use sea_orm::prelude::*;

impl RPCServer for GetVoiceChannels {
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
                if let Some(user_ids) = connected_users {
                    let mut members = vec![];

                    for user_id in user_ids.iter() {
                        let user = User::find_by_id(user_id.value)
                            .one(&app_state.db)
                            .await
                            .map_err(DbErr::into_api_error)?;

                        let Some(user) = user else {
                            log::error!(
                                "Connected (ChannelID: {}) user (ID {}) does not exist in the DB!",
                                channel.id,
                                user_id.value,
                            );

                            continue;
                        };

                        members.push(VoiceChannelMember {
                            id: *user_id,
                            name: user.username,
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

impl RPCServer for JoinVoiceChannel {
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

        // Update global state in a block to not hold
        // the lock for a long time
        {
            app_state
                .channels
                .voice_channels
                .entry(channel_id)
                .and_modify(|v| {
                    v.push(current_user_id);
                })
                .or_insert_with(|| vec![current_user_id]);
        }

        // Update connection state in a block to not hold
        // the lock for a long time
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

            writer
                .write(
                    "VoiceChannelUpdate".into(),
                    VoiceChannelUpdate {
                        channel_id,
                        message: VoiceChannelUpdateMessage::UserConnected(current_user_id),
                    },
                    None,
                )
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
    )
}
