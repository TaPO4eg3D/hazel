use std::ptr;

use ringbuf::{HeapProd, traits::Producer};
use windows::Win32::{
    Foundation::HANDLE,
    Media::Audio::{
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, EDataFlow,
        IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator, IMMEndpoint,
        MMDeviceEnumerator, WAVEFORMATEX, eCapture, eConsole,
    },
    System::{
        Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree},
        Threading::CreateEventW,
    },
};
use windows_core::{HSTRING, Interface, PWSTR};

use crate::audio::{DEFAULT_RATE, Notifier, windows::try_get_device};

pub(crate) struct CaptureStream {
    pub(crate) event_handle: HANDLE,
    pub(crate) capture_producer: Option<HeapProd<f32>>,
    pub(crate) active_device: String,

    capture_notifier: Notifier,

    audio_client: IAudioClient,
    capture_client: IAudioCaptureClient,

    format_ptr: *mut WAVEFORMATEX,
}

impl Drop for CaptureStream {
    fn drop(&mut self) {
        unsafe {
            _ = self.audio_client.Stop();

            CoTaskMemFree(Some(self.format_ptr as *const _));
        }
    }
}

fn try_activate_device(
    enumerator: &IMMDeviceEnumerator,
    preffered_device: &Option<HSTRING>,
) -> Option<(IMMDevice, IAudioClient)> {
    let Some(device) = try_get_device(enumerator, preffered_device, eCapture) else {
        return None;
    };

    unsafe {
        let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None).ok()?;

        Some((device, audio_client))
    }
}

impl CaptureStream {
    pub(crate) fn new(
        capture_producer: HeapProd<f32>,
        capture_notifier: Notifier,
        preffered_device: Option<String>,
    ) -> windows::core::Result<Self> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

            let preffered_device = preffered_device.map(|value| HSTRING::from(value));
            let (device, audio_client) = match try_activate_device(&enumerator, &preffered_device) {
                Some(value) => value,
                None => {
                    let device = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole)?;
                    let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

                    (device, audio_client)
                }
            };

            let device_id = device.GetId()?.to_string()?;

            let format_ptr: *mut WAVEFORMATEX = audio_client.GetMixFormat()?;
            let format = &mut *format_ptr;

            format.nChannels = 1; // We always want mono for capture
            format.nSamplesPerSec = DEFAULT_RATE;
            // TODO: Assuming f32 for now. Make it more robust
            format.nAvgBytesPerSec = DEFAULT_RATE * 4 * format.nChannels as u32;
            format.nBlockAlign = 4 * format.nChannels;

            let event_handle = CreateEventW(None, false, false, None)?;

            // Ask for 20ms (units are 100ns)
            let req_buffer_duration = 200_000;
            audio_client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                // Ask Windows to resample if needed
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
                    | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                req_buffer_duration,
                0,
                format_ptr,
                None,
            )?;
            audio_client.SetEventHandle(event_handle)?;

            let capture_client: IAudioCaptureClient = audio_client.GetService()?;

            windows::core::Result::Ok(Self {
                event_handle,
                active_device: device_id,
                capture_producer: Some(capture_producer),
                audio_client,
                capture_client,
                capture_notifier,
                format_ptr,
            })
        }
    }

    pub(crate) fn set_enabled(&mut self, value: bool) -> windows::core::Result<()> {
        if value {
            unsafe {
                self.audio_client.Start()?;
            }
        } else {
            unsafe {
                self.audio_client.Stop()?;
            }
        }

        Ok(())
    }

    pub(crate) fn process(&mut self) -> windows::core::Result<()> {
        unsafe {
            let format = *self.format_ptr;
            let mut packet_length = self.capture_client.GetNextPacketSize()?;

            while packet_length != 0 {
                let mut buffer_ptr: *mut u8 = ptr::null_mut();
                let mut num_frames_read = 0;
                let mut flags = 0;

                self.capture_client.GetBuffer(
                    &mut buffer_ptr,
                    &mut num_frames_read,
                    &mut flags,
                    None,
                    None,
                )?;

                // If the pointer is valid (not silent/glitch)
                if flags == 0 {
                    let total_samples = (num_frames_read as usize) * (format.nChannels as usize);

                    let samples =
                        std::slice::from_raw_parts(buffer_ptr as *const f32, total_samples);

                    if let Some(producer) = self.capture_producer.as_mut() {
                        producer.push_slice(samples);

                        self.capture_notifier.notify();
                    }
                }

                self.capture_client.ReleaseBuffer(num_frames_read)?;
                packet_length = self.capture_client.GetNextPacketSize()?;
            }
        }

        Ok(())
    }
}
