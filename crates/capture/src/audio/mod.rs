use std::{
    cell::UnsafeCell,
    collections::{BinaryHeap, VecDeque},
    mem::MaybeUninit,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    },
    thread,
    time::Instant,
};

use anyhow::Result as AResult;

use ffmpeg_next::{Packet, codec};
use streaming_common::FFMpegPacketPayload;

use crossbeam::channel;

use crate::audio::{
    decode::AudioDecoder,
    linux::{LinuxCapture, LinuxPlayback},
};

pub mod linux;

pub mod decode;
pub mod encode;

pub const DEFAULT_RATE: u32 = 48000;
pub const DEFAULT_CHANNELS: u32 = 2;

// As recommended per: https://wiki.xiph.org/Opus_Recommended_Settings
pub const DEFAULT_BIT_RATE: usize = 128000;

/// Small utilities that make working with VecDeque buffers more enjoyable
trait VecDequeExt<T> {
    /// Fill the passed buffer with content from the VecDeque.
    /// If `partial` is set to:
    ///     - true: the function tries to fill as much as possible
    ///     - false: the function returns immediately if the Deque has not enough data
    ///
    /// Return: how much items are copied to the passed buffer
    fn pop_slice(&mut self, out: &mut [T], partial: bool) -> usize;

    /// Same as [`VecDequeExt::pop_slice`] but accepts a transformation function
    fn pop_slice_with(&mut self, out: &mut [T], partial: bool, f: impl Fn(T, T) -> T) -> usize;
}

impl<T: Clone + Copy> VecDequeExt<T> for VecDeque<T> {
    #[inline(always)]
    fn pop_slice(&mut self, out: &mut [T], partial: bool) -> usize {
        self.pop_slice_with(out, partial, |_old, new| new)
    }

    #[inline(always)]
    fn pop_slice_with(&mut self, out: &mut [T], partial: bool, f: impl Fn(T, T) -> T) -> usize {
        if !partial && self.len() < out.len() {
            return 0;
        }

        let length = self.len().min(out.len());
        (0..length).for_each(|idx| {
            let value = self.pop_front().unwrap();
            out[idx] = f(out[idx], value);
        });

        length
    }
}

trait StreamingCompatFrom {
    fn to_packet(&self) -> Packet;
}

trait StreamingCompatInto {
    fn to_payload(&self) -> FFMpegPacketPayload;
}

impl StreamingCompatFrom for FFMpegPacketPayload {
    fn to_packet(&self) -> Packet {
        let mut packet = Packet::new(self.data.len());

        packet.set_pts(Some(self.pts));

        packet.set_flags(codec::packet::Flags::from_bits_truncate(self.flags));
        let data = packet
            .data_mut()
            .expect("Should be present because Packet::new");

        data.copy_from_slice(&self.data);

        packet
    }
}

impl StreamingCompatInto for Packet {
    fn to_payload(&self) -> FFMpegPacketPayload {
        FFMpegPacketPayload {
            pts: self.pts().unwrap(),

            flags: self.flags().bits(),
            data: self.data().unwrap_or_default().to_vec(),
        }
    }
}

type PlatformCapture = LinuxCapture;
type PlatformPlayback = LinuxPlayback;

pub struct StreamingClient {
    user_id: i32,
    decoder: AudioDecoder,

    packets: BinaryHeap<FFMpegPacketPayload>,
}

type Slot = UnsafeCell<Option<channel::Sender<Vec<f32>>>>;

/// Playback handle, can be safely shared between threads
pub struct Capture {
    rx: channel::Receiver<Vec<f32>>,

    slot: u8,

    handle: Arc<thread::JoinHandle<()>>,
    is_enabled: Arc<AtomicBool>,

    consumers: Arc<AtomicU8>,
    consumer_slots: Arc<[Slot]>,
}

