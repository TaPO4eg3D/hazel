use std::{
    net::{SocketAddr, UdpSocket}, sync::{Arc, RwLock, atomic::AtomicUsize}, thread
};

use bytes::{Bytes, BytesMut};
use capture::audio::{self, Capture, Playback, StreamingClient};
use gpui::{App, AppContext, AsyncApp, Global};

use rpc::models::markers::UserId;
use streaming_common::{FFMpegPacketPayload, UDPPacket, UDPPacketType};

type Addr = Arc<RwLock<Option<(i32, SocketAddr)>>>;

pub enum StreamingMessage {
    Connect((i32, SocketAddr)),
    Disconnect,
}

fn spawn_sender(
    addr: Addr,
    socket: Arc<UdpSocket>,
    capture: Capture,
) {
    let mut buf = BytesMut::new();
    let mut recv = capture.get_recv();

    loop {
        let mut encoded_recv = recv.recv_encoded();

        while let Some(audio_packet) = encoded_recv.pop() {
            if let Some((user_id, addr)) = *addr.read().unwrap() {
                buf.clear();

                let udp_packet = UDPPacket {
                    user_id,
                    payload: UDPPacketType::Voice(audio_packet),
                };

                udp_packet.to_bytes(&mut buf);

                _ = socket.send_to(&buf, addr);
            }
        }
    }
}

fn spawn_receiver(
    socket: Arc<UdpSocket>,
    playback: Playback,
) {
    let mut buf = BytesMut::with_capacity(4800 * 2);
    let mut clients: Vec<StreamingClient> = vec![];

    loop {
        buf.clear();
        buf.resize(4800 * 2, 0);

        if let Ok(len) = socket.recv(&mut buf[..]) {
            buf.truncate(len);

            let mut buf: Bytes = buf.split().into();
            let packet = UDPPacket::parse(&mut buf);

            let client = clients.iter_mut()
                .find(|client| client.user_id == packet.user_id);

            let client = match client {
                Some(client) => client,
                None => {
                    let client = StreamingClient::new(packet.user_id);
                    clients.push(client);

                    clients.last_mut().unwrap()
                }
            };

            match packet.payload {
                UDPPacketType::Voice(packet) => {
                    client.push(packet);

                    playback.process_streaming(&mut clients);
                },
                _ => todo!(),
            }
        }
    }
}

fn _init(tx: std::sync::mpsc::Receiver<StreamingMessage>) {
    let addr: Addr = Arc::new(RwLock::new(None));

    let socket = Arc::new(UdpSocket::bind("0.0.0.0:0").unwrap());
    let (capture, playback) = audio::init();

    thread::spawn({
        let addr = addr.clone();
        let capture = capture.clone();
        let socket = socket.clone();

        move || {
            spawn_sender(addr, socket, capture);
        }
    });

    thread::spawn({
        let socket = socket.clone();
        let playback = playback.clone();

        move || {
            spawn_receiver(socket, playback);
        }
    });

    loop {
        if let Ok(msg) = tx.recv() {
            match msg {
                StreamingMessage::Connect(new_addr) => {
                    let mut addr = addr.write().unwrap();
                    *addr = Some(new_addr);

                    capture.set_enabled(true);
                }
                StreamingMessage::Disconnect => {
                    let mut addr = addr.write().unwrap();

                    *addr = None;
                    capture.set_enabled(false);
                }
            }
        }
    }
}

struct GlobalStreaming {
    tx: std::sync::mpsc::Sender<StreamingMessage>,
}

impl Global for GlobalStreaming {}

pub struct Streaming {}

impl Streaming {
    pub fn connect<C: AppContext>(cx: &C, user_id: UserId, addr: SocketAddr) {
        cx.read_global(|stream: &GlobalStreaming, _| {
            _ = stream.tx.send(StreamingMessage::Connect((user_id.value, addr)))
        });
    }
}

pub fn init(cx: &mut App) {
    let (tx, rx) = std::sync::mpsc::channel();

    thread::spawn(move || {
        _init(rx);
    });

    cx.set_global(GlobalStreaming { tx });
}
