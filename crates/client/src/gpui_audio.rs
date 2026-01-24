use std::{
    net::{SocketAddr, UdpSocket}, sync::{Arc, Mutex, RwLock, Weak, atomic::{AtomicBool, AtomicUsize, Ordering}}, thread, time::{Duration, Instant}
};

use bytes::{Bytes, BytesMut};
use capture::audio::{self, Capture, Playback, StreamingClientState};
use gpui::{App, AppContext, AsyncApp, Global};

use rpc::models::markers::UserId;
use streaming_common::{FFMpegPacketPayload, UDPPacket, UDPPacketType};

type Addr = Arc<RwLock<Option<(UserId, SocketAddr)>>>;

pub struct VoiceMemberSharedData {
    id: UserId,
    last_packet: RwLock<Instant>,
}

impl VoiceMemberSharedData {
    pub fn new(id: UserId) -> Self {
        Self {
            id,
            last_packet: RwLock::new(Instant::now()),
        }
    }

    pub fn is_talking(&self) -> bool {
        let now = Instant::now();
        let last_packet = self.last_packet.read()
            .unwrap();

        now - *last_packet < Duration::from_millis(250)
    }
    
    fn update_timestamp(&self) {
        let mut last_packet = self.last_packet.write()
            .unwrap();

        *last_packet = Instant::now();
    }
}

pub enum StreamingMessage {
    /// Connect to an UDP socket
    Connect((UserId, SocketAddr)),
    // Disconnect from an active UDP socket
    Disconnect,
    /// Add voice member for tracking
    AddVoiceMember(Weak<VoiceMemberSharedData>),
}

struct VoiceMember {
    shared_state: Weak<VoiceMemberSharedData>,
    streaming_state: StreamingClientState,
}

impl VoiceMember {
    fn new(shared: Weak<VoiceMemberSharedData>) -> Self { 
        let user_id = shared.upgrade().unwrap().id;

        Self {
            shared_state: shared,
            streaming_state: StreamingClientState::new(user_id.value)
        }
    }
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
                    user_id: user_id.value,
                    payload: UDPPacketType::Voice(audio_packet),
                };

                udp_packet.to_bytes(&mut buf);

                _ = socket.send_to(&buf, addr);
            }
        }
    }
}

#[derive(Default)]
struct ReceiverState {
    voice_members: Vec<VoiceMember>,
}

impl ReceiverState {
    fn cleanup(&mut self) {
        self.voice_members
            .retain(|member| {
                member.shared_state.strong_count() != 0
            });
    }
}

impl ReceiverState {
    pub fn get_voiced_member_mut(&mut self, user_id: i32) -> Option<&mut VoiceMember> {
        self.voice_members.iter_mut()
            .find(|client| {
                if let Some(client) = client.shared_state.upgrade() {
                    return client.id.value == user_id
                }

                false
            })
    }
}

fn spawn_receiver(
    socket: Arc<UdpSocket>,
    playback: Playback,
    state: Arc<Mutex<ReceiverState>>,
) {
    let mut buf = BytesMut::with_capacity(4800 * 2);

    loop {
        buf.clear();
        buf.resize(4800 * 2, 0);

        if let Ok(len) = socket.recv(&mut buf[..]) {
            buf.truncate(len);

            let mut buf: Bytes = buf.split().into();
            let packet = UDPPacket::parse(&mut buf);

            let mut state = state.lock().unwrap();
            state.cleanup();

            let Some(member) = state.get_voiced_member_mut(packet.user_id) else {
                continue
            };

            match packet.payload {
                UDPPacketType::Voice(packet) => {
                    if let Some(shared_state) = member.shared_state.upgrade() {
                        shared_state.update_timestamp();
                    }
                    member.streaming_state.push(packet);
                    
                    playback.process_streaming(
                        state.voice_members.iter_mut()
                            .map(|member| &mut member.streaming_state)
                    );
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

    let reciver_state = Arc::new(Mutex::new(ReceiverState::default()));

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
        let state = reciver_state.clone();

        move || {
            spawn_receiver(socket, playback, state);
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
                },
                StreamingMessage::AddVoiceMember(shared) => {
                    let mut state = reciver_state.lock().unwrap();
                    state.voice_members.push(VoiceMember::new(shared));
                },
            }
        }
    }
}

struct GlobalStreaming {
    tx: std::sync::mpsc::Sender<StreamingMessage>,
}

impl Global for GlobalStreaming {}

pub struct Streaming {}

impl Streaming
{
    pub fn connect<C: AppContext>(cx: &C, user_id: UserId, addr: SocketAddr) {
        cx.read_global(|stream: &GlobalStreaming, _| {
            _ = stream.tx.send(StreamingMessage::Connect((user_id, addr)))
        });
    }

    pub fn add_voice_member<C: AppContext>(cx: &C, shared: Weak<VoiceMemberSharedData>) {
        cx.read_global(|stream: &GlobalStreaming, _| {
            _ = stream.tx.send(StreamingMessage::AddVoiceMember(shared))
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
