use anyhow::Result as AResult;
use bytes::BytesMut;
use rpc::models::markers::{Id, User, UserId};
use tokio::net::UdpSocket;

use crate::{AppState, config::Config};
use streaming_common::UDPPacket;


pub async fn open_udp_socket(
    state: AppState,
    udp_addr: &str
) -> AResult<()> {
    let sock = UdpSocket::bind(udp_addr)
        .await
        .unwrap();

    let mut buf = BytesMut::with_capacity(4800 * 10);

    let active_streams = state.active_streams.pin_owned();
    loop {
        let (bytes_read, addr) = sock.recv_from(&mut buf).await?;
        if bytes_read == 0 {
            continue;
        }

        let packet = UDPPacket::parse(&mut buf);
        let user_id = Id::<User>::new(packet.user_id);

        // match active_streams.get(&user_id) {
        //     Some(stream) if stream.addr != addr => {
        //         active_streams.update(user_id, |stream| {
        //             let mut stream = stream.clone();
        //             stream.addr = addr;
        //
        //             stream
        //         });
        //     },
        //     None => {
        //         active_streams.insert(user_id, );
        //     }
        //     _ => {},
        // }

        buf.clear();
    }
}
