use std::{
    net::{SocketAddr, UdpSocket}, sync::{Arc, RwLock, atomic::AtomicUsize}, thread
};

use bytes::{Bytes, BytesMut};
use capture::audio::linux::{Audio, RegisteredClient};
use gpui::{App, AppContext, AsyncApp, Global};

use rpc::models::markers::UserId;
use streaming_common::{FFMpegPacketPayload, UDPPacket, UDPPacketType};

type Addr = Arc<RwLock<Option<(i32, SocketAddr)>>>;

pub enum StreamingMessage {
    Connect((i32, SocketAddr)),
    Disconnect,
}

fn init_sender(
    socket: Arc<UdpSocket>,
    addr: Addr,
    packet_recv: std::sync::mpsc::Receiver<FFMpegPacketPayload>,
) {
    let mut buf = BytesMut::new();

    while let Ok(packet) = packet_recv.recv() {
        let addr = addr.read().unwrap();
        let Some((user_id, addr)) = *addr else {
            continue;
        };

        let msg = UDPPacket {
            user_id,
            payload: UDPPacketType::Voice(packet),
        };
        msg.to_bytes(&mut buf);

        let buf = buf.split();
        _ = socket.send_to(&buf[..], addr);
    }
}

fn init_reciever(
    socket: Arc<UdpSocket>,
    audio: Audio,
) {
    let mut buf = BytesMut::with_capacity(4800 * 10);
    let mut clients: Vec<RegisteredClient> = vec![];

    loop {
        buf.clear();
        buf.resize(4800 * 10, 0);

        if let Ok(len) = socket.recv(&mut buf[..]) {
            buf.truncate(len);

            let mut buf: Bytes = buf.split().into();
            let packet = UDPPacket::parse(&mut buf);

            let client = clients.iter()
                .find(|client| client.user_id == packet.user_id);

            let client = match client {
                Some(client) => client,
                None => {
                    let client = audio.register_client(packet.user_id);
                    clients.push(client);

                    clients.last().unwrap()
                }
            };

            match packet.payload {
                UDPPacketType::Voice(packet) => {
                    _ = client.packet_sender.send(packet);
                },
                _ => todo!(),
            }
        }
    }
}

fn _init(tx: std::sync::mpsc::Receiver<StreamingMessage>) {
    let addr: Addr = Arc::new(RwLock::new(None));

    let socket = Arc::new(UdpSocket::bind("0.0.0.0:0").unwrap());

    let (audio, packet_recv) = Audio::new().unwrap();

    thread::spawn({
        let addr = addr.clone();
        let socket = socket.clone();

        move || {
            init_sender(socket, addr, packet_recv);
        }
    });

    thread::spawn({
        let socket = socket.clone();
        let audio = audio.clone();

        move || {
            init_reciever(socket, audio);
        }
    });

    loop {
        if let Ok(msg) = tx.recv() {
            match msg {
                StreamingMessage::Connect(new_addr) => {
                    let mut addr = addr.write().unwrap();
                    *addr = Some(new_addr);

                    audio.set_capture(true);
                }
                StreamingMessage::Disconnect => {
                    let mut addr = addr.write().unwrap();

                    *addr = None;
                    audio.set_capture(false);
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
            stream.tx.send(StreamingMessage::Connect((user_id.value, addr)))
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
