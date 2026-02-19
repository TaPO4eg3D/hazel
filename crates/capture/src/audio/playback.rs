use std::{
    collections::{BTreeMap, HashMap}, sync::{
        Arc, Mutex, Weak, atomic::{AtomicBool, Ordering}
    }, time::Instant
};

use atomic_float::AtomicF32;
use crossbeam::channel;
use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer as _, Producer as _, Split as _},
};
use streaming_common::EncodedAudioPacket;

use crate::audio::{
    AudioLoopCommand, DEFAULT_CHANNELS, DEFAULT_RATE, PlatformLoopController,
    decode::AudioDecoder,
};

const SAMPLES_BUFFER: usize = (DEFAULT_RATE * DEFAULT_CHANNELS) as usize;

#[derive(Default, Clone, Debug)]
pub struct JitterBufferStats {
    pub missed_packets: u32,

    pub target_delay: f64,
    pub estimated_delay: f64,
}

struct JitterBuffer {
    decoder: AudioDecoder,

    packets_buffer: BTreeMap<u64, (Instant, EncodedAudioPacket)>,
    samples_buffer: heapless::Deque<f32, SAMPLES_BUFFER>,

    // SEQ of the next expected packet
    next_playout_seq: Option<u64>,
    target_delay_ms: f64,

    min_delay_ms: f64,
    max_delay_ms: f64,

    jitter_estimate_ms: f64,

    /// Smoothing factor for jitter estimation (0.0 - 1.0)
    alpha: f64,

    // SEQ of a packet that marks the end of the
    // speech chunk. If there's no packets after this
    // SEQ, we don't need to treat it as jittering
    ending_chunk: Option<u64>,

    last_arrival: Option<Instant>,
    last_ts: Option<u64>,

    // How many times had to generate PLC in a row
    misses: u32,

    debug: bool,
    stats: Arc<Mutex<JitterBufferStats>>,
}

impl JitterBuffer {
    fn new(debug: bool) -> Self {
        Self {
            decoder: AudioDecoder::new(),
            packets_buffer: BTreeMap::new(),
            samples_buffer: heapless::Deque::new(),
            next_playout_seq: None,
            target_delay_ms: 20.,
            min_delay_ms: 20.,
            max_delay_ms: 200.,
            jitter_estimate_ms: 0.0,
            alpha: 0.05,
            last_arrival: None,
            last_ts: None,
            misses: 0,
            ending_chunk: None,
            stats: Arc::new(Mutex::new(JitterBufferStats::default())),
            debug,
        }
    }

    fn update_stats(&self) {
        let mut stats = self.stats.lock().unwrap();

        stats.target_delay = self.target_delay_ms;
        stats.estimated_delay = self.jitter_estimate_ms;
        stats.missed_packets += self.misses;
    }

    fn push_packet(&mut self, arrival_ts: Instant, packet: EncodedAudioPacket) {
        // Packet arrived out of order, we already finished with
        // this speech chunk
        if let Some(seq) = self.ending_chunk
            && self.next_playout_seq.is_none()
            && packet.seq < seq
        {
            return;
        }

        // Special packet, means the end of the speech chunk
        if packet.marker {
            self.ending_chunk = Some(packet.seq);

            return;
        }

        self.update_jitter(arrival_ts, &packet);
        self.adapt_target_delay();

        // Packet arrived out of order, we already played PLC
        if let Some(next) = self.next_playout_seq
            && packet.seq < next
        {
            return;
        }

        const MAX_BUFFER_SIZE: usize = 20;
        if self.packets_buffer.len() >= MAX_BUFFER_SIZE
            && let Some(&oldest_seq) = self.packets_buffer.keys().next()
        {
            self.packets_buffer.remove(&oldest_seq);
        }

        self.packets_buffer.insert(packet.seq, (arrival_ts, packet));
    }

    fn close_speech_chunk(&mut self) {
        self.last_ts = None;
        self.last_arrival = None;
        self.misses = 0;

        self.next_playout_seq = None;

        self.decoder.reset();
    }

