use anyhow::Result as AResult;
use bytes::BytesMut;
use tokio::net::UdpSocket;

use crate::config::Config;
use streaming_common::UDPPacket;


pub async fn open_udp_socket(
    udp_addr: &str
) -> AResult<()> {
    let sock = UdpSocket::bind(udp_addr)
        .await
        .unwrap();

    let mut buf = BytesMut::with_capacity(4800 * 10);

    loop {
        let bytes_read = sock.recv(&mut buf).await?;

        if bytes_read > 0 {
            let packet = UDPPacket::parse(&mut buf);
        }

        buf.clear();
    }
}
