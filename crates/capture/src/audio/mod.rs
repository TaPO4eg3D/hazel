use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, VecDeque, hash_map::Entry},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    },
    task::{Poll, Waker},
    thread::{self, Thread},
    time::{Duration, Instant},
};

use ffmpeg_next::{Packet, codec, device::output};
use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Producer, Split as _},
};
use streaming_common::{DATA_BUFF_SIZE, FFMpegPacketPayload};

use crossbeam::channel;

use crate::audio::{decode::AudioDecoder, encode::AudioEncoder};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "windows")]
pub mod windows;

pub mod decode;
pub mod encode;

/// Sampling rate per channel
pub const DEFAULT_RATE: u32 = 48000;
pub const DEFAULT_CHANNELS: u32 = 2;

// As recommended per: https://wiki.xiph.org/Opus_Recommended_Settings
pub const DEFAULT_BIT_RATE: usize = 128000;

/// Small utilities that make working with VecDeque buffers more enjoyable
pub(crate) trait VecDequeExt<T> {
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

/// Wakes up a sleeping thread when data
/// is available for consumption
#[derive(Clone, Default)]
pub(crate) struct Notifier {
    thread: Arc<Mutex<Option<Thread>>>,
}

impl Notifier {
    pub fn new() -> Self {
        Self {
            thread: Arc::new(Mutex::new(None)),
        }
    }

    pub fn notify(&self) {
        let handle = {
            let guard = self.thread.lock().unwrap();

            guard.clone()
        };

        if let Some(thread) = handle {
            thread.unpark();
        }
    }

    pub fn listen_updates(&self) {
        let mut guard = self.thread.lock().unwrap();
        *guard = Some(std::thread::current());
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
        let data = &self.data[..self.items as usize];

        // TODO: It results in allocation, improve?
        let mut packet = Packet::new(data.len());

        packet.set_pts(Some(self.pts));

        packet.set_flags(codec::packet::Flags::from_bits_truncate(self.flags));
        let packet_data = packet
            .data_mut()
            .expect("Should be present because Packet::new");

        packet_data.copy_from_slice(data);

        packet
    }
}

impl StreamingCompatInto for Packet {
    fn to_payload(&self) -> FFMpegPacketPayload {
        let mut buffer = [0; DATA_BUFF_SIZE];
        let packet_data = self.data().unwrap_or_default();

        for (i, value) in packet_data.iter().enumerate() {
            buffer[i] = *value;
        }

        FFMpegPacketPayload {
            pts: self.pts().unwrap(),

            flags: self.flags().bits(),
            items: packet_data.len() as u32,
            data: buffer,
        }
    }
}

#[derive(Clone)]
pub struct DeviceRegistry {
    inner: Arc<RwLock<DeviceRegistryInner>>,
}

pub struct Subscription {
    is_first_fetch: bool,

    registry: DeviceRegistry,
}

pub struct RecvFuture {
    first: bool,
    registry: DeviceRegistry,
}

impl Future for RecvFuture {
    type Output = DeviceRegistry;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Self::Output> {
        if !self.first {
            return Poll::Ready(self.registry.clone());
        }

        self.as_mut().first = false;

        let waker = cx.waker().clone();
        let mut registry = self.registry.inner.write().unwrap();
        registry.tasks.push(waker);

        Poll::Pending
    }
}

impl Subscription {
    pub fn recv(&mut self) -> RecvFuture {
        let future = RecvFuture {
            first: !self.is_first_fetch,
            registry: self.registry.clone(),
        };

        self.is_first_fetch = false;

        future
    }
}

impl DeviceRegistry {
    pub fn new(controller: PlatformLoopController) -> Self {
        Self {
            inner: Arc::new(RwLock::new(DeviceRegistryInner::new(controller))),
        }
    }

    pub fn subscribe(self) -> Subscription {
        Subscription {
            is_first_fetch: true,
            registry: self,
        }
    }

    pub fn get_input_devices(&self) -> Vec<AudioDevice> {
        let registry = self.inner.read().unwrap();

        registry.input.clone()
    }

    pub fn get_output_devices(&self) -> Vec<AudioDevice> {
        let registry = self.inner.read().unwrap();

        registry.output.clone()
    }

    pub fn set_active_input(&self, device: &AudioDevice) {
        let registry = self.inner.read().unwrap();

        _ = registry
            .platform_loop_controller
            .send(AudioLoopCommand::SetActiveInputDevice(device.clone()));
    }

