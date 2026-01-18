use std::sync::mpsc::channel;

use rpc::common::Empty;
use rpc::models::common::{APIError, APIResult};
use rpc::models::markers::TaggedEntity;
use rpc::models::voice::{
    JoinVoiceChannelError, JoinVoiceChannelPayload, VoiceChannelMember, VoiceChannelUpdate,
    VoiceChannelUpdateMessage,
};
use rpc::server::RpcRouter;

use rpc::{self, check_auth, models};

use crate::api::common::DbErrReponseCompat;
use crate::entity::{user::Entity as User, voice_channel::Entity as VoiceChannel};
use crate::{AppState, ConnectionState};

use sea_orm::prelude::*;

async fn get_voice_channels(
    state: AppState,
    conn_state: ConnectionState,
    _: Empty,
) -> APIResult<Vec<models::voice::VoiceChannel>, ()> {
    check_auth!(conn_state);

    let voice_channels = VoiceChannel::find()
        .all(&state.db)
        .await
        .map_err(DbErr::into_api_error)?;

    let mut result = Vec::new();
    for channel in voice_channels.into_iter() {
        let connected_users = state.channels.voice_channels.get(&channel.tagged_id());

        let members = {
            if let Some(user_ids) = connected_users {
                let mut members = vec![];

                for user_id in user_ids.iter() {
                    let user = User::find_by_id(user_id.value)
                        .one(&state.db)
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

async fn join_voice_channel(
    state: AppState,
    conn_state: ConnectionState,
    payload: JoinVoiceChannelPayload,
) -> APIResult<(), JoinVoiceChannelError> {
    check_auth!(conn_state);

    let exists = VoiceChannel::find_by_id(payload.channel_id.value)
        .exists(&state.db)
        .await
        .map_err(DbErr::into_api_error)?;

    if !exists {
        return Err(APIError::Err(JoinVoiceChannelError::DoesNotExist));
    }

    let current_user_id = {
        conn_state
            .read()
            .unwrap()
            .get_user_id()
            .expect("We checked auth above")
    };

    // Update global state in a block to not hold 
    // the lock for a long time
    {
        state
            .channels
            .voice_channels
            .entry(payload.channel_id)
            .and_modify(|v| {
                v.push(current_user_id);
            })
            .or_insert_with(|| vec![current_user_id]);
    }

    // Update connection state in a block to not hold 
    // the lock for a long time
    {
        let mut conn_state = conn_state.write()
            .unwrap();

        conn_state.active_voice_channel = Some(payload.channel_id);
    }

    for value in state.connected_clients.iter() {
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
                    channel_id: payload.channel_id,
                    message: VoiceChannelUpdateMessage::UserConnected(current_user_id),
                },
                None,
            )
            .await;
    }

    Ok(())
}

async fn get_udp_port(
    _state: AppState,
    _conn_state: ConnectionState,
    _: Empty,
) -> APIResult<String, ()> {
    // TODO: Implement
    Ok("9899".into())
}

pub fn merge(router: RpcRouter<AppState, ConnectionState>) -> RpcRouter<AppState, ConnectionState> {
    router
        .register("JoinVoiceChannel", join_voice_channel)
        .register("GetVoiceChannels", get_voice_channels)
        .register("GetUdpPort", get_udp_port)
}