    fn update_jitter(&mut self, arrival_ts: Instant, packet: &EncodedAudioPacket) {
        // Opus encodes in chunks of 20ms
        let timestamp = packet.seq * 20;

        if let (Some(last_arrival), Some(last_ts)) = (self.last_arrival, self.last_ts) {
            let arrival_diff_ms = arrival_ts.duration_since(last_arrival).as_secs_f64() * 1000.;

            let ts_diff_ms = timestamp.abs_diff(last_ts) as f64;
            let deviation = (arrival_diff_ms - ts_diff_ms).abs();

            // Exponential moving average
            self.jitter_estimate_ms =
                self.jitter_estimate_ms * (1.0 - self.alpha) + deviation * self.alpha;
        }

        self.last_arrival = Some(arrival_ts);
        self.last_ts = Some(timestamp);
    }

    fn adapt_target_delay(&mut self) {
        let desired = self.jitter_estimate_ms * 2.0;
        let adjustment_rate = 0.1;

        self.target_delay_ms += (desired - self.target_delay_ms) * adjustment_rate;
        self.target_delay_ms = self
            .target_delay_ms
            .clamp(self.min_delay_ms, self.max_delay_ms);
    }

    fn decode(&mut self, out_limit: usize) -> bool {
        if self.next_playout_seq.is_none() {
            if let Some((&pts, (arrival_ts, _))) = self.packets_buffer.iter().next() {
                let buffered_ms = arrival_ts.elapsed().as_secs_f64() * 1000.0;

                if buffered_ms < self.target_delay_ms {
                    return false;
                }

                self.next_playout_seq = Some(pts);
            } else {
                return false;
            }
        }

        let seq = self.next_playout_seq.unwrap();
        if self.ending_chunk.is_some_and(|end_pts| end_pts == seq) {
            self.close_speech_chunk();

            return false;
        }

        if let Some((_, packet)) = self.packets_buffer.remove(&seq) {
            self.misses = 0;
            self.next_playout_seq = Some(seq.wrapping_add(1));

            self.decoder.decode(packet);
        } else {
            // YABAI!! No data to play...

            let seq = seq.wrapping_add(1);
            self.next_playout_seq = Some(seq);

            // We might have the next packet with FEC
            if let Some((_, packet)) = self.packets_buffer.remove(&seq) {
                // println!("Corrected using FEC");

                // We don't need to increment `next_playout_seq`
                // this packet is used only for correction
                self.decoder.decode_fec(packet, out_limit);
            } else {
                // No FEC, trying regular PLC
                self.misses += 1;

                // If have have too much misses, we probably missed the marker
                // and we need to close the speech chunk
                if self.misses > 4 {
                    self.close_speech_chunk();

                    return false;
                }

                // Packet is missing, ask decoder for PLC
                self.decoder.ask_plc(out_limit);
            }
        }

        while let Some(decoded_sample) = self.decoder.decoded_samples.pop_front() {
            if self.samples_buffer.push_back(decoded_sample).is_err() {
                println!("Samples buffer overrun!");
            }
        }

        true
    }

    fn pop_slice(&mut self, output: &mut [f32], f: impl Fn(f32, f32) -> f32) -> bool {
        let mut i = 0;

        while i < output.len() {
            if let Some(sample) = self.samples_buffer.pop_front() {
                output[i] = f(output[i], sample);
                i += 1;

                continue;
            };

            // Return if we failed to decode anything, mixer
            // will take care of filling missing bits with zeroes
            if !self.decode(output.len() - i) {
                break;
            }
        }

        if self.debug {
            self.update_stats();
        }

        i != 0
    }
}

pub struct AudioStreamingClientState {
    pub user_id: i32,

    jitter_buffer: JitterBuffer,

    shared: Weak<AudioStreamingClientSharedState>,
    active: bool,
}

// Shared state with UI to control volume, mute, etc.
pub struct AudioStreamingClientSharedState {
    pub user_id: i32,
    pub is_talking: AtomicBool,
}

impl AudioStreamingClientSharedState {
    pub fn new(user_id: i32) -> Self {
        Self {
            user_id,
            is_talking: AtomicBool::new(false),
        }
    }
}

impl AudioStreamingClientState {
    pub fn new(user_id: i32, shared: Weak<AudioStreamingClientSharedState>, debug: bool) -> Self {
        Self {
            user_id,
            shared,
            jitter_buffer: JitterBuffer::new(debug),
            active: true,
        }
    }
}

pub enum AudioPacketCommand {
    AddClient((i32, Weak<AudioStreamingClientSharedState>)),
    RemoveClient(i32),
}

pub struct AudioPacketInput {
    pub tx: channel::Sender<AudioPacketCommand>,
    pub output_state: AudioOutputState,

    packet_buffer: HeapProd<(i32, Instant, EncodedAudioPacket)>,
}