    pub fn set_active_output(&self, device: &AudioDevice) {
        let registry = self.inner.read().unwrap();

        _ = registry
            .platform_loop_controller
            .send(AudioLoopCommand::SetActiveOutputDevice(device.clone()));
    }

    pub(crate) fn add_input(&self, device: AudioDevice) {
        let mut registry = self.inner.write().unwrap();
        registry.input.push(device);

        registry.notify();
    }

    pub(crate) fn add_output(&self, device: AudioDevice) {
        let mut registry = self.inner.write().unwrap();
        registry.output.push(device);

        registry.notify();
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn find_by_node_id(&self, id: u32) -> Option<String> {
        let registry = self.inner.read().unwrap();

        registry
            .input
            .iter()
            .find(|item| item.node_id == id)
            .or(registry.output.iter().find(|item| item.node_id == id))
            .map(|item| item.id.clone())
    }

    pub(crate) fn mark_active_input(&self, id: &str) {
        let mut registry = self.inner.write().unwrap();

        registry
            .input
            .iter_mut()
            .for_each(|item| item.is_active = item.id == id);

        registry.notify();
    }

    pub(crate) fn mark_active_output(&self, id: &str) {
        let mut registry = self.inner.write().unwrap();

        registry
            .output
            .iter_mut()
            .for_each(|item| item.is_active = item.id == id);

        registry.notify();
    }

    pub(crate) fn device_exists(&self, id: &str) -> bool {
        let registry = self.inner.read().unwrap();

        registry.input.iter().any(|item| item.id == id)
            || registry.output.iter().any(|item| item.id == id)
    }

    pub(crate) fn remove_device(&self, id: &str) {
        let mut registry = self.inner.write().unwrap();

        if registry.input.iter().any(|item| item.id == id)
            || registry.output.iter().any(|item| item.id == id)
        {
            registry.input.retain(|item| item.id != id);
            registry.output.retain(|item| item.id != id);
        }

        registry.notify();
    }
}

#[cfg(target_os = "windows")]
type PlatformLoopController = windows::CommandSender<AudioLoopCommand>;
#[cfg(target_os = "linux")]
type PlatformLoopController = pipewire::channel::Sender<AudioLoopCommand>;

struct DeviceRegistryInner {
    input: Vec<AudioDevice>,
    output: Vec<AudioDevice>,

    platform_loop_controller: PlatformLoopController,

    tasks: Vec<Waker>,
}

impl DeviceRegistryInner {
    fn new(controller: PlatformLoopController) -> Self {
        Self {
            input: vec![],
            output: vec![],
            tasks: vec![],

            platform_loop_controller: controller,
        }
    }

    fn notify(&mut self) {
        while let Some(waker) = self.tasks.pop() {
            waker.wake();
        }
    }
}

// TODO: Change `String` to `SharedString`
#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub id: String,
    #[cfg(target_os = "linux")]
    pub node_id: u32,

    pub display_name: String,

    pub is_active: bool,
}

#[cfg(target_os = "linux")]
type PlatformCapture = linux::LinuxCapture;
#[cfg(target_os = "windows")]
type PlatformCapture = windows::WindowsCapture;

#[cfg(target_os = "linux")]
type PlatformPlayback = linux::LinuxPlayback;
#[cfg(target_os = "windows")]
type PlatformPlayback = windows::WindowsPlayback;

pub struct StreamingClientState {
    pub user_id: i32,
    decoder: AudioDecoder,

    /// We buffer packets to decode them in correct order
    packets: BinaryHeap<Reverse<FFMpegPacketPayload>>,
}

impl StreamingClientState {
    pub fn new(user_id: i32) -> Self {
        Self {
            user_id,
            decoder: AudioDecoder::new(),
            packets: BinaryHeap::new(),
        }
    }

    pub fn push(&mut self, packet: FFMpegPacketPayload) {
        self.packets.push(Reverse(packet));
    }
}

pub enum AudioLoopCommand {
    SetEnabledCapture(bool),
    SetEnabledPlayback(bool),

    SetActiveInputDevice(AudioDevice),
    SetActiveOutputDevice(AudioDevice),
}

/// (id, Sender)
type CaptureConsumer = (usize, channel::Sender<Vec<f32>>);

/// Playback handle, can be safely shared between threads
#[derive(Clone)]
pub struct Capture {
    idx_count: Arc<AtomicUsize>,

    handle: Arc<thread::JoinHandle<()>>,
    is_enabled: Arc<AtomicBool>,

    platform_loop_controller: PlatformLoopController,
    consumers: Arc<RwLock<Vec<CaptureConsumer>>>,
}

pub struct CaptureReciever<'a> {
    idx: usize,
    pub rx: channel::Receiver<Vec<f32>>,
    encoder: AudioEncoder,
    capture: &'a Capture,
}

