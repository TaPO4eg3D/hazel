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
        capture::{AudioCapture, AudioCaptureController},
        noise::RNNoiseState,
        playback::{
            AudioPlaybackController, AudioPlaybackPacketInput, AudioStreamingClientSharedState,
            PlaybackOutputState, PlaybackPacketCommand,
        },
    },
    video::{
        self,
        linux::screengrab::{ActiveScreencast, ScreencastPreview},
        playback::{DecodingWorkerCommand, VideoPlaybackController},
    },
};
use crossbeam::channel;
use gpui::{App, AppContext, AsyncApp, DMABuffer, Global};

use reed_solomon_simd::ReedSolomonEncoder;
use ringbuf::traits::Consumer as _;
use rpc::models::{markers::UserId, voice_channels::VideoSessionParams};
use streaming_common::{
    BorrowedEncodedVideoFrameChunk, EncodedAudioPacket, Ping, StreamPacketHeader, UDPPacket,
    UDPPayloadType, to_udp_packet_bytes,
};

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

type SharedStartedScreencast = Arc<Mutex<Option<ActiveScreencast>>>;

struct ScreenStreamingData {
    seq: u64,
    screencast: SharedStartedScreencast,
}

impl ScreenStreamingData {
    fn new(screencast: SharedStartedScreencast) -> Self {
        Self { seq: 0, screencast }
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
        screen_state: ScreenStreamingData,
        notifier: CaptureNotifier,
    ) -> Self {
        Self {
            buf: BytesMut::new(),
            last_send: Instant::now(),

            notifier,

            audio: audio_state,
            screen: screen_state,

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

            to_udp_packet_bytes(&mut self.buf, user_id.value, &packet);

            self.audio.seq += 1;
            self.last_send = Instant::now();

            _ = self.socket.send_to(&self.buf, addr);
        }
    }

