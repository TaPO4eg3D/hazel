use std::{
    cell::RefCell, net::{SocketAddr, UdpSocket}, sync::{Arc, Mutex, RwLock, Weak, atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering}}, thread, time::{Duration, Instant}
};

use atomic_float::AtomicF32;
use bytes::{Bytes, BytesMut};
use capture::audio::{self, Capture, Playback, StreamingClientState};
use gpui::{App, AppContext, AsyncApp, Global};

use rpc::models::markers::UserId;
use streaming_common::{FFMpegPacketPayload, UDPPacket, UDPPacketType};

type Addr = Arc<Mutex<Option<(UserId, SocketAddr)>>>;

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

struct SenderState {
    transmit_volume: AtomicF32,
    is_talking: AtomicBool,
}

impl SenderState {
    fn new() -> Self {
        Self {
            is_talking: AtomicBool::new(false),
            transmit_volume: AtomicF32::new(0.010),
        }
    }
}

fn spawn_sender(
    addr: Addr,
    socket: Arc<UdpSocket>,
    state: Arc<SenderState>,
    capture: Capture,
) {
    let mut buf = BytesMut::new();
    let mut recv = capture.get_recv();

    let last_silence = RefCell::new(Some(Instant::now()));

    loop {
        let transmit_volume = state.transmit_volume.load(Ordering::Relaxed);

        let mut encoded_recv = recv.recv_encoded_with(|samples| {
            let max_volume = *(samples
                .iter()
                .max_by(|a, b| a.total_cmp(b))
                .unwrap());

            if max_volume < transmit_volume {
                let now = Instant::now();

                let silence = { *last_silence.borrow() };
                match silence {
                    Some(value) => {
                        if now - value > Duration::from_millis(400) {
                            state.is_talking.store(false, Ordering::Relaxed);

                            return None;
                        }
                    },
                    None => {
                        *last_silence.borrow_mut() = Some(now)
                    }
                }
            } else {
                state.is_talking.store(true, Ordering::Relaxed);

                *last_silence.borrow_mut() = None;
            }

            Some(samples)
        });

        while let Some(audio_packet) = encoded_recv.pop() {
            if let Some((user_id, addr)) = *addr.lock().unwrap() {
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

struct GlobalStreaming {
    capture: Capture,
    playback: Playback,

    stream_addr: Addr,

    reciever_state: Arc<Mutex<ReceiverState>>,
    sender_state: Arc<SenderState>,
}

impl Global for GlobalStreaming {}

pub struct Streaming {}

impl Streaming
{
    pub fn is_talking<C: AppContext>(cx: &C) -> bool {
        cx.read_global(|stream: &GlobalStreaming, _| {
            stream.sender_state.is_talking.load(Ordering::Relaxed)
        })
    }

    pub fn get_playback<C: AppContext>(cx: &C) -> Playback {
        cx.read_global(|stream: &GlobalStreaming, _| {
            stream.playback.clone()
        })
    }

    pub fn get_capture<C: AppContext>(cx: &C) -> Capture {
        cx.read_global(|stream: &GlobalStreaming, _| {
            stream.capture.clone()
        })
    }

    pub fn connect<C: AppContext>(cx: &C, user_id: UserId, addr: SocketAddr) {
        cx.read_global(|stream: &GlobalStreaming, _| {
            let mut state = stream.stream_addr.lock()
                .unwrap();

            *state = Some((user_id, addr));

            stream.capture.set_enabled(true);
        });
    }

    pub fn add_voice_member<C: AppContext>(cx: &C, shared: Weak<VoiceMemberSharedData>) {
        cx.read_global(|stream: &GlobalStreaming, _| {
            let mut state = stream.reciever_state.lock()
                .unwrap();

            state.voice_members.push(VoiceMember::new(shared));
        });
    }
}

pub fn init(cx: &mut App) {
    let stream_addr: Addr = Arc::new(Mutex::new(None));

    let socket = Arc::new(UdpSocket::bind("0.0.0.0:0").unwrap());
    let (capture, playback) = audio::init();

    let sender_state = Arc::new(SenderState::new());
    let reciever_state = Arc::new(Mutex::new(ReceiverState::default()));

    thread::spawn({
        let addr = stream_addr.clone();
        let capture = capture.clone();
        let socket = socket.clone();
        let state = sender_state.clone();

        move || {
            spawn_sender(addr, socket, state, capture);
        }
    });

    thread::spawn({
        let socket = socket.clone();
        let playback = playback.clone();
        let state = reciever_state.clone();

        move || {
            spawn_receiver(socket, playback, state);
        }
    });

    cx.set_global(GlobalStreaming { 
        capture,
        playback,
        sender_state,
        stream_addr,
        reciever_state,
    });
}
