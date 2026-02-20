//! TODO: Migrate to safe WASAPI wrapper? Like this one: https://github.com/HEnquist/wasapi-rs

use std::{
    sync::{Arc, Mutex},
    thread,
};

use ringbuf::{
    HeapCons, HeapRb,
    traits::{Consumer, Observer as _, Split as _},
};
use windows::{
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Foundation::{HANDLE, WAIT_OBJECT_0},
        Media::Audio::{
            DEVICE_STATE_ACTIVE, EDataFlow, IMMDevice, IMMDeviceEnumerator, IMMEndpoint,
            IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator, eAll, eCapture,
            eConsole, eRender,
        },
        System::{
            Com::{
                CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, STGM_READ,
                StructuredStorage::PropVariantToStringAlloc,
            },
            Threading::{CreateEventW, SetEvent, WaitForMultipleObjects},
        },
    },
    core::implement,
};
use windows_core::{HSTRING, Interface as _, PWSTR};

use crate::audio::{
    AudioDevice, AudioLoopCommand, DEFAULT_RATE, DeviceRegistry, Notifier,
    playback::{AudioPacketInput, AudioPacketOutput, Playback, PlaybackController},
    windows::{capture::CaptureStream, playback::PlaybackStream},
};

pub mod capture;
pub mod playback;

pub(crate) fn try_get_device(
    enumerator: &IMMDeviceEnumerator,
    preffered_device: &Option<HSTRING>,
    expected_flow: EDataFlow,
) -> Option<IMMDevice> {
    let Some(preffered_device) = preffered_device else {
        return None;
    };

    unsafe {
        let device = enumerator
            .GetDevice(PWSTR(preffered_device.as_ptr() as *mut _))
            .ok()?;

        let endpoint: IMMEndpoint = device.cast().ok()?;
        let data_flow = endpoint.GetDataFlow().ok()?;

        if data_flow != expected_flow {
            return None;
        }

        Some(device)
    }
}

#[implement(IMMNotificationClient)]
struct DeviceNotifier {
    device_registry: DeviceRegistry,
}

impl DeviceNotifier {
    fn new(
        enumerator: &IMMDeviceEnumerator,
        registry: DeviceRegistry,
    ) -> windows::core::Result<Self> {
        unsafe {
            let default_capture = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole)?;
            let default_capture = default_capture.GetId()?.to_string()?;

            let default_render = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let default_render = default_render.GetId()?.to_string()?;

            let collection = enumerator.EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE)?;

            let count = collection.GetCount().unwrap();

            for i in 0..count {
                let device: IMMDevice = collection.Item(i)?;

                let endpoint: IMMEndpoint = device.cast()?;
                let data_flow = endpoint.GetDataFlow()?;

                let store = device.OpenPropertyStore(STGM_READ)?;
                let prop = store.GetValue(&PKEY_Device_FriendlyName)?;

                let id = device.GetId()?;
                let id = id.to_string()?;

                let display_name = PropVariantToStringAlloc(&prop)?;
                let display_name = display_name.to_string()?;

                if data_flow == eRender {
                    registry.add_output(AudioDevice {
                        is_active: id == default_render,
                        id,
                        display_name,
                    });
                } else if data_flow == eCapture {
                    registry.add_input(AudioDevice {
                        is_active: id == default_capture,
                        id,
                        display_name,
                    });
                }
            }
        }

        Ok(DeviceNotifier {
            device_registry: registry,
        })
    }
}

impl IMMNotificationClient_Impl for DeviceNotifier_Impl {
    fn OnDeviceAdded(&self, _pwstrdeviceid: &windows_core::PCWSTR) -> windows_core::Result<()> {
        Ok(())
    }

    fn OnDeviceRemoved(&self, _pwstrdeviceid: &windows_core::PCWSTR) -> windows_core::Result<()> {
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        _flow: windows::Win32::Media::Audio::EDataFlow,
        _role: windows::Win32::Media::Audio::ERole,
        _pwstrdefaultdeviceid: &windows_core::PCWSTR,
    ) -> windows_core::Result<()> {
        // NOTE: Should we change the device?

        Ok(())
    }

    fn OnDeviceStateChanged(
        &self,
        pwstrdeviceid: &windows_core::PCWSTR,
        dwnewstate: windows::Win32::Media::Audio::DEVICE_STATE,
    ) -> windows_core::Result<()> {
        // New device plugged
        if dwnewstate.0 == 1 {
            unsafe {
                let enumerator: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

                let device = enumerator.GetDevice(*pwstrdeviceid)?;

                let id = device.GetId()?;
                let id = id.to_string()?;

                let store = device.OpenPropertyStore(STGM_READ)?;
                let prop_variant = store.GetValue(&PKEY_Device_FriendlyName)?;

                let display_name = PropVariantToStringAlloc(&prop_variant)?;
                let display_name = display_name.to_string()?;

                let endpoint: IMMEndpoint = device.cast()?;

                let dataflow = endpoint.GetDataFlow()?;
                if dataflow == eCapture {
                    self.device_registry.add_input(AudioDevice {
                        id,
                        display_name,
                        is_active: false,
                    });
                } else if dataflow == eRender {
                    self.device_registry.add_output(AudioDevice {
                        id,
                        display_name,
                        is_active: false,
                    });
                }
            }
        }

        // Device is unplugged
        if dwnewstate.0 == 4 || dwnewstate.0 == 8 {
            unsafe {
                let enumerator: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

                let device = enumerator.GetDevice(*pwstrdeviceid)?;

                let id = device.GetId()?;
                let id = id.to_string()?;

                self.device_registry.remove_device(&id);
            }
        }

        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _pwstrdeviceid: &windows_core::PCWSTR,
        _key: &windows::Win32::Foundation::PROPERTYKEY,
    ) -> windows_core::Result<()> {
        // User might rename the device or change sampling rate
        // Fuck it for now, that's a late game stuff
        // TODO: Handle renaming and recreate streams if sampling rate is changed

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

struct ChannelState<T> {
    inner: Arc<Mutex<Vec<T>>>,
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
            inner: Arc::new(Mutex::new(Vec::with_capacity(4))),
        }
    }

