use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Poll, Waker},
    thread::{self, Thread},
    time::Duration,
};

use crossbeam::channel;
use streaming_common::EncodedAudioPacket;

use crate::audio::{capture::Capture, encode::AudioEncoder};
use crate::audio::playback::{Playback, init_packet_processing};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "windows")]
pub mod windows;

pub mod decode;
pub mod encode;
pub mod playback;
pub mod capture;
pub mod noise;

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

#[derive(Clone)]
pub struct DeviceRegistry {
    inner: Arc<RwLock<DeviceRegistryInner>>,
}

pub struct DeviceSubscription {
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

impl DeviceSubscription {
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

    pub fn subscribe(self) -> DeviceSubscription {
        DeviceSubscription {
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
pub(crate) type PlatformLoopController = windows::CommandSender<AudioLoopCommand>;
#[cfg(target_os = "linux")]
pub(crate) type PlatformLoopController = pipewire::channel::Sender<AudioLoopCommand>;

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

#[derive(Debug)]
pub enum AudioLoopCommand {
    SetEnabledCapture(bool),
    SetEnabledPlayback(bool),

    SetActiveInputDevice(AudioDevice),
    SetActiveOutputDevice(AudioDevice),
}

pub fn init(debug: bool) -> (Capture, Playback, DeviceRegistry) {
    let (packet_input, packet_output) = init_packet_processing(debug);

    if debug {
        let debug_stats = packet_output.debug_stats.clone();

        thread::spawn(move || {
            loop {
                let items = {
                    let mut debug_stats = debug_stats.as_ref().unwrap().lock().unwrap();

                    debug_stats.retain(|(_, stats)| stats.strong_count() > 0);
                    debug_stats
                        .iter()
                        .filter_map(|(user_id, stats)| {
                            let stats = stats.upgrade()?;
                            let stats = stats.lock().unwrap();

                            Some((*user_id, stats.clone()))
                        })
                        .collect::<Vec<_>>()
                };

                println!("AUDIO: {items:#?}");
                thread::sleep(Duration::from_secs(10));
            }
        });
    }

    #[cfg(target_os = "linux")]
    let (capture, playback, device_registry) = linux::init(packet_input, packet_output);
    #[cfg(target_os = "windows")]
    let (capture, playback, device_registry) = windows::init(packet_input, packet_output);

    (capture, playback, device_registry)
}
