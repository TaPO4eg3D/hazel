use rpc::common::Empty;
use rpc::models::markers::TaggedEntity;
use rpc::models::voice::VoiceChannelMember;
use rpc::server::RpcRouter;
use rpc::models::common::{APIError, APIResult};

use rpc::{self, check_auth, models};

use crate::api::common::DbErrReponseCompat;
use crate::{AppState, ConnectionState};
use crate::entity::{
    user::Entity as User,
    voice_channel::Entity as VoiceChannel,
};

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
        let connected_users = state
            .channels
            .voice_channels
            .get(&channel.tagged_id());

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
            }

            vec![]
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

pub fn merge(router: RpcRouter<AppState, ConnectionState>) -> RpcRouter<AppState, ConnectionState> {
    router
        .register("GetVoiceChannels", get_voice_channels)
}
