use anyhow::Result as AResult;
use bytes::BytesMut;
use rpc::models::markers::{Id, User};
use tokio::net::UdpSocket;

use crate::AppState;
use streaming_common::UDPPacket;

pub async fn open_udp_socket(state: AppState, udp_addr: &str) -> AResult<()> {
    let sock = UdpSocket::bind(udp_addr).await.unwrap();

    // Two seconds of dual channel 48kHz if we don't
    // count the size of UDPPacket header
    let mut buf = BytesMut::with_capacity(4800 * 4);

    loop {
        buf.clear();
        buf.resize(4800 * 4, 0);

        let (bytes_read, addr) = sock.recv_from(&mut buf).await?;

        if bytes_read == 0 {
            continue;
        }
        buf.truncate(bytes_read);

        // To parse data but keep original bytes intact
        let buf = buf.split().freeze();
        let packet = {
            let mut buf = buf.clone();

            UDPPacket::parse(&mut buf)
        };
        let currend_user_id = Id::<User>::new(packet.user_id);

        let (voice_channel, addr_differs) = match state.connected_clients.get(&currend_user_id) {
            Some(state) => {
                let state = state.read().unwrap();

                let Some(channel_id) = state.active_voice_channel else {
                    continue;
                };

                if let Some(curr_addr) = state.active_stream {
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
            let Some(state) = state.connected_clients.get(&currend_user_id) else {
                continue;
            };

            let mut state = state.write().unwrap();

            state.active_stream = Some(addr);
        }

        let Some(voice_users) = state.channels.voice_channels.get(&voice_channel) else {
            continue;
        };

        for user in voice_users.iter() {
            if user.id == currend_user_id {
                continue;
            }

            if let Some(user) = state.connected_clients.get(&user.id) {
                let addr = { user.read().unwrap().active_stream };

                if let Some(addr) = addr {
                    _ = sock.send_to(&buf[..bytes_read], addr).await;
                }
            }
        }
    }
}
