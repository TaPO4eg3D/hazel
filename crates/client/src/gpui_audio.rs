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
    self, DEFAULT_BIT_RATE, DEFAULT_CHANNELS, DeviceRegistry,
    capture::{Capture, CaptureController, WaitResult},
    noise::RNNoiseState,
    playback::{
        AudioStreamingClientSharedState, PlaybackController, PlaybackOutputState,
        PlaybackPacketCommand, PlaybackPacketInput,
    },
};
use crossbeam::channel;
use gpui::{App, AppContext, Global};

use ringbuf::traits::Consumer as _;
use rpc::models::markers::UserId;
use streaming_common::{EncodedAudioPacket, UDPPacket, UDPPacketType};

use crate::components::streaming_state::{AtomicNoiseReductionAlgorithm, NoiseReductionAlgorithm};

type Addr = Arc<Mutex<Option<(UserId, SocketAddr)>>>;

struct SenderSharedState {
    transmit_volume: AtomicF32,
    volume_modifier: AtomicF32,

    is_talking: AtomicBool,
    noise_reduction: AtomicNoiseReductionAlgorithm,
}

impl SenderSharedState {
    fn new() -> Self {
        Self {
            is_talking: AtomicBool::new(false),
            transmit_volume: AtomicF32::new(0.010),
            volume_modifier: AtomicF32::new(1.0),
            noise_reduction: AtomicNoiseReductionAlgorithm::new(NoiseReductionAlgorithm::RNNoise),
        }
    }
}

enum DenoiserState {
    Disabled,
    RNNoise(RNNoiseState),
}

impl DenoiserState {
    fn apply_denoiser(&mut self, input: &mut [f32]) -> usize {
        match self {
            DenoiserState::Disabled => input.len(),
            DenoiserState::RNNoise(state) => {
                state.process(input);

                let mut count = 0;
                for sample in input.iter_mut() {
                    if let Some(value) = state.output_queue.pop_front() {
                        count += 1;

                        *sample = value;
                    } else {
                        return count;
                    }
                }

                count
            }
        }
    }
}

struct PacketSender {
    seq: u64,
    buf: BytesMut,

    transmitting: bool,

    last_send: Instant,
    last_vad: Instant,

    shared_state: Arc<SenderSharedState>,
    capture: Capture,

    addr: Addr,
    socket: Arc<UdpSocket>,

    denoiser_state: DenoiserState,
}

impl PacketSender {
    fn new(
        addr: Addr,
        socket: Arc<UdpSocket>,
        state: Arc<SenderSharedState>,
        capture: Capture,
    ) -> Self {
        Self {
            seq: 0,
            buf: BytesMut::new(),

            transmitting: false,

            last_send: Instant::now(),
            last_vad: Instant::now(),

            addr,
            shared_state: state,
            socket,
            capture,

            denoiser_state: DenoiserState::Disabled,
        }
    }

    /// Send a special packet that marks the end of the speech section.
    /// It prevents the growth of the jitter buffer on the recv side
    fn send_marker(&mut self) {
        if let Some((user_id, addr)) = *self.addr.lock().unwrap() {
            self.buf.clear();

            let mut packet = EncodedAudioPacket::marker();
            packet.seq = self.seq;

            let udp_packet = UDPPacket {
                user_id: user_id.value,
                payload: UDPPacketType::Voice(packet),
            };

            udp_packet.to_bytes(&mut self.buf);

            self.seq += 1;
            self.last_send = Instant::now();

            _ = self.socket.send_to(&self.buf, addr);
        }
    }

    /// Just a ping message to keep NAT mapping opened
    fn send_ping(&mut self) {
        if let Some((user_id, addr)) = *self.addr.lock().unwrap() {
            self.buf.clear();

            let udp_packet = UDPPacket {
                user_id: user_id.value,
                payload: UDPPacketType::Ping,
            };

            udp_packet.to_bytes(&mut self.buf);

            self.last_send = Instant::now();
            _ = self.socket.send_to(&self.buf, addr);
        }
    }

    fn increase_volume(&self, input: &mut [f32]) {
        let volume_modifier = self.shared_state.volume_modifier.load(Ordering::Relaxed);
        input.iter_mut()
            .for_each(|s| *s *= volume_modifier);
    }

    fn apply_denoiser(&mut self, input: &mut [f32]) -> usize {
        let denoise = self.shared_state.noise_reduction.load(Ordering::Relaxed);

        match denoise {
            NoiseReductionAlgorithm::Disabled => {
                self.denoiser_state = DenoiserState::Disabled;
            }
            NoiseReductionAlgorithm::RNNoise | NoiseReductionAlgorithm::DeepFilterNet => {
                if !matches!(self.denoiser_state, DenoiserState::RNNoise(_)) {
                    self.denoiser_state = DenoiserState::RNNoise(RNNoiseState::new());
                }
            }
        }

        self.denoiser_state.apply_denoiser(input)
    }

    fn is_voice_activity_detected(&self, input: &[f32]) -> bool {
        let transmit_volume = self.shared_state.transmit_volume.load(Ordering::Relaxed);
        let max_volume = *(input
            .iter()
            .max_by(|a, b| a.total_cmp(b))
            .expect("Input buffer should not be empty"));

        max_volume >= transmit_volume
    }

    fn is_silence(&self) -> bool {
        // To not cut the sound off too sharply
        self.last_vad.elapsed() > Duration::from_millis(400)
    }

    fn process_samples(&mut self) {
        let mut input_buffer = [0_f32; DEFAULT_BIT_RATE];

        let mut count = self.capture.samples_buffer.pop_slice(&mut input_buffer);
        if count > 0 {
            count = self.apply_denoiser(&mut input_buffer[..count]);

            // Denoiser is not ready yet
            if count == 0 {
                return;
            }

            self.increase_volume(&mut input_buffer[..count]);
            if self.is_voice_activity_detected(&input_buffer[..count]) {
                self.last_vad = Instant::now();
            }

            if !self.is_silence() {
                self.transmitting = true;
                self.capture.encoder.encode(&input_buffer[..count]);
            } else if self.transmitting {
                self.transmitting = false;
                self.capture.encoder.reset();

                self.send_marker();
            }
        }
    }

    fn run(mut self) {
        loop {
            let result = self.capture.wait(Duration::from_millis(80));
            let is_enabled = self.capture.is_enabled.load(Ordering::Relaxed);

            if matches!(result, WaitResult::Ready) {
                self.process_samples();
            }

            if self.transmitting
                && (matches!(result, WaitResult::Timeout) || !is_enabled)
            {
                self.transmitting = false;
                self.capture.encoder.reset();

                self.send_marker();
            }

            while let Some(mut packet) = self.capture.encoder.pop_packet() {
                if self.transmitting && let Some((user_id, addr)) = *self.addr.lock().unwrap() {
                    self.buf.clear();

                    packet.seq = self.seq;
                    let udp_packet = UDPPacket {
                        user_id: user_id.value,
                        payload: UDPPacketType::Voice(packet),
                    };

                    udp_packet.to_bytes(&mut self.buf);

                    self.seq += 1;
                    self.last_send = Instant::now();

                    _ = self.socket.send_to(&self.buf, addr);
                }
            }

            self.shared_state.is_talking.store(self.transmitting, Ordering::Relaxed);

            if self.last_send.elapsed() > Duration::from_secs(10) {
                self.send_ping();
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

    sender_state: Arc<SenderSharedState>,
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

    let sender_state = Arc::new(SenderSharedState::new());

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
                let sender = PacketSender::new(addr, socket, state, capture);

                sender.run();
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
