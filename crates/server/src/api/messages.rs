use std::sync::Arc;

use rpc::{models::messages::{SendMessagePayload, TextMessageChannel}, server::RpcRouter};

use crate::{AppState, ConnectionState};

async fn send_message(
    state: AppState,
    conn_state: ConnectionState,
    SendMessagePayload {
        content,
        destination,
    }: SendMessagePayload,
) -> Result<(), String> {
    Ok(())
}

pub fn merge(router: RpcRouter<AppState, ConnectionState>) -> RpcRouter<AppState, ConnectionState> {
    router
        .register("SendMessage", send_message)
}