    fn pop(&self) -> Option<T> {
        let mut state = self.inner.lock().unwrap();

        state.pop()
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
        let mut state = self.state.inner.lock().unwrap();
        state.push(msg);

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

pub(crate) fn init(
    packet_input: AudioPacketInput,
    packet_output: AudioPacketOutput,
) -> (WindowsCapture, Playback, DeviceRegistry) {
    // We capture in mono and there's no point to store
    // more than 60ms
    let ring = HeapRb::<f32>::new(((DEFAULT_RATE / 1000) * 60) as usize);
    let (capture_producer, capture_consumer) = ring.split();

    let (command_event, command_state, sender) = chnannel::<AudioLoopCommand>();

    let capture_notifier = Notifier::new();
    let capture = WindowsCapture {
        capture_consumer,
        loop_controller: sender.clone(),
        notifier: capture_notifier.clone(),
    };

    let playback = Playback {
        controller: PlaybackController::new(sender.clone()),
        packet_input: Some(packet_input),
    };

    let _device_registry = DeviceRegistry::new(sender);
    let device_registry = _device_registry.clone();

    _ = thread::Builder::new()
        .name("wasapi-loop".into())
        .spawn(move || unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .expect("Failed to init COM library");

            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .expect("Failed to create device enumerator");

            let notifier = DeviceNotifier::new(&enumerator, device_registry.clone())
                .expect("Failed to setup DeviceNotifier");
            let notification_client: IMMNotificationClient = notifier.into();

            enumerator
                .RegisterEndpointNotificationCallback(&notification_client)
                .unwrap();

            let mut preffered_capture_device: Option<String> = None;
            let mut preffered_playback_device: Option<String> = None;

            let mut capture_enabled = false;
            let mut playback_enabled = true;

            let mut capture_stream = CaptureStream::new(
                capture_producer,
                capture_notifier.clone(),
                preffered_capture_device.clone(),
            )
            .expect("Failed to init capture");
            let mut playback_stream =
                PlaybackStream::new(packet_output, preffered_playback_device.clone())
                    .expect("Failed to init playback");

            let command_event = command_event;

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

                        capture_stream = CaptureStream::new(
                            producer,
                            capture_notifier.clone(),
                            preffered_capture_device.clone(),
                        )
                        .expect("Failed to recreate the capture stream");

                        _ = capture_stream.set_enabled(capture_enabled);
                        device_registry.mark_active_input(&capture_stream.active_device);
                    }
                } else if wait_result.0 == WAIT_OBJECT_0.0 + 1 {
                    // Failure most likely means that device has changed
                    if playback_stream.process().is_err() {
                        let packet_output = playback_stream.packet_output.take().unwrap();

                        playback_stream =
                            PlaybackStream::new(packet_output, preffered_playback_device.clone())
                                .expect("Failed to recreate the playback stream");

                        _ = playback_stream.set_enabled(playback_enabled);
                        device_registry.mark_active_output(&playback_stream.active_device);
                    }
                } else if wait_result.0 == WAIT_OBJECT_0.0 + 2 {
                    while let Some(event) = command_state.pop() {
                        match event {
                            AudioLoopCommand::SetActiveInputDevice(device) => {
                                let producer = capture_stream.capture_producer.take().unwrap();
                                preffered_capture_device = Some(device.id.clone());

                                capture_stream = CaptureStream::new(
                                    producer,
                                    capture_notifier.clone(),
                                    preffered_capture_device.clone(),
                                )
                                .expect("Failed to recreate the capture stream");

                                _ = capture_stream.set_enabled(capture_enabled);
                                device_registry.mark_active_input(&capture_stream.active_device);
                            }
                            AudioLoopCommand::SetActiveOutputDevice(device) => {
                                let packet_output = playback_stream.packet_output.take().unwrap();
                                preffered_playback_device = Some(device.id.clone());

                                playback_stream = PlaybackStream::new(
                                    packet_output,
                                    preffered_playback_device.clone(),
                                )
                                .expect("Failed to recreate the playback stream");

                                _ = playback_stream.set_enabled(playback_enabled);
                                device_registry.mark_active_output(&playback_stream.active_device);
                            }
                            AudioLoopCommand::SetEnabledCapture(value) => {
                                capture_enabled = value;
                                _ = capture_stream.set_enabled(capture_enabled);
                            }
                            AudioLoopCommand::SetEnabledPlayback(value) => {
                                playback_enabled = value;
                                _ = playback_stream.set_enabled(playback_enabled);
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
