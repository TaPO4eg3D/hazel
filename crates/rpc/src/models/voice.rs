use rpc_macros::{RPCNotification, rpc_method};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    common::Empty,
    models::markers::{UserId, VoiceChannelId},
};

#[derive(Serialize, Deserialize, Debug)]
pub struct VoiceChannelMember {
    pub id: UserId,
    pub name: String,

    pub state: VoiceChannelUserState,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct VoiceChannel {
    pub id: VoiceChannelId,
    pub name: String,

    pub members: Vec<VoiceChannelMember>,
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

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
pub struct VoiceChannelUserState {
    pub is_mic_off: bool,
    pub is_sound_off: bool,
    pub is_streaming: bool,
}

#[rpc_method]
pub struct UpdateVoiceChannelUserState {
    request: VoiceChannelUserState,
    response: (),
    error: (),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum VoiceChannelUpdateMessage {
    UserConnected(UserId),
    UserDisconnected(UserId),
    UserStateUpdated((UserId, VoiceChannelUserState)),
}

#[derive(Serialize, Deserialize, Error, Debug)]
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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VideoSessionParams {
    pub shard_size: u32,
    pub network_loss: f32,
}

impl Default for VideoSessionParams {
    fn default() -> Self {
        Self {
            shard_size: 1280,
            network_loss: 0.2,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, RPCNotification)]
pub enum OwnedScreenCastUpdate {
    ParamsUpdated(VideoSessionParams),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum WatchedScreenCastUpdateMessage {
    ParamsUpdated(VideoSessionParams),
    SessionEnded,
}

#[derive(Serialize, Deserialize, Debug, RPCNotification)]
pub struct WatchedScreenCastUpdate {
    pub user_id: UserId,
    pub message: WatchedScreenCastUpdateMessage,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum HostScreenCastError {
    NotConnectedToVoiceChannel,
}

#[rpc_method]
pub struct StartScreenCast {
    request: Empty,
    response: VideoSessionParams,
    error: HostScreenCastError,
}

#[rpc_method]
pub struct StopScreenCast {
    request: Empty,
    response: (),
    error: HostScreenCastError,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct JoinScreenCastRequest {
    pub user_id: UserId,
    pub mtu: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum WatchScreenCastError {
    NotConnectedToVoiceChannel,
    InvalidHostId,
    NoSuchStreamAvailable,
}

#[rpc_method]
pub struct JoinScreenCast {
    request: JoinScreenCastRequest,
    response: VideoSessionParams,
    error: WatchScreenCastError,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LeaveScreenCastRequest {
    pub user_id: UserId,
}

#[rpc_method]
pub struct LeaveScreenCast {
    request: LeaveScreenCastRequest,
    response: (),
    error: WatchScreenCastError,
}
