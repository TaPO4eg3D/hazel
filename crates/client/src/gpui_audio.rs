use std::{
    cell::RefCell,
    net::{SocketAddr, UdpSocket},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use atomic_float::AtomicF32;
use bytes::{Bytes, BytesMut};
use capture::audio::{
    self, DeviceRegistry,
    capture::{Capture, CaptureController},
    playback::{
        AudioStreamingClientSharedState, PlaybackController, PlaybackOutputState,
        PlaybackPacketCommand, PlaybackPacketInput,
    },
};
use crossbeam::channel;
use gpui::{App, AppContext, Global};

use rpc::models::markers::UserId;
use streaming_common::{EncodedAudioPacket, UDPPacket, UDPPacketType};

use crate::components::streaming_state::{AtomicNoiseReductionAlgorithm, NoiseReductionAlgorithm};

type Addr = Arc<Mutex<Option<(UserId, SocketAddr)>>>;

struct SenderState {
    transmit_volume: AtomicF32,
    volume_modifier: AtomicF32,

    is_talking: AtomicBool,
    noise_reduction: AtomicNoiseReductionAlgorithm,
}

impl SenderState {
    fn new() -> Self {
        Self {
            is_talking: AtomicBool::new(false),
            transmit_volume: AtomicF32::new(0.010),
            volume_modifier: AtomicF32::new(1.0),
            noise_reduction: AtomicNoiseReductionAlgorithm::new(NoiseReductionAlgorithm::RNNoise),
        }
    }
}

fn spawn_sender(addr: Addr, socket: Arc<UdpSocket>, state: Arc<SenderState>, capture: Capture) {
    let mut seq = 0;

    let mut buf = BytesMut::new();

    let mut last_send = Instant::now();
    let mut transmitting = false;

    let last_silence = RefCell::new(Some(Instant::now()));

    loop {
        let transmit_volume = state.transmit_volume.load(Ordering::Relaxed);
        let volume_modifier = state.volume_modifier.load(Ordering::Relaxed);
        let noise_reduction = state.noise_reduction.load(Ordering::Relaxed);

        let encoded_recv = recv.recv_encoded_with(|mut samples| {
            if samples.is_empty() {
                state.is_talking.store(false, Ordering::Relaxed);

                return None;
            }

            samples
                .iter_mut()
                .for_each(|sample| *sample *= volume_modifier);

            let max_volume = *(samples.iter().max_by(|a, b| a.total_cmp(b)).unwrap()); // Safe due to the check above

            if max_volume < transmit_volume {
                let now = Instant::now();

                let silence = { *last_silence.borrow() };
                match silence {
                    Some(value) => {
                        if now - value > Duration::from_millis(400) {
                            state.is_talking.store(false, Ordering::Relaxed);

                            return None;
                        }
                    }
                    None => *last_silence.borrow_mut() = Some(now),
                }
            } else {
                state.is_talking.store(true, Ordering::Relaxed);

                *last_silence.borrow_mut() = None;
            }

            Some(samples)
        });

        if encoded_recv.is_none() {
            // Let the receivers know that we finished with our speech chunk
            if transmitting && let Some((user_id, addr)) = *addr.lock().unwrap() {
                buf.clear();

                let mut packet = EncodedAudioPacket::marker();
                packet.seq = seq;

                let udp_packet = UDPPacket {
                    user_id: user_id.value,
                    payload: UDPPacketType::Voice(packet),
                };

                udp_packet.to_bytes(&mut buf);

                seq += 1;
                last_send = Instant::now();

                _ = socket.send_to(&buf, addr);
            }

            // Yes, recv packets also prolong UDP NAT mapping but
            // it's kinda pain in the butt to account for them.
            // I think this solution is more than enough
            if last_send.elapsed() > Duration::from_secs(10)
                && let Some((user_id, addr)) = *addr.lock().unwrap()
            {
                buf.clear();

                let udp_packet = UDPPacket {
                    user_id: user_id.value,
                    payload: UDPPacketType::Ping,
                };

                udp_packet.to_bytes(&mut buf);

                last_send = Instant::now();
                _ = socket.send_to(&buf, addr);
            }

            transmitting = false;

            continue;
        }

        if let Some(mut encoded_recv) = encoded_recv {
            transmitting = true;

            while let Some(mut encoded_packet) = encoded_recv.pop() {
                if let Some((user_id, addr)) = *addr.lock().unwrap() {
                    buf.clear();

                    encoded_packet.seq = seq;
                    let udp_packet = UDPPacket {
                        user_id: user_id.value,
                        payload: UDPPacketType::Voice(encoded_packet),
                    };

                    udp_packet.to_bytes(&mut buf);

                    seq += 1;
                    last_send = Instant::now();

                    _ = socket.send_to(&buf, addr);
                }
            }
        }
    }
}

fn spawn_receiver(socket: Arc<UdpSocket>, mut packet_input: PlaybackPacketInput) {
    let mut buf = BytesMut::with_capacity(4800 * 2);

    loop {
        buf.clear();
        buf.resize(4800 * 2, 0);

        if let Ok(len) = socket.recv(&mut buf[..]) {
            buf.truncate(len);

            let mut buf: Bytes = buf.split().into();
            let packet = UDPPacket::parse(&mut buf);

            let user_id = packet.user_id;
            match packet.payload {
                UDPPacketType::Voice(packet) => {
                    packet_input.send(user_id, Instant::now(), packet);
                }
                _ => todo!(),
            }
        }
    }
}

struct GlobalStreaming {
    capture: CaptureController,
    playback: PlaybackController,

    packet_tx: channel::Sender<PlaybackPacketCommand>,
    packet_output_state: PlaybackOutputState,

    device_registry: DeviceRegistry,

    stream_addr: Addr,

    sender_state: Arc<SenderState>,
}

impl Global for GlobalStreaming {}

pub struct Streaming {}

impl Streaming {
    pub fn is_talking<C: AppContext>(cx: &C) -> bool {
        cx.read_global(|stream: &GlobalStreaming, _| {
            stream.sender_state.is_talking.load(Ordering::Relaxed)
        })
    }

    pub fn set_noise_reduction<C: AppContext>(noise_reduction: NoiseReductionAlgorithm, cx: &C) {
        cx.read_global(move |stream: &GlobalStreaming, _| {
            stream
                .sender_state
                .noise_reduction
                .store(noise_reduction, Ordering::Relaxed);
        });
    }

    pub fn set_input_volume_modifier<C: AppContext>(cx: &C, value: f32) {
        cx.read_global(|stream: &GlobalStreaming, _| {
            stream
                .sender_state
                .volume_modifier
                .store(value, Ordering::Relaxed);
        })
    }

    pub fn set_output_volume_modifier<C: AppContext>(cx: &C, value: f32) {
        cx.read_global(|stream: &GlobalStreaming, _| {
            stream
                .packet_output_state
                .volume
                .store(value, Ordering::Relaxed);
        })
    }

    pub fn get_playback<C: AppContext>(cx: &C) -> PlaybackController {
        cx.read_global(|stream: &GlobalStreaming, _| stream.playback.clone())
    }

    pub fn get_device_registry<C: AppContext>(cx: &mut C) -> DeviceRegistry {
        cx.read_global(|stream: &GlobalStreaming, _| stream.device_registry.clone())
    }

    pub fn get_capture<C: AppContext>(cx: &C) -> CaptureController {
        cx.read_global(|stream: &GlobalStreaming, _| stream.capture.clone())
    }

    pub fn connect<C: AppContext>(cx: &C, user_id: UserId, addr: SocketAddr) {
        cx.read_global(|stream: &GlobalStreaming, _| {
            let mut state = stream.stream_addr.lock().unwrap();

            *state = Some((user_id, addr));
        });
    }

    pub fn add_voice_member<C: AppContext>(cx: &C, shared: Weak<AudioStreamingClientSharedState>) {
        cx.read_global(|stream: &GlobalStreaming, _| {
            let shared = shared.upgrade().unwrap();

            _ = stream.packet_tx.send(PlaybackPacketCommand::AddClient((
                shared.user_id,
                Arc::downgrade(&shared),
            )));
        });
    }
}

pub fn init(cx: &mut App, debug: bool) {
    let stream_addr: Addr = Arc::new(Mutex::new(None));

    let socket = Arc::new(UdpSocket::bind("0.0.0.0:0").unwrap());
    let (capture, mut playback, device_registry) = audio::init(debug);

    let sender_state = Arc::new(SenderState::new());

    let packet_input = playback.packet_input.take().unwrap();

    let packet_tx = packet_input.command_sender.clone();
    let packet_output_state = packet_input.output_state.clone();

    let capture_controller = capture.get_controller();

    thread::Builder::new()
        .name("udp-sender".into())
        .spawn({
            let addr = stream_addr.clone();
            let socket = socket.clone();
            let state = sender_state.clone();

            move || {
                spawn_sender(addr, socket, state, capture);
            }
        })
        .unwrap();

    thread::Builder::new()
        .name("udp-receiver".into())
        .spawn({
            let socket = socket.clone();

            move || {
                spawn_receiver(socket, packet_input);
            }
        })
        .unwrap();

    cx.set_global(GlobalStreaming {
        capture: capture_controller,
        playback: playback.controller,
        packet_tx,
        packet_output_state,
        sender_state,
        stream_addr,
        device_registry,
    });
}