pub struct EncodedRecv<'a> {
    encoder: &'a mut AudioEncoder,
}

impl<'a> EncodedRecv<'a> {
    pub fn pop(&mut self) -> Option<FFMpegPacketPayload> {
        self.encoder.pop_packet()
    }
}

impl<'a> CaptureReciever<'a> {
    fn new(capture: &'a Capture) -> CaptureReciever<'a> {
        let mut recievers = capture.consumers.write().unwrap();

        let idx = capture.idx_count.fetch_add(1, Ordering::AcqRel);

        let (tx, rx) = channel::unbounded();
        recievers.push((idx, tx));

        Self {
            idx,
            encoder: AudioEncoder::new(),
            rx,
            capture,
        }
    }

    pub fn recv_encoded<'b>(&'b mut self) -> EncodedRecv<'b> {
        if let Ok(samples) = self.rx.recv() {
            self.encoder.encode(&samples);
        }

        EncodedRecv {
            encoder: &mut self.encoder,
        }
    }

    pub fn recv_encoded_with<'b>(
        &'b mut self,
        f: impl Fn(Vec<f32>) -> Option<Vec<f32>>,
    ) -> EncodedRecv<'b> {
        if let Ok(samples) = self.rx.recv()
            && let Some(samples) = f(samples)
        {
            self.encoder.encode(&samples);
        }

        EncodedRecv {
            encoder: &mut self.encoder,
        }
    }
}

impl<'a> Drop for CaptureReciever<'a> {
    fn drop(&mut self) {
        let mut recievers = self.capture.consumers.write().unwrap();

        recievers.retain(|(id, _)| *id != self.idx);

        if recievers.is_empty() {
            self.capture.set_enabled(false);
        }
    }
}

impl Capture {
    fn new(mut platform_capture: PlatformCapture) -> Self {
        let is_enabled = Arc::new(AtomicBool::new(false));
        let platform_loop_controller = platform_capture.get_controller();

        let consumers: Arc<RwLock<Vec<CaptureConsumer>>> = Arc::new(RwLock::new(Vec::new()));

        let handle = thread::Builder::new()
            .name("capture-controller".into())
            .spawn({
                let consumers = consumers.clone();
                let is_enabled = is_enabled.clone();

                move || {
                    let mut buf = vec![0.; (DEFAULT_RATE * DEFAULT_CHANNELS) as usize];

                    // IMPORTANT: without this function, the thread
                    // will not be unparked on new data
                    platform_capture.listen_updates();

                    // We start with disabled capturing
                    thread::park();

                    loop {
                        if !is_enabled.load(Ordering::Relaxed) {
                            std::thread::park();
                        }

                        let len = platform_capture.pop(&mut buf);
                        if len == 0 {
                            continue;
                        }

                        let consumers = consumers.read().unwrap();

                        for (_, consumer) in consumers.iter() {
                            _ = consumer.send(buf[0..len].to_vec());
                        }
                    }
                }
            })
            .unwrap();

        Self {
            is_enabled,
            consumers,
            platform_loop_controller,
            handle: Arc::new(handle),
            idx_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// TODO: Make it a builder API. To build your receiver in layers
    pub fn get_recv(&self) -> CaptureReciever<'_> {
        CaptureReciever::new(self)
    }

    pub fn set_enabled(&self, value: bool) {
        self.is_enabled.store(value, Ordering::Relaxed);

        _ = self
            .platform_loop_controller
            .send(AudioLoopCommand::SetEnabledCapture(value));

        if value {
            self.handle.thread().unpark();
        }
    }
}

/// Playback handle, can be safely shared between threads
#[derive(Clone)]
pub struct Playback {
    is_enabled: Arc<AtomicBool>,

    tx: channel::Sender<(i32, PlaybackChunk)>,
}

impl Playback {
    fn new(mut platform_playback: PlatformPlayback) -> Self {
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

#[cfg(target_os = "linux")]
pub fn init() -> (Capture, Playback, DeviceRegistry) {
    let (capture, playback, device_registry) = linux::init();

    let capture = Capture::new(capture);
    let playback = Playback::new(playback);

    (capture, playback, device_registry)
}

#[cfg(target_os = "windows")]
pub fn init() -> (Capture, Playback, DeviceRegistry) {
    let (capture, playback, device_registry) = windows::init();

    let capture = Capture::new(capture);
    let playback = Playback::new(playback);

    (capture, playback, device_registry)
}
