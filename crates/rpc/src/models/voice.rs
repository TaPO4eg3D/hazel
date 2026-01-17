use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::models::markers::{UserId, VoiceChannelId};

#[derive(Serialize, Deserialize, Debug)]
pub struct VoiceChannelMember {
    pub id: UserId,
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct VoiceChannel {
    pub id: VoiceChannelId,
    pub name: String,

    pub members: Vec<VoiceChannelMember>
}

#[derive(Serialize, Deserialize, Debug)]
pub struct VoiceChannelUpdate {
    pub channel_id: VoiceChannelId,
    pub message: VoiceChannelUpdateMessage,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum VoiceChannelUpdateMessage {
    UserConnected(UserId),
    UserDisconnected(UserId),
}

#[derive(Serialize, Deserialize)]
#[derive(Error, Debug)]
pub enum GetVoiceChannelsError {
    #[error("Unauthorized access")]
    Unauthorized,
}