type DebugStats = Vec<(i32, Weak<Mutex<JitterBufferStats>>)>;
pub(crate) struct AudioPacketOutput {
    active_clients: HashMap<i32, AudioStreamingClientState>,

    rx: channel::Receiver<AudioPacketCommand>,
    packet_buffer: HeapCons<(i32, Instant, EncodedAudioPacket)>,

    output_state: AudioOutputState,

    pub(crate) debug_stats: Option<Arc<Mutex<DebugStats>>>,
}

impl AudioPacketInput {
    pub fn send(&mut self, user_id: i32, arrival_ts: Instant, packet: EncodedAudioPacket) {
        _ = self.packet_buffer.try_push((user_id, arrival_ts, packet));
    }
}

impl AudioPacketOutput {
    fn process_commands(&mut self) {
        while let Ok(command) = self.rx.try_recv() {
            match command {
                AudioPacketCommand::AddClient((user_id, state)) => {
                    let state = AudioStreamingClientState::new(user_id, state, self.debug_stats.is_some());

                    if let Some(debug_stats) = self.debug_stats.as_ref() {
                        let mut debug_stats = debug_stats.lock().unwrap();

                        debug_stats.push((user_id, Arc::downgrade(&state.jitter_buffer.stats)));
                    }

                    self.active_clients
                        .insert(user_id, state);
                }
                AudioPacketCommand::RemoveClient(user_id) => {
                    self.active_clients.remove(&user_id);
                }
            }
        }
    }

    fn process_packets(&mut self) {
        while let Some((user_id, arrival_ts, packet)) = self.packet_buffer.try_pop() {
            let Some(client_state) = self.active_clients.get_mut(&user_id) else {
                // Probably a late packet. We don't have such user anymore, skipping
                continue;
            };

            client_state.jitter_buffer.push_packet(arrival_ts, packet);
        }
    }

    pub(crate) fn process(&mut self, output: &mut [f32]) {
        self.process_commands();
        self.process_packets();

        output.iter_mut().for_each(|s| *s = 0.);

        for client_state in self.active_clients.values_mut() {
            let played = client_state
                .jitter_buffer
                .pop_slice(output, |old, new| old + new);

            if let Some(shared) = client_state.shared.upgrade() {
                shared.is_talking.store(played, Ordering::Relaxed);
            } else {
                client_state.active = false;
            }
        }

        let volume = self.output_state.volume.load(Ordering::Relaxed);
        output.iter_mut().for_each(|s| *s *= volume);

        self.active_clients.retain(|_, state| state.active);
    }
}

/// Used to enqueue raw audio samples
/// for a playback
pub struct AudioSamplesSender {}
pub struct AudioSamplesRecv {}

#[derive(Clone)]
pub struct AudioOutputState {
    pub is_sound_off: Arc<AtomicBool>,
    pub volume: Arc<AtomicF32>,
}

impl Default for AudioOutputState {
    fn default() -> Self {
        Self {
            is_sound_off: Arc::new(AtomicBool::new(false)),
            volume: Arc::new(AtomicF32::new(1.)),
        }
    }
}

#[derive(Clone)]
pub struct PlaybackController {
    loop_controller: PlatformLoopController,
}

impl PlaybackController {
    pub(crate) fn new(loop_controller: PlatformLoopController) -> Self {
        Self { loop_controller }
    }
}

impl PlaybackController {
    pub fn set_enabled(&self, value: bool) {
        _ = self
            .loop_controller
            .send(AudioLoopCommand::SetEnabledPlayback(value));
    }
}

pub struct Playback {
    pub packet_input: Option<AudioPacketInput>,
    pub controller: PlaybackController,
}

pub(crate) fn init_packet_processing(debug: bool) -> (AudioPacketInput, AudioPacketOutput) {
    let ring = HeapRb::new(24);
    let (packet_prod, packet_cons) = ring.split();

    let (tx, rx) = channel::bounded(14);

    let output_state = AudioOutputState::default();

    let packet_input = AudioPacketInput {
        tx,
        output_state: output_state.clone(),
        packet_buffer: packet_prod,
    };

    let packet_output = AudioPacketOutput {
        rx,

        active_clients: HashMap::new(),
        packet_buffer: packet_cons,
        output_state,

        debug_stats: debug.then(|| Arc::new(Mutex::new(Vec::new()))),
    };

    (packet_input, packet_output)
}
