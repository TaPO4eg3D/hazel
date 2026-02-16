use std::{
    cmp::Reverse,
    collections::{BTreeMap, BinaryHeap, HashMap, hash_map::Entry},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Instant,
};

use crossbeam::channel;
use ffmpeg_next::Packet;
use heapless::Deque;
use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer as _, Producer as _, Split as _},
};
use streaming_common::FFMpegPacketPayload;

#[cfg(target_os = "linux")]
use crate::audio::linux;
use crate::audio::{
    DEFAULT_CHANNELS, DEFAULT_RATE, StreamingCompatFrom as _, VecDequeExt, decode::AudioDecoder
};

#[cfg(target_os = "linux")]
type PlatformPlayback = linux::LinuxPlayback;
#[cfg(target_os = "windows")]
type PlatformPlayback = windows::WindowsPlayback;


const SAMPLES_BUFFER: usize = (DEFAULT_RATE * DEFAULT_CHANNELS) as usize;

struct JitterBuffer {
    decoder: AudioDecoder,

    packets_buffer: BTreeMap<u64, (Instant, FFMpegPacketPayload)>,
    samples_buffer: heapless::Deque<f32, SAMPLES_BUFFER>,

    // PTS of the next expected packet
    next_playout_pts: Option<u64>,
    target_delay_ms: f64,

    min_delay_ms: f64,
    max_delay_ms: f64,

    jitter_estimate_ms: f64,

    /// Smoothing factor for jitter estimation (0.0 - 1.0)
    alpha: f64,

    // PTS of a packet that marks the end of the
    // speech chunk. If there's no packets after this
    // PTS, we don't need to treat it as jittering
    ending_chunk: Option<u64>,

    last_arrival: Option<Instant>,
    last_pts: Option<u64>,

    // How many times had to generate PLC in a row
    misses: u32,
}


impl JitterBuffer {
    fn new() -> Self {
        Self {
            decoder: AudioDecoder::new(),
            packets_buffer: BTreeMap::new(),
            samples_buffer: heapless::Deque::new(),
            next_playout_pts: None,
            target_delay_ms: 20.,
            min_delay_ms: 20.,
            max_delay_ms: 200.,
            jitter_estimate_ms: 0.0,
            alpha: 0.05,
            last_arrival: None,
            last_pts: None,
            misses: 0,
            ending_chunk: None,
        }
    }

    fn push_packet(&mut self, arrival_ts: Instant, packet: FFMpegPacketPayload) {
        // Packet arrived out of order, we already finished with
        // this speech chunk
        if let Some(pts) = self.ending_chunk
            && self.next_playout_pts.is_none()
            && packet.pts < pts {
            return;
        }

        self.update_jitter(arrival_ts, &packet);
        self.adapt_target_delay();

        // Special packet, means the end of the speech chunk
        if packet.items == -1 {
            self.ending_chunk = Some(packet.pts);

            return;
        }

        // Packet arrived out of order, we already played PLC
        if let Some(next) = self.next_playout_pts
            && packet.pts < next {
                return;
            }

        const MAX_BUFFER_SIZE: usize = 20;
        if self.packets_buffer.len() >= MAX_BUFFER_SIZE
            && let Some(&oldest_seq) = self.packets_buffer.keys().next() {
                self.packets_buffer.remove(&oldest_seq);
            }

        self.packets_buffer.insert(packet.pts, (arrival_ts, packet));
    }

    fn close_speech_chunk(&mut self) {
        self.last_pts = None;
        self.last_arrival = None;

        self.next_playout_pts = None;
    }

    fn update_jitter(&mut self, arrival_ts: Instant, packet: &FFMpegPacketPayload) {
        if let (Some(last_arrival), Some(last_pts)) = (self.last_arrival, self.last_pts) {
            let arrival_diff_ms = arrival_ts
                .duration_since(last_arrival)
                .as_secs_f64() * 1000.;

            let ts_diff_samples = packet.pts.wrapping_sub(last_pts) as f64;
            let ts_diff_ms = (ts_diff_samples / DEFAULT_RATE as f64) * 1000.;

            let deviation = (arrival_diff_ms - ts_diff_ms).abs();

            // Exponential moving average
            self.jitter_estimate_ms =
                self.jitter_estimate_ms * (1.0 - self.alpha) + deviation * self.alpha;
        }

        self.last_arrival = Some(arrival_ts);
        self.last_pts = Some(packet.pts);
    }

    fn adapt_target_delay(&mut self) {
        let desired = self.jitter_estimate_ms * 2.0;
        let adjustment_rate = 0.1;

        self.target_delay_ms += (desired - self.target_delay_ms) * adjustment_rate;
        self.target_delay_ms = self.target_delay_ms.clamp(self.min_delay_ms, self.max_delay_ms);
    }

    fn decode(&mut self) -> bool {
        if self.next_playout_pts.is_none() {
            if let Some((&pts, (arrival_ts, _))) = self.packets_buffer.iter().next() {
                let buffered_ms = arrival_ts.elapsed().as_secs_f64() * 1000.0;

                if buffered_ms < self.target_delay_ms {
                    return false;
                }

                self.next_playout_pts = Some(pts);
            } else {
                return false;
            }
        }

        let pts = self.next_playout_pts.unwrap();
        if self.ending_chunk.is_some_and(|end_pts| end_pts == pts) {
            self.close_speech_chunk();

            return false;
        }

        if let Some((_, packet)) = self.packets_buffer.remove(&pts) {
            self.misses = 0;
            self.next_playout_pts = Some(pts.wrapping_add(1));

            self.decoder.decode(packet.to_packet());
        } else {
            println!("Missing packet!");

            self.misses += 1;
            // If have have too much misses, we probably missed the marker
            // and we need to close the speech chunk
            if self.misses > 4 {
                self.close_speech_chunk();

                return false;
            }

            // Packet is missing, ask decoder for PLC
            self.decoder.decode(Packet::new(0));
        }

        while let Some(decoded_sample) = self.decoder.decoded_samples.pop_front() {
            self.samples_buffer.push_back(decoded_sample);
        }

        true
    }

    fn pop_slice(&mut self, output: &mut [f32]) {
        let mut i = 0;

        while i < output.len() {
            if let Some(sample) = self.samples_buffer.pop_front() {
                output[i] = sample;
                i += 1;

                continue;
            };

            // If we failed to decode anything, fill with zeroes
            if !self.decode() {
                output[i..]
                    .iter_mut()
                    .for_each(|s| *s = 0.);

                break;
            }
        }
    }
}


pub struct StreamingClientState {
    pub user_id: i32,
    jitter_buffer: JitterBuffer,
}


impl StreamingClientState {
    pub fn new(user_id: i32) -> Self {
        Self {
            user_id,
            jitter_buffer: JitterBuffer::new(),
        }
    }
}

/// Used to enqueue streaming audio packets
/// for a playback
pub struct AudioPacketSender {
}

impl AudioPacketSender {
}

/// Used to enqueue raw audio samples
/// for a playback
pub struct AudioSamplesSender {
}

#[derive(Clone)]
pub struct AudioStreamingState {
    is_muted: Arc<AtomicBool>,
    volume: Arc<AtomicBool>,
}


pub fn init_playback() {
}