impl Clone for Capture {
    fn clone(&self) -> Self {
        let consumers = self.consumers.clone();
        let consumer_slots = self.consumer_slots.clone();

        let idx = consumers.fetch_add(1, Ordering::AcqRel);
        let (tx, rx) = channel::unbounded();

        unsafe {
            let slot = &consumer_slots[idx as usize];
            let slot = &mut *slot.get();

            slot.replace(tx);
        }

        Self {
            rx,
            slot: idx,
            handle: self.handle.clone(),
            is_enabled: self.is_enabled.clone(),
            consumers: self.consumers.clone(),
            consumer_slots: self.consumer_slots.clone(),
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let idx = self.consumers.fetch_sub(1, Ordering::AcqRel);

        unsafe {
            let slot = &self.consumer_slots[idx as usize];
            let slot = &mut *slot.get();

            _ = slot.take()
        }
    }
}

impl Capture {
    const MAX_CONSUMERS: usize = 4;

    fn new(platform_capture: PlatformCapture) -> Self {
        let (tx, rx) = channel::unbounded();

        let is_enabled = Arc::new(AtomicBool::new(false));
        let handle = thread::spawn(move || {
            // We start with disabled capturing
            thread::park();
        });

        let consumer_slots: Arc<[Slot]> = (0..Self::MAX_CONSUMERS)
            .map(|i| UnsafeCell::new(None))
            .collect();

        unsafe {
            let slot = &consumer_slots[0];
            let slot = &mut *slot.get();

            slot.replace(tx);
        }

        Self {
            rx,
            is_enabled,
            handle: Arc::new(handle),

            slot: 0,
            consumers: Arc::new(AtomicU8::new(0)),
            consumer_slots,
        }
    }

    fn set_enabled(&self, value: bool) {
        self.is_enabled.store(value, Ordering::Relaxed);
    }
}

/// Playback handle, can be safely shared between threads
#[derive(Clone)]
pub struct Playback {
    volume: Arc<AtomicU8>,
    is_enabled: Arc<AtomicBool>,

    tx: channel::Sender<Vec<f32>>,
}

impl Playback {
    fn new(mut platform_playback: LinuxPlayback) -> Self {
        let (tx, rx) = channel::bounded::<Vec<f32>>(24);

        let volume = Arc::new(AtomicU8::new(140));
        let is_enabled = Arc::new(AtomicBool::new(false));

        thread::spawn({
            move || {
                loop {
                    if let Ok(packet) = rx.recv() {
                        platform_playback.push(&packet);
                    }
                }
            }
        });

        Self {
            tx,
            is_enabled,
            volume,
        }
    }

    pub fn set_enabled(&self, value: bool) {
        self.is_enabled.store(value, Ordering::Relaxed);
    }

    pub fn set_volume(&self, value: u8) {
        self.volume.store(value, Ordering::Relaxed);
    }

    fn send_samples(&self, mut samples: Vec<f32>) {
        let volume = self.volume.load(Ordering::Relaxed);
        let volume: f32 = volume as f32 / 100.;

        samples.iter_mut().for_each(|value| *value *= volume);

        _ = self.tx.send(samples);
    }

    fn process_client(buf: &mut Vec<f32>, client: &mut StreamingClient) {
        // 3 packets is about 60 ms
        if client.packets.len() < 3 {
            return;
        };

        // Safe due the check above
        let packet = client.packets.pop().unwrap();
        client.decoder.decode(packet.to_packet());

        // Mixing if we already have data
        let len = buf.len().min(client.decoder.decoded_samples.len());
        (0..len).for_each(|idx| {
            // Safe due how we derived len
            buf[idx] = client.decoder.decoded_samples.pop_front().unwrap();
        });

        // Pushing the rest
        while let Some(value) = client.decoder.decoded_samples.pop_front() {
            buf.push(value);
        }
    }

    pub fn play_file(&self) {
        todo!()
    }

    pub fn process_streaming(&self, clients: &mut [StreamingClient]) {
        let mut buf: Vec<f32> = vec![];

        for client in clients {
            Self::process_client(&mut buf, client);
        }

        if !buf.is_empty() {
            self.send_samples(buf);
        }
    }
}

pub fn init() {}
