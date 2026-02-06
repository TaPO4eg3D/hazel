use std::{
    cmp::Reverse, collections::{BinaryHeap, VecDeque}, sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    }, task::{Poll, Waker}, thread
};


use ffmpeg_next::{Packet, codec};
use streaming_common::FFMpegPacketPayload;

use crossbeam::channel;

use crate::audio::{
    decode::AudioDecoder,
    encode::AudioEncoder,
    linux::{LinuxCapture, LinuxPlayback},
};

pub mod linux;

pub mod decode;
pub mod encode;

/// Sampling rate per channel
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

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
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
    pub fn new(controller: pipewire::channel::Sender<AudioLoopCommand>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(DeviceRegistryInner::new(controller)))
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

        _ = registry.platform_loop_controller.send(AudioLoopCommand::SetActiveInputDevice(
            device.clone()
        ));
    }

    pub fn set_active_output(&self, device: &AudioDevice) {
        let registry = self.inner.read().unwrap();

        _ = registry.platform_loop_controller.send(AudioLoopCommand::SetActiveOutputDevice(
            device.clone()
        ));
    }

    fn add_input(&self, device: AudioDevice) {
        let mut registry = self.inner.write().unwrap();
        registry.input.push(device);

        registry.notify();
    }

    fn add_output(&self, device: AudioDevice) {
        let mut registry = self.inner.write().unwrap();
        registry.output.push(device);

        registry.notify();
    }

    fn mark_active_input(&self, id: u32) {
        let mut registry = self.inner.write().unwrap();

        registry
            .input
            .iter_mut()
            .for_each(|item| item.is_active = item.id == id);

        registry.notify();
    }

    fn mark_active_output(&self, id: u32) {
        let mut registry = self.inner.write().unwrap();

        registry
            .output
            .iter_mut()
            .for_each(|item| item.is_active = item.id == id);

        registry.notify();
    }

    fn device_exists(&self, id: u32) -> bool {
        let registry = self.inner.read().unwrap();

        registry.input.iter().any(|item| item.id == id)
            || registry.output.iter().any(|item| item.id == id)
    }

    fn remove_device(&self, id: u32) {
        let mut registry = self.inner.write().unwrap();

        if registry.input.iter().any(|item| item.id == id)
            || registry.output.iter().any(|item| item.id == id)
        {
            registry.input.retain(|item| item.id != id);
            registry.output.retain(|item| item.id != id);

        }
    }
}

struct DeviceRegistryInner {
    input: Vec<AudioDevice>,
    output: Vec<AudioDevice>,

    platform_loop_controller: pipewire::channel::Sender<AudioLoopCommand>,

    tasks: Vec<Waker>,
}

impl DeviceRegistryInner {
    fn new(controller: pipewire::channel::Sender<AudioLoopCommand>) -> Self {
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

#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub id: u32,

    pub name: String,
    pub display_name: String,

    pub is_active: bool,
}


type PlatformCapture = LinuxCapture;
type PlatformPlayback = LinuxPlayback;

pub struct StreamingClientState {
    pub user_id: i32,
    decoder: AudioDecoder,

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

    platform_loop_controller: pipewire::channel::Sender<AudioLoopCommand>,
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
    volume: Arc<AtomicU8>,
    is_enabled: Arc<AtomicBool>,

    tx: channel::Sender<Vec<f32>>,
}

impl Playback {
    fn new(mut platform_playback: LinuxPlayback) -> Self {
        let (tx, rx) = channel::bounded::<Vec<f32>>(24);

        let volume = Arc::new(AtomicU8::new(140));
        let is_enabled = Arc::new(AtomicBool::new(true));

        thread::Builder::new()
            .name("playback-controller".into())
            .spawn({
                let is_enabled = is_enabled.clone();

                move || {
                    loop {
                        if let Ok(packet) = rx.recv() {
                            if !is_enabled.load(Ordering::Relaxed) {
                                continue;
                            }

                            platform_playback.push(&packet);
                        }
                    }
                }
            })
            .unwrap();

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

    pub fn send_samples(&self, mut samples: Vec<f32>) {
        let volume = self.volume.load(Ordering::Relaxed);
        let volume: f32 = volume as f32 / 100.;

        samples.iter_mut().for_each(|value| *value *= volume);

        _ = self.tx.send(samples);
    }

    fn process_client(buf: &mut Vec<f32>, client: &mut StreamingClientState) {
        // 3 packets is about 60 ms
        if client.packets.len() < 3 {
            return;
        };

        // Safe due the check above
        let packet = client.packets.pop().unwrap().0;
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

    pub fn process_streaming<'a>(
        &self,
        clients: impl Iterator<Item = &'a mut StreamingClientState>,
        post_process: impl Fn(Vec<f32>) -> Vec<f32>,
    ) {
        let mut buf: Vec<f32> = vec![];

        for client in clients {
            Self::process_client(&mut buf, client);
        }

        if !buf.is_empty() {
            self.send_samples(post_process(buf));
        }
    }
}

pub fn init() -> (Capture, Playback, DeviceRegistry) {
    let (capture, playback, device_registry) = linux::init();

    let capture = Capture::new(capture);
    let playback = Playback::new(playback);

    (capture, playback, device_registry)
}
