use anyhow::Result as AResult;
use bytes::BytesMut;
use rpc::models::markers::{Id, User};
use tokio::net::UdpSocket;

use crate::state::AppState;
use streaming_common::{UDPPacket, UDPPayloadType};

// TODO: Somehow implement authorized socket communication.
// Currenlty it is possible to do quite nasty stuff
pub async fn open_udp_socket(app_state: AppState, udp_addr: &str) -> AResult<()> {
    let socket = UdpSocket::bind(udp_addr)
        .await
        .expect("Failed to bind a UDP socket");

    loop {
        let mut buf = BytesMut::with_capacity(4096);
        let (bytes_read, addr) = socket.recv_buf_from(&mut buf).await?;

        if bytes_read == 0 {
            continue;
        }

        let buf = buf.split_to(bytes_read).freeze();
        let mut parse_bytes = buf.clone();

        let Ok(packet) = UDPPacket::parse(&mut parse_bytes) else {
            continue;
        };

        let current_user_id = Id::<User>::new(packet.user_id);

        // No need to process, the client wants to maintain UDP connection
        if matches!(packet.payload, UDPPayloadType::Ping(_)) {
            let Some(state) = app_state.connected_clients.get(&current_user_id) else {
                continue;
            };

            if let Ok(mut state) = state.write::<()>() {
                state.active_stream = Some(addr);
            }

            continue;
        }

        let (voice_channel, addr_differs) = match app_state.connected_clients.get(&current_user_id)
        {
            Some(user_state) => {
                let Ok(user_state) = user_state.read::<()>() else {
                    continue;
                };

                let Some(channel_id) = user_state.active_voice_channel else {
                    continue;
                };

                if let Some(curr_addr) = user_state.active_stream {
                    (channel_id, curr_addr != addr)
                } else {
                    (channel_id, true)
                }
            }
            None => {
                continue;
            }
        };

        if addr_differs {
            let Some(state) = app_state.connected_clients.get(&current_user_id) else {
                continue;
            };

            if let Ok(mut state) = state.write::<()>() {
                state.active_stream = Some(addr);
            }
        }

        let Some(voice_users) = app_state.channels.voice_channels.get(&voice_channel) else {
            continue;
        };

        match packet.payload {
            UDPPayloadType::Audio(_) => {
                for user in voice_users.iter() {
                    if user.id == current_user_id {
                        continue;
                    }

                    if let Some(user) = app_state.connected_clients.get(&user.id) {
                        let addr = user.read::<()>().ok().and_then(|user| user.active_stream);

                        if let Some(addr) = addr {
                            _ = socket.send_to(&buf[..bytes_read], addr).await;
                        }
                    }
                }
            }
            UDPPayloadType::Video(_) => {
                let Some(host) = voice_users.iter().find(|user| user.id == current_user_id) else {
                    continue;
                };

                let Some(video_session) = host.screencast_session.as_ref() else {
                    continue;
                };

                for user_id in video_session.connected_clients.iter() {
                    if let Some(user) = app_state.connected_clients.get(user_id) {
                        let addr = user.read::<()>().ok().and_then(|user| user.active_stream);

                        if let Some(addr) = addr {
                            _ = socket.send_to(&buf[..bytes_read], addr).await;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
