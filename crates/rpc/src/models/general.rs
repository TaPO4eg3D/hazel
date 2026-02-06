use rpc_macros::RPCNotification;
use serde::{Deserialize, Serialize};

use crate::models::markers::UserId;

#[derive(Serialize, Deserialize, Debug)]
pub enum UserConnectionUpdateMessage {
    UserConnected,
    UserDisconnected,
}

#[derive(Serialize, Deserialize, Debug, RPCNotification)]
pub struct UserConnectionUpdate {
    pub user_id: UserId,
    pub message: UserConnectionUpdateMessage,
}

