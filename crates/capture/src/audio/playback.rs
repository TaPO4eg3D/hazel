use std::{collections::{HashMap, hash_map::Entry}, sync::{Arc, atomic::{AtomicBool, Ordering}}, thread, time::Instant};

use crossbeam::channel;
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::{Consumer as _, Producer as _, Split as _}};

use crate::audio::{DEFAULT_CHANNELS, DEFAULT_RATE, StreamingClientState, StreamingCompatFrom as _};
#[cfg(target_os = "linux")]
use crate::audio::linux;

#[cfg(target_os = "linux")]
type PlatformPlayback = linux::LinuxPlayback;
#[cfg(target_os = "windows")]
type PlatformPlayback = windows::WindowsPlayback;


/// Playback handle, can be safely shared between threads
#[derive(Clone)]
pub struct Playback {
    is_enabled: Arc<AtomicBool>,

    tx: channel::Sender<(i32, PlaybackChunk)>,
}

impl Playback {
    pub(crate) fn new(mut platform_playback: PlatformPlayback) -> Self {
        let (tx, rx) = channel::bounded::<(i32, PlaybackChunk)>(50);

        let is_enabled = Arc::new(AtomicBool::new(true));

        thread::Builder::new()
            .name("playback-controller".into())
            .spawn({
                let is_enabled = is_enabled.clone();

                move || {
                    loop {
                        while let Ok((user_id, chunk)) = rx.recv() {
                            if !is_enabled.load(Ordering::Relaxed) {
                                continue;
                            }

                            platform_playback.scheduler.push_streaming(user_id, chunk);
                        }
                    }
                }
            })
            .unwrap();

        Self { tx, is_enabled }
    }

    pub fn set_enabled(&self, value: bool) {
        self.is_enabled.store(value, Ordering::Relaxed);
    }

    pub fn process_client(
        &self,
        client: &mut StreamingClientState,
        post_process: impl Fn(&mut PlaybackChunk),
    ) {
        let mut chunk = PlaybackChunk::new();

        // 3 packets is about 60 ms
        if client.packets.len() < 3 {
            return;
        };

        // Safe due the check above
        let packet = client.packets.pop().unwrap().0;
        client.decoder.decode(packet.to_packet());

        while let Some(value) = client.decoder.decoded_samples.pop_front() {
            chunk
                .buffer
                .push_back(value)
                .expect("Decoder output is fixed, it should never fail")
        }

        if !chunk.buffer.is_empty() {
            post_process(&mut chunk);

            _ = self.tx.send((client.user_id, chunk));
        }
    }

    pub fn play_file(&self) {
        todo!()
    }
}

const CHUNK_SIZE: usize = ((DEFAULT_RATE as usize / 1000) * 20) * DEFAULT_CHANNELS as usize;

pub struct PlaybackChunk {
    pub buffer: heapless::Deque<f32, CHUNK_SIZE>,
}

impl PlaybackChunk {
    pub(crate) fn new() -> Self {
        Self {
            buffer: heapless::Deque::new(),
        }
    }
}

struct StreamingQueueItem {
    last_update: Instant,
    queue: heapless::Deque<PlaybackChunk, 128>,
    buffering: bool,
}

impl StreamingQueueItem {
    fn new() -> Self {
        Self {
            last_update: Instant::now(),
            queue: heapless::Deque::new(),
            buffering: false,
        }
    }

    fn pop_slice_with(&mut self, output: &mut [f32], f: impl Fn(f32, f32) -> f32) -> bool {
        const TARGET_BUFFER_SAMPLES: usize = ((DEFAULT_RATE as usize / 1000) * 100) * DEFAULT_CHANNELS as usize;

        let samples_len = self.queue.iter().fold(0, |acc, b| acc + b.buffer.len());

        if self.buffering {
            if samples_len < TARGET_BUFFER_SAMPLES {
                return false;
            }

            self.buffering = false;
        }

        let len = samples_len.min(output.len());
        if len == 0 {
            self.buffering = true;

            return false;
        }

        for out in output[0..len].iter_mut() {
            let sample = match self.queue.get_mut(0).unwrap().buffer.pop_front() {
                Some(sample) => sample,
                None => {
                    _ = self.queue.pop_front();

                    self.queue.get_mut(0)
                        .unwrap()
                        .buffer
                        .pop_front()
                        .expect("We checked total amount of samples")
                }
            };

            *out = f(*out, sample)
        }

        true
    }
}

/// Receiving end of a PlaybackScheduler
pub(crate) struct PlaybackSchedulerRecv {
    streaming_buffer: HeapCons<(i32, PlaybackChunk)>,
    // TODO: Make this buffer heapless as well
    streaming_queue: HashMap<i32, StreamingQueueItem>,
}

impl PlaybackSchedulerRecv {
    fn new(buffer: HeapCons<(i32, PlaybackChunk)>) -> Self {
        Self {
            streaming_buffer: buffer,
            streaming_queue: HashMap::new(),
        }
    }
}

impl PlaybackSchedulerRecv {
    pub(crate) fn pop_slice(&mut self, output: &mut [f32]) {
        // Process pending items
        while let Some((user_id, chunk)) = self.streaming_buffer.try_pop() {
            match self.streaming_queue.entry(user_id) {
                Entry::Occupied(mut entry) => {
                    let item = entry.get_mut();

                    item.last_update = Instant::now();
                    _ = item.queue.push_back(chunk);
                }
                Entry::Vacant(entry) => {
                    let item = entry.insert(StreamingQueueItem::new());

                    _ = item.queue.push_back(chunk);
                }
            };
        }

        output.iter_mut().for_each(|s| *s = 0.);

        for queue in self.streaming_queue.values_mut() {
            queue.pop_slice_with(output, |old, new| old + new);
        }
    }
}

/// Schedules audio for a playback
pub(crate) struct PlaybackSchedulerSender {
    streaming_buffer: HeapProd<(i32, PlaybackChunk)>,
}

impl PlaybackSchedulerSender {
    fn new(buffer: HeapProd<(i32, PlaybackChunk)>) -> Self {
        Self {
            streaming_buffer: buffer,
        }
    }
}

impl PlaybackSchedulerSender {
    pub(crate) fn push_streaming(&mut self, user_id: i32, chunk: PlaybackChunk) {
        _ = self.streaming_buffer.try_push((user_id, chunk));
    }
}

pub(crate) fn create_playback_scheduler() -> (PlaybackSchedulerSender, PlaybackSchedulerRecv) {
    let ring = HeapRb::<(i32, PlaybackChunk)>::new(150);
    let (streaming_prod, streaming_cons) = ring.split();

    let sender = PlaybackSchedulerSender::new(streaming_prod);
    let recv = PlaybackSchedulerRecv::new(streaming_cons);

    (sender, recv)
}
