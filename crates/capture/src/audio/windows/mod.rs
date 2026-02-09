use std::{
    ptr::{self, read_unaligned},
    thread,
    time::Duration,
};

use windows::Win32::{
    Foundation::WAIT_OBJECT_0,
    Media::Audio::{
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
        IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX,
        eCapture, eConsole,
    },
    System::{
        Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx},
        Threading::{CreateEventA, WaitForSingleObject},
    },
};

pub const DEFAULT_RATE: u32 = 48000;

struct WindowsCapture {}

pub fn init() -> windows::core::Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

        let device = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole)?;
        let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

        let format_ptr: *mut WAVEFORMATEX = audio_client.GetMixFormat()?;
        let format = &mut *format_ptr;

        format.nChannels = 1;
        format.nSamplesPerSec = DEFAULT_RATE;
        // Asuming f32, idk if that's a safe assumption
        format.nAvgBytesPerSec = DEFAULT_RATE * 4 * format.nChannels as u32;
        format.nBlockAlign = 4 * format.nChannels;

        let audio_event_handle = CreateEventA(None, false, false, None)?;

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
        audio_client.SetEventHandle(audio_event_handle)?;

        let capture_client: IAudioCaptureClient = audio_client.GetService()?;
        audio_client
            .Start()
            .expect("Failed to start audio capturing");

        loop {
            // returns the number of **frames** available (oh my dear frames..)
            let mut packet_length = capture_client.GetNextPacketSize()?;

            let wait_result = WaitForSingleObject(audio_event_handle, 2000);
            if wait_result != WAIT_OBJECT_0 {
                println!("Timeout or Error waiting for audio buffer.");
                break;
            }

            while packet_length != 0 {
                let mut buffer_ptr: *mut u8 = ptr::null_mut();
                let mut num_frames_read = 0;
                let mut flags = 0;

                capture_client.GetBuffer(
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

                    // Visualizer
                    let bars = (rms * 50.0) as usize;
                    // println!("Raw Input: [{}]", "#".repeat(bars));
                } else {
                    println!("Silent...");
                }

                capture_client.ReleaseBuffer(num_frames_read)?;
                packet_length = capture_client.GetNextPacketSize()?;
            }
        }

        Ok(())
    }
}
