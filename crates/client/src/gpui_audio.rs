use std::{
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
use capture::{
    CaptureNotifier, WaitResult,
    audio::{
        self, DEFAULT_BIT_RATE, DeviceRegistry,
        capture::{AudioCapture, CaptureController},
        noise::RNNoiseState,
        playback::{
            AudioStreamingClientSharedState, PlaybackController, PlaybackOutputState,
            PlaybackPacketCommand, PlaybackPacketInput,
        },
    },
};
use crossbeam::channel;
use gpui::{App, AppContext, Global};

use ringbuf::traits::Consumer as _;
use rpc::models::markers::UserId;
use streaming_common::{EncodedAudioPacket, UDPPacket, UDPPacketType};

use crate::components::streaming_state::{AtomicNoiseReductionAlgorithm, NoiseReductionAlgorithm};

type UDPAddr = Arc<Mutex<Option<(UserId, SocketAddr)>>>;

/// Shared state beteween UI and Packet Sender
struct AudioStreamingSharedState {
    transmit_volume: AtomicF32,
    volume_modifier: AtomicF32,

    is_talking: AtomicBool,
    noise_reduction: AtomicNoiseReductionAlgorithm,
}

impl AudioStreamingSharedState {
    fn new() -> Self {
        Self {
            is_talking: AtomicBool::new(false),
            transmit_volume: AtomicF32::new(0.010),
            volume_modifier: AtomicF32::new(1.0),
            noise_reduction: AtomicNoiseReductionAlgorithm::new(NoiseReductionAlgorithm::RNNoise),
        }
    }
}

struct ScreenStreamingData {}

impl ScreenStreamingData {
    fn new() -> Self {
        Self {}
    }
}

struct AudioStreamingState {
    seq: u64,

    transmitting: bool,
    last_vad: Instant,

    denoiser_state: DenoiserState,
    capture: AudioCapture,

    shared: Arc<AudioStreamingSharedState>,
}

impl AudioStreamingState {
    fn new(shared: Arc<AudioStreamingSharedState>, capture: AudioCapture) -> Self {
        Self {
            seq: 0,

            transmitting: false,
            last_vad: Instant::now(),

            capture,

            shared,
            denoiser_state: DenoiserState::Disabled,
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
    buf: BytesMut,
    notifier: CaptureNotifier,

    audio: AudioStreamingState,
    screen: ScreenStreamingData,

    /// Last time we've send a packet (of any kind)
    last_send: Instant,

    addr: UDPAddr,
    socket: Arc<UdpSocket>,
}

impl PacketSender {
    fn new(
        addr: UDPAddr,
        socket: Arc<UdpSocket>,
        audio_state: AudioStreamingState,
        notifier: CaptureNotifier,
    ) -> Self {
        Self {
            buf: BytesMut::new(),
            last_send: Instant::now(),

            notifier,

            audio: audio_state,
            screen: ScreenStreamingData::new(),

            addr,
            socket,
        }
    }

    /// Send a special packet that marks the end of the speech section.
    /// It prevents the growth of the jitter buffer on the recv side
    fn send_audio_marker(&mut self) {
        if let Some((user_id, addr)) = *self.addr.lock().unwrap() {
            self.buf.clear();

            let mut packet = EncodedAudioPacket::marker();
            packet.seq = self.audio.seq;

            let udp_packet = UDPPacket {
                user_id: user_id.value,
                payload: UDPPacketType::Voice(packet),
            };

            udp_packet.to_bytes(&mut self.buf);

            self.audio.seq += 1;
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
        let volume_modifier = self.audio.shared.volume_modifier.load(Ordering::Relaxed);
        input.iter_mut().for_each(|s| *s *= volume_modifier);
    }

    fn apply_denoiser(&mut self, input: &mut [f32]) -> usize {
        let denoise = self.audio.shared.noise_reduction.load(Ordering::Relaxed);

        match denoise {
            NoiseReductionAlgorithm::Disabled => {
                self.audio.denoiser_state = DenoiserState::Disabled;
            }
            NoiseReductionAlgorithm::RNNoise | NoiseReductionAlgorithm::DeepFilterNet => {
                if !matches!(self.audio.denoiser_state, DenoiserState::RNNoise(_)) {
                    self.audio.denoiser_state = DenoiserState::RNNoise(RNNoiseState::new());
                }
            }
        }

        self.audio.denoiser_state.apply_denoiser(input)
    }

    fn is_voice_activity_detected(&self, input: &[f32]) -> bool {
        let transmit_volume = self.audio.shared.transmit_volume.load(Ordering::Relaxed);
        let max_volume = *(input
            .iter()
            .max_by(|a, b| a.total_cmp(b))
            .expect("Input buffer should not be empty"));

        max_volume >= transmit_volume
    }

    fn is_silence(&self) -> bool {
        // To not cut the sound off too sharply
        self.audio.last_vad.elapsed() > Duration::from_millis(400)
    }

    fn process_audio_samples(&mut self) {
        let mut input_buffer = [0_f32; DEFAULT_BIT_RATE];

        let mut count = self
            .audio
            .capture
            .samples_buffer
            .pop_slice(&mut input_buffer);

        if count > 0 {
            count = self.apply_denoiser(&mut input_buffer[..count]);

            // Denoiser is not ready yet
            if count == 0 {
                return;
            }

            self.increase_volume(&mut input_buffer[..count]);
            if self.is_voice_activity_detected(&input_buffer[..count]) {
                self.audio.last_vad = Instant::now();
            }

            if !self.is_silence() {
                self.audio.transmitting = true;
                self.audio.capture.encoder.encode(&input_buffer[..count]);
            } else if self.audio.transmitting {
                self.audio.transmitting = false;
                self.audio.capture.encoder.reset();

                self.send_audio_marker();
            }
        }
    }

    fn process_screen_frame(&mut self) {}

    fn run(mut self) {
        loop {
            let result = self.notifier.wait(Duration::from_millis(80));

            if let WaitResult::Ready(state) = result {
                if state.is_audio_ready {
                    self.process_audio_samples();
                }

                if state.is_screen_ready {}
            }

            let is_enabled = self.audio.capture.is_enabled.load(Ordering::Relaxed);
            if self.audio.transmitting && (matches!(result, WaitResult::Timeout) || !is_enabled) {
                self.audio.transmitting = false;
                self.audio.capture.encoder.reset();

                self.send_audio_marker();
            }

            while let Some(mut packet) = self.audio.capture.encoder.pop_packet() {
                if self.audio.transmitting
                    && let Some((user_id, addr)) = *self.addr.lock().unwrap()
                {
                    self.buf.clear();

                    packet.seq = self.audio.seq;
                    let udp_packet = UDPPacket {
                        user_id: user_id.value,
                        payload: UDPPacketType::Voice(packet),
                    };

                    udp_packet.to_bytes(&mut self.buf);

                    self.audio.seq += 1;
                    self.last_send = Instant::now();

                    _ = self.socket.send_to(&self.buf, addr);
                }
            }

            self.audio
                .shared
                .is_talking
                .store(self.audio.transmitting, Ordering::Relaxed);

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
    stream_addr: UDPAddr,

    audio_capture: CaptureController,
    audio_playback: PlaybackController,

    audio_packet_command_tx: channel::Sender<PlaybackPacketCommand>,
    audio_playback_output_state: PlaybackOutputState,

    /// Registry to query devices for audio I/O
    audio_device_registry: DeviceRegistry,
    /// Shared state beteween UI and Packet Sender
    audio_shared_state: Arc<AudioStreamingSharedState>,
}

impl Global for GlobalStreaming {}

pub struct Streaming {}

impl Streaming {
    pub fn is_talking<C: AppContext>(cx: &C) -> bool {
        cx.read_global(|stream: &GlobalStreaming, _| {
            stream.audio_shared_state.is_talking.load(Ordering::Relaxed)
        })
    }

    pub fn set_noise_reduction<C: AppContext>(noise_reduction: NoiseReductionAlgorithm, cx: &C) {
        cx.read_global(move |stream: &GlobalStreaming, _| {
            stream
                .audio_shared_state
                .noise_reduction
                .store(noise_reduction, Ordering::Relaxed);
        });
    }

    pub fn set_input_volume_modifier<C: AppContext>(cx: &C, value: f32) {
        cx.read_global(|stream: &GlobalStreaming, _| {
            stream
                .audio_shared_state
                .volume_modifier
                .store(value, Ordering::Relaxed);
        })
    }

    pub fn set_output_volume_modifier<C: AppContext>(cx: &C, value: f32) {
        cx.read_global(|stream: &GlobalStreaming, _| {
            stream
                .audio_playback_output_state
                .volume
                .store(value, Ordering::Relaxed);
        })
    }

    pub fn get_playback<C: AppContext>(cx: &C) -> PlaybackController {
        cx.read_global(|stream: &GlobalStreaming, _| stream.audio_playback.clone())
    }

    pub fn get_device_registry<C: AppContext>(cx: &mut C) -> DeviceRegistry {
        cx.read_global(|stream: &GlobalStreaming, _| stream.audio_device_registry.clone())
    }

    pub fn get_capture<C: AppContext>(cx: &C) -> CaptureController {
        cx.read_global(|stream: &GlobalStreaming, _| stream.audio_capture.clone())
    }

    pub fn connect<C: AppContext>(cx: &C, user_id: UserId, addr: SocketAddr) {
        cx.read_global(|stream: &GlobalStreaming, _| {
            let mut state = stream.stream_addr.lock().unwrap();

            *state = Some((user_id, addr));
        });
    }

    pub fn disconnect<C: AppContext>(cx: &C) {
        cx.read_global(|stream: &GlobalStreaming, _| {
            let mut state = stream.stream_addr.lock().unwrap();

            *state = None;
        });
    }

    pub fn add_voice_member<C: AppContext>(cx: &C, shared: Weak<AudioStreamingClientSharedState>) {
        cx.read_global(|stream: &GlobalStreaming, _| {
            let shared = shared.upgrade().unwrap();

            _ = stream
                .audio_packet_command_tx
                .send(PlaybackPacketCommand::AddClient((
                    shared.user_id,
                    Arc::downgrade(&shared),
                )));
        });
    }
}

pub fn init(cx: &mut App, debug: bool) {
    let stream_addr: UDPAddr = Arc::new(Mutex::new(None));

    let capture_notifier = CaptureNotifier::new();

    let socket = Arc::new(UdpSocket::bind("0.0.0.0:0").unwrap());
    let (audio_capture, mut audio_playback, audio_device_registry) =
        audio::init(debug, capture_notifier.clone());

    let audio_shared_state = Arc::new(AudioStreamingSharedState::new());

    let audio_packet_input = audio_playback.packet_input.take().unwrap();

    let audio_packet_tx = audio_packet_input.command_sender.clone();
    let audio_packet_output_state = audio_packet_input.output_state.clone();

    let audio_capture_controller = audio_capture.get_controller();

    thread::Builder::new()
        .name("udp-sender".into())
        .spawn({
            let addr = stream_addr.clone();
            let socket = socket.clone();
            let shared = audio_shared_state.clone();

            move || {
                let sender = PacketSender::new(
                    addr,
                    socket,
                    AudioStreamingState::new(shared, audio_capture),
                    capture_notifier,
                );

                sender.run();
            }
        })
        .unwrap();

    thread::Builder::new()
        .name("udp-receiver".into())
        .spawn({
            let socket = socket.clone();

            move || {
                spawn_receiver(socket, audio_packet_input);
            }
        })
        .unwrap();

    cx.set_global(GlobalStreaming {
        audio_capture: audio_capture_controller,
        audio_playback: audio_playback.controller,
        audio_packet_command_tx: audio_packet_tx,
        audio_playback_output_state: audio_packet_output_state,
        audio_shared_state,
        stream_addr,
        audio_device_registry,
    });
}
