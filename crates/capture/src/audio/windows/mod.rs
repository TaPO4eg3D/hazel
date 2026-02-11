//! TODO: Migrate to safe WASAPI wrapper? Like this one: https://github.com/HEnquist/wasapi-rs

use std::{
    sync::{Arc, Mutex},
    thread,
};

use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Observer as _, Producer, Split as _},
};
use windows::{
    Win32::{
        Foundation::{HANDLE, WAIT_OBJECT_0},
        Media::Audio::{
            IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl,
            MMDeviceEnumerator,
        },
        System::{
            Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx},
            Threading::{CreateEventW, SetEvent, WaitForMultipleObjects},
        },
    },
    core::implement,
};

use crate::audio::{
    AudioDevice, AudioLoopCommand, DEFAULT_CHANNELS, DEFAULT_RATE, DeviceRegistry, Notifier,
    windows::{capture::CaptureStream, playback::PlaybackStream},
};

pub mod capture;
pub mod playback;

#[implement(IMMNotificationClient)]
struct DeviceNotifier {}

impl IMMNotificationClient_Impl for DeviceNotifier_Impl {
    fn OnDeviceAdded(&self, pwstrdeviceid: &windows_core::PCWSTR) -> windows_core::Result<()> {
        Ok(())
    }

    fn OnDeviceRemoved(&self, pwstrdeviceid: &windows_core::PCWSTR) -> windows_core::Result<()> {
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        flow: windows::Win32::Media::Audio::EDataFlow,
        role: windows::Win32::Media::Audio::ERole,
        pwstrdefaultdeviceid: &windows_core::PCWSTR,
    ) -> windows_core::Result<()> {
        Ok(())
    }

    fn OnDeviceStateChanged(
        &self,
        pwstrdeviceid: &windows_core::PCWSTR,
        dwnewstate: windows::Win32::Media::Audio::DEVICE_STATE,
    ) -> windows_core::Result<()> {
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        pwstrdeviceid: &windows_core::PCWSTR,
        key: &windows::Win32::Foundation::PROPERTYKEY,
    ) -> windows_core::Result<()> {
        Ok(())
    }
}

pub struct WindowsCapture {
    notifier: Notifier,
    loop_controller: CommandSender<AudioLoopCommand>,
    capture_consumer: HeapCons<f32>,
}

impl WindowsCapture {
    pub fn get_controller(&self) -> CommandSender<AudioLoopCommand> {
        self.loop_controller.clone()
    }

    pub fn listen_updates(&self) {
        self.notifier.listen_updates();
    }

    pub fn pop(&mut self, buf: &mut [f32]) -> usize {
        if self.capture_consumer.occupied_len() == 0 {
            std::thread::park();
        }

        self.capture_consumer.pop_slice(buf)
    }
}

pub struct WindowsPlayback {
    playback_producer: HeapProd<f32>,
    loop_controller: CommandSender<AudioLoopCommand>,
}

impl WindowsPlayback {
    pub fn push(&mut self, data: &[f32]) {
        self.playback_producer.push_slice(data);
    }
}

struct ChannelState<T> {
    inner: Arc<Mutex<Option<T>>>,
}

impl<T> Clone for ChannelState<T> {
    fn clone(&self) -> Self {
        ChannelState {
            inner: self.inner.clone(),
        }
    }
}

impl<T> ChannelState<T> {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }

    fn take(&self) -> Option<T> {
        let mut state = self.inner.lock().unwrap();

        state.take()
    }
}

#[derive(Clone, Copy)]
#[repr(transparent)]
struct EventHandle(HANDLE);

unsafe impl Send for EventHandle {}
unsafe impl Sync for EventHandle {}

pub struct CommandSender<T> {
    event: EventHandle,
    state: ChannelState<T>,
}

impl<T> Clone for CommandSender<T> {
    fn clone(&self) -> Self {
        Self {
            event: self.event,
            state: self.state.clone(),
        }
    }
}

impl<T> CommandSender<T> {
    pub fn send(&self, msg: T) {
        {
            let mut state = self.state.inner.lock().unwrap();
            *state = Some(msg);
        }

        unsafe {
            _ = SetEvent(self.event.0);
        }
    }
}