    /// Just a ping message to keep NAT mapping opened
    fn send_ping(&mut self) {
        if let Some((user_id, addr)) = *self.addr.lock().unwrap() {
            self.buf.clear();

            to_udp_packet_bytes(&mut self.buf, user_id.value, &Ping);

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

    fn try_send_frame(
        &mut self,
        user_id: UserId,
        addr: SocketAddr,
        shard_len: usize,
        packet_loss: f32,
    ) -> bool {
        let mut screencast = self.screen.screencast.lock().unwrap();
        let Some(screencast) = screencast.as_mut() else {
            return false;
        };

        let Some(mut ready_frame) = screencast.frame_pool.get_ready_frame() else {
            return false;
        };

        let data_shards_len = ((ready_frame.len() as f32) / shard_len as f32).ceil() as usize;
        let rec_shards_len =
            ((packet_loss / (1. - packet_loss)) * data_shards_len as f32).ceil() as usize;

        let data_size = ready_frame.len();
        let padding = (data_shards_len * shard_len) - ready_frame.len();
        for _ in 0..padding {
            ready_frame.push(0);
        }

        // TODO: Fork this library? It allocates memory on every invocation,
        // this kinda defeats the purpose of the dedicated frame pool
        let mut encoder = ReedSolomonEncoder::new(data_shards_len, rec_shards_len, shard_len)
            .expect("Failed to initialize Reed Solomon Encoder");

        ready_frame.chunks_exact(shard_len).for_each(|chunk| {
            encoder
                .add_original_shard(chunk)
                .expect("All preparations done above, should not fail");
        });

        let encoded = encoder.encode().expect("Should not fail");

        let frame_chunks = ready_frame
            .chunks_exact(shard_len)
            .chain(encoded.recovery_iter())
            .enumerate()
            .map(|(i, chunk)| BorrowedEncodedVideoFrameChunk {
                header: StreamPacketHeader {
                    seq: self.screen.seq,
                    shard: i as u16,
                    data_shards: data_shards_len as u16,
                    shard_size: shard_len as u16,
                    recovery_shards: rec_shards_len as u16,
                    data_size: data_size as u64,
                },
                data: chunk,
            });

        for frame_chunk in frame_chunks {
            self.buf.clear();
            to_udp_packet_bytes(&mut self.buf, user_id.value, &frame_chunk);

            // TODO: Limit throughput, we don't want to spam packets way too fast?
            _ = self.socket.send_to(&self.buf, addr);
            self.last_send = Instant::now();
        }

        screencast.frame_pool.push_emtpy_frame(ready_frame);

        true
    }

    fn process_video_frame(&mut self) {
        let Some((user_id, addr)) = *self.addr.lock().unwrap() else {
            return;
        };

        // TODO: Should be comming from the server (VideoSessionParams)
        if self.try_send_frame(user_id, addr, 1280, 0.2) {
            self.screen.seq += 1;
        }
    }

    fn run(mut self) {
        loop {
            let result = self.notifier.wait(Duration::from_millis(80));

            if let WaitResult::Ready(state) = result {
                if state.is_audio_ready {
                    self.process_audio_samples();
                }

                if state.is_screen_ready {
                    self.process_video_frame();
                }
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
                    to_udp_packet_bytes(&mut self.buf, user_id.value, &packet);

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

fn spawn_receiver(
    socket: Arc<UdpSocket>,
    mut video_controller: VideoPlaybackController,
    mut audio_packet_input: AudioPlaybackPacketInput,
) {
    const BUF_SIZE: usize = 4096;
    let mut buf = BytesMut::with_capacity(BUF_SIZE);

    loop {
        buf.resize(BUF_SIZE, 0);

        if let Ok(len) = socket.recv(&mut buf[..]) {
            buf.truncate(len);

            let mut buf: Bytes = buf.split().into();

            if let Ok(packet) = UDPPacket::parse(&mut buf) {
                let user_id = packet.user_id;

                match packet.payload {
                    UDPPayloadType::Audio(audio_bytes) => {
                        let mut audio_packet = EncodedAudioPacket::default();

                        if audio_bytes.parse(&mut audio_packet).is_ok() {
                            audio_packet_input.send(user_id, Instant::now(), audio_packet);
                        }
                    }
                    UDPPayloadType::Video(chunk_bytes) => {
                        video_controller.process_frame(UserId::new(user_id), chunk_bytes);
                    }
                    _ => todo!(),
                }
            }
        }
    }
}

struct GlobalStreaming {
    stream_addr: UDPAddr,

    capture_notifier: CaptureNotifier,

    active_screencast: SharedStartedScreencast,

    audio_capture: AudioCaptureController,
    audio_playback: AudioPlaybackController,

    video_command_tx: channel::Sender<DecodingWorkerCommand>,

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

    pub fn get_playback<C: AppContext>(cx: &C) -> AudioPlaybackController {
        cx.read_global(|stream: &GlobalStreaming, _| stream.audio_playback.clone())
    }

    pub fn get_device_registry<C: AppContext>(cx: &mut C) -> DeviceRegistry {
        cx.read_global(|stream: &GlobalStreaming, _| stream.audio_device_registry.clone())
    }

    pub fn get_capture<C: AppContext>(cx: &C) -> AudioCaptureController {
        cx.read_global(|stream: &GlobalStreaming, _| stream.audio_capture.clone())
    }

    pub fn connect<C: AppContext>(cx: &C, user_id: UserId, addr: SocketAddr) {
        cx.read_global(|stream: &GlobalStreaming, _| {
            let mut state = stream.stream_addr.lock().unwrap();
            *state = Some((user_id, addr));

            // TODO: Extremelly bad solution, fix ASAP.
            // We have to properly ensure we're established a UDP connection
            // + reflect the status of connection in UI
            stream.capture_notifier.notify_ping();
        });
    }

    pub async fn start_screencast(cx: &mut AsyncApp) -> Option<ScreencastPreview> {
        let notifier =
            cx.read_global(|stream: &GlobalStreaming, _cx| stream.capture_notifier.clone());

        let (cast, preview) = capture::video::linux::screengrab::init_screencast(notifier)
            .await
            .ok()?;

        cx.update_global(move |stream: &mut GlobalStreaming, _cx| {
            let mut active_screencast = stream.active_screencast.lock().unwrap();
            *active_screencast = Some(cast);
        });

        Some(preview)
    }

    pub async fn stop_screencast(cx: &mut AsyncApp) {
        cx.update_global(move |stream: &mut GlobalStreaming, _cx| {
            if let Some(cast) = stream.active_screencast.lock().unwrap().take() {
                cast.close();
            }
        });
    }

    pub fn register_video_stream(
        cx: &mut AsyncApp,
        user_id: UserId,
        frame_tx: smol::channel::Sender<DMABuffer>,
        params: VideoSessionParams,
    ) {
        cx.update_global(move |stream: &mut GlobalStreaming, _cx| {
            _ = stream
                .video_command_tx
                .send(DecodingWorkerCommand::AddClient((
                    user_id, frame_tx, params,
                )));
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

    let active_screencast = Arc::new(Mutex::new(None));

    let video_playback = video::playback::init();
    let video_command_tx = video_playback.get_command_tx();

    thread::Builder::new()
        .name("udp-sender".into())
        .spawn({
            let addr = stream_addr.clone();
            let socket = socket.clone();
            let shared = audio_shared_state.clone();
            let active_screencast = active_screencast.clone();
            let capture_notifier = capture_notifier.clone();

            move || {
                let sender = PacketSender::new(
                    addr,
                    socket,
                    AudioStreamingState::new(shared, audio_capture),
                    ScreenStreamingData::new(active_screencast),
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
                spawn_receiver(socket, video_playback, audio_packet_input);
            }
        })
        .unwrap();

    cx.set_global(GlobalStreaming {
        active_screencast,
        capture_notifier,
        audio_capture: audio_capture_controller,
        audio_playback: audio_playback.controller,
        audio_packet_command_tx: audio_packet_tx,
        audio_playback_output_state: audio_packet_output_state,
        video_command_tx,
        audio_shared_state,
        stream_addr,
        audio_device_registry,
    });
}
