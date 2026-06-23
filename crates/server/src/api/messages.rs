use rpc::models::messages::SendMessagePayload;

use crate::{AppRouter, AppState, ConnectionState};

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

pub fn register(router: AppRouter) -> AppRouter {
    router.register("SendMessage", send_message)
}
