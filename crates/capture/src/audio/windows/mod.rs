//! TODO: Migrate to safe WASAPI wrapper? Like this one: https://github.com/HEnquist/wasapi-rs

use std::ptr;

use windows::Win32::{
    Foundation::{HANDLE, WAIT_OBJECT_0},
    Media::Audio::{
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
        IAudioCaptureClient, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator,
        MMDeviceEnumerator, WAVEFORMATEX, eCapture, eConsole, eRender,
    },
    System::{
        Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree},
        Threading::{CreateEventW, WaitForMultipleObjects, WaitForSingleObject},
    },
};

pub const DEFAULT_RATE: u32 = 48000;

pub mod capture;
pub mod playback;

struct WindowsCapture {
    event_handle: HANDLE,

    audio_client: IAudioClient,
    capture_client: IAudioCaptureClient,

    format_ptr: *mut WAVEFORMATEX,
}

impl Drop for WindowsCapture {
    fn drop(&mut self) {
        unsafe {
            _ = self.audio_client.Stop();

            CoTaskMemFree(Some(self.format_ptr as *const _));
        }
    }
}

impl WindowsCapture {
    fn new() -> windows::core::Result<Self> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

            let device = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole)?;
            let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

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
            audio_client
                .Start()
                .expect("Failed to start audio capturing");

            Ok(Self {
                event_handle,
                audio_client,
                capture_client,
                format_ptr,
            })
        }
    }

    fn process(&self) -> windows::core::Result<()> {
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

                    let mut sum = 0.0;
                    for &sample in samples {
                        sum += sample * sample;
                    }
                    let rms = (sum / total_samples as f32).sqrt();

                    let bars = (rms * 50.0) as usize;
                    println!("Raw Input: [{}]", "#".repeat(bars));
                } else {
                    println!("Silent...");
                }

                self.capture_client.ReleaseBuffer(num_frames_read)?;
                packet_length = self.capture_client.GetNextPacketSize()?;
            }
        }

        Ok(())
    }
}

struct WindowsPlayback {
    event_handle: HANDLE,

    audio_client: IAudioClient,
    render_client: IAudioRenderClient,

    format_ptr: *mut WAVEFORMATEX,
}

impl WindowsPlayback {
    fn new() -> windows::core::Result<Self> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

            let format_ptr: *mut WAVEFORMATEX = audio_client.GetMixFormat()?;
            let format = &mut *format_ptr;

            format.nChannels = 2; // We always want stereo for playback
            format.nSamplesPerSec = DEFAULT_RATE;
            // TODO: Assuming f32 for now. Make it more robust
            format.nAvgBytesPerSec = DEFAULT_RATE * 4 * format.nChannels as u32;
            format.nBlockAlign = 4 * format.nChannels;

            let event_handle = CreateEventW(None, false, false, None)?;

            // Ask for 40ms (units are 100ns)
            let req_buffer_duration = 400_000;
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

            let render_client: IAudioRenderClient = audio_client.GetService()?;
            audio_client
                .Start()
                .expect("Failed to start audio capturing");

            Ok(Self {
                event_handle,
                audio_client,
                render_client,
                format_ptr,
            })
        }
    }

    fn process(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

pub fn init() -> windows::core::Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;

        let capture = WindowsCapture::new()?;
        let playback = WindowsPlayback::new()?;

        loop {
            let wait_result = WaitForMultipleObjects(
                &[capture.event_handle, playback.event_handle],
                false, // wake on any
                2000,
            );

            if wait_result == WAIT_OBJECT_0 {
                capture.process().unwrap();
            } else if wait_result.0 == WAIT_OBJECT_0.0 + 1 {
                playback.process().unwrap();
            } else {
                panic!("Timeout!");
            }
        }
    }
}
