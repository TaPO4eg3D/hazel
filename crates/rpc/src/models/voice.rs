use rpc_macros::{RPCNotification, rpc_method};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{common::Empty, models::markers::{UserId, VoiceChannelId}};

#[derive(Serialize, Deserialize, Debug)]
pub struct VoiceChannelMember {
    pub id: UserId,
    pub name: String,

    pub is_muted: bool,
    pub is_sound_off: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct VoiceChannel {
    pub id: VoiceChannelId,
    pub name: String,

    pub members: Vec<VoiceChannelMember>
}

#[derive(Serialize, Deserialize, Debug, RPCNotification)]
pub struct VoiceChannelUpdate {
    pub channel_id: VoiceChannelId,
    pub message: VoiceChannelUpdateMessage,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct JoinVoiceChannelPayload {
    pub channel_id: VoiceChannelId,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum JoinVoiceChannelError {
    DoesNotExist,
    ChannelIsFull,
}

#[rpc_method]
pub struct JoinVoiceChannel {
    request: JoinVoiceChannelPayload,
    response: (),
    error: JoinVoiceChannelError,
}

#[rpc_method]
pub struct LeaveVoiceChannel {
    request: Empty,
    response: (),
    error: (),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct VoiceUserState {
    pub is_mic_off: bool,
    pub is_sound_off: bool,
}

#[rpc_method]
pub struct UpdateVoiceUserState {
    request: VoiceUserState,
    response: (),
    error: (),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum VoiceChannelUpdateMessage {
    UserConnected(UserId),
    UserDisconnected(UserId),
    UserStateUpdated((UserId, VoiceUserState))
}

#[derive(Serialize, Deserialize)]
#[derive(Error, Debug)]
pub enum GetVoiceChannelsError {
    #[error("Unauthorized access")]
    Unauthorized,
}

#[rpc_method]
pub struct GetVoiceChannels {
    request: Empty,
    response: Vec<VoiceChannel>,
    error: (),
}