fn chnannel<T>() -> (EventHandle, ChannelState<T>, CommandSender<T>) {
    unsafe {
        let event = CreateEventW(None, false, false, None).unwrap();
        let state = ChannelState::new();

        (
            EventHandle(event),
            state.clone(),
            CommandSender {
                event: EventHandle(event),
                state: state,
            },
        )
    }
}

pub fn init() -> (WindowsCapture, WindowsPlayback, DeviceRegistry) {
    // We capture in mono and there's no point to store
    // more than 60ms
    let ring = HeapRb::<f32>::new(((DEFAULT_RATE / 1000) * 60) as usize);
    let (capture_producer, capture_consumer) = ring.split();

    let ring = HeapRb::<f32>::new((DEFAULT_RATE * DEFAULT_CHANNELS) as usize);
    let (playback_producer, playback_consumer) = ring.split();

    let (command_event, command_state, sender) = chnannel::<AudioLoopCommand>();

    let capture_notifier = Notifier::new();
    let capture = WindowsCapture {
        capture_consumer,
        loop_controller: sender.clone(),
        notifier: capture_notifier.clone(),
    };

    let playback = WindowsPlayback {
        playback_producer,
        loop_controller: sender.clone(),
    };

    let _device_registry = DeviceRegistry::new(sender);

    _ = thread::Builder::new()
        .name("wasapi-loop".into())
        .spawn(move || unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .expect("Failed to init COM library");

            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .expect("Failed to create device enumerator");

            let notifier = DeviceNotifier {};
            let notification_client: IMMNotificationClient = notifier.into();

            enumerator
                .RegisterEndpointNotificationCallback(&notification_client)
                .unwrap();

            let mut capture_stream = CaptureStream::new(capture_producer, capture_notifier.clone())
                .expect("Failed to init capture");
            let mut playback_stream =
                PlaybackStream::new(playback_consumer).expect("Failed to init playback");

            let command_event = command_event;
            let mut preffered_device: Option<AudioDevice> = None;

            loop {
                let wait_result = WaitForMultipleObjects(
                    &[
                        capture_stream.event_handle,
                        playback_stream.event_handle,
                        command_event.0,
                    ],
                    false, // wake on any
                    2000,
                );

                if wait_result == WAIT_OBJECT_0 {
                    // Failure most likely means that device has changed
                    if capture_stream.process().is_err() {
                        let producer = capture_stream.capture_producer.take().unwrap();

                        capture_stream = CaptureStream::new(producer, capture_notifier.clone())
                            .expect("Failed to recreate the capture stream");
                    }
                } else if wait_result.0 == WAIT_OBJECT_0.0 + 1 {
                    // Failure most likely means that device has changed
                    if playback_stream.process().is_err() {
                        let consumer = playback_stream.playback_consumer.take().unwrap();

                        playback_stream = PlaybackStream::new(consumer)
                            .expect("Failed to recreate the playback stream");
                    }
                } else if wait_result.0 == WAIT_OBJECT_0.0 + 2 {
                    if let Some(event) = command_state.take() {
                        match event {
                            AudioLoopCommand::SetActiveInputDevice(device) => {
                                let producer = capture_stream.capture_producer.take().unwrap();

                                capture_stream =
                                    CaptureStream::new(producer, capture_notifier.clone())
                                        .expect("Failed to recreate the capture stream");
                            }
                            AudioLoopCommand::SetActiveOutputDevice(device) => {
                                let consumer = playback_stream.playback_consumer.take().unwrap();

                                playback_stream = PlaybackStream::new(consumer)
                                    .expect("Failed to recreate the playback stream");
                            }
                            AudioLoopCommand::SetEnabledCapture(value) => {
                                _ = capture_stream.set_enabled(value);
                            }
                            AudioLoopCommand::SetEnabledPlayback(value) => {
                                _ = playback_stream.set_enabled(value);
                            }
                        }
                    }
                } else {
                    panic!("Timeout!");
                }
            }
        })
        .unwrap();

    (capture, playback, _device_registry)
}
