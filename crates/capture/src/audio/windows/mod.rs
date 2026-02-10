//! TODO: Migrate to safe WASAPI wrapper? Like this one: https://github.com/HEnquist/wasapi-rs

use std::{ptr, thread};

use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Producer, Split as _},
};
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
pub const DEFAULT_CHANNELS: u32 = 2;

pub mod capture;
pub mod playback;

struct CaptureStream {
    event_handle: HANDLE,

    capture_producer: HeapProd<f32>,

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

impl CaptureStream {
    fn new(capture_producer: HeapProd<f32>) -> windows::core::Result<Self> {
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
                capture_producer,
                audio_client,
                capture_client,
                format_ptr,
            })
        }
    }

    fn process(&mut self) -> windows::core::Result<()> {
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

                    self.capture_producer.push_slice(samples);
                }

                self.capture_client.ReleaseBuffer(num_frames_read)?;
                packet_length = self.capture_client.GetNextPacketSize()?;
            }
        }

        Ok(())
    }
}

struct PlaybackStream {
    event_handle: HANDLE,

    playback_consumer: HeapCons<f32>,

    audio_client: IAudioClient,
    render_client: IAudioRenderClient,

    format_ptr: *mut WAVEFORMATEX,
    buffer_frame_count: u32,
}

impl PlaybackStream {
    fn new(playback_consumer: HeapCons<f32>) -> windows::core::Result<Self> {
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

            let buffer_frame_count = audio_client.GetBufferSize()?;

            let render_client: IAudioRenderClient = audio_client.GetService()?;
            audio_client
                .Start()
                .expect("Failed to start audio capturing");

            Ok(Self {
                event_handle,
                playback_consumer,
                audio_client,
                render_client,
                buffer_frame_count,
                format_ptr,
            })
        }
    }

    fn process(&mut self) -> windows::core::Result<()> {
        unsafe {
            let format = *self.format_ptr;

            // Frames in the buffer
            let num_padding_frames = self.audio_client.GetCurrentPadding()?;
            let num_frames_available = (self.buffer_frame_count - num_padding_frames) as usize;

            if num_frames_available > 0 {
                let buffer_ptr =
                    self.render_client.GetBuffer(num_frames_available as u32)? as *mut f32;
                let buffer = std::slice::from_raw_parts_mut(
                    buffer_ptr,
                    num_frames_available * format.nChannels as usize,
                );

                // Testing loopback
                let mut i = 0;
                while let Some(sample) = self.playback_consumer.try_pop() {
                    if i + 1 >= buffer.len() {
                        break;
                    }

                    buffer[i] = sample;
                    buffer[i + 1] = sample;

                    i += 2;
                }

                self.render_client
                    .ReleaseBuffer(num_frames_available as u32, 0)?;
            }
        }

        Ok(())
    }
}

pub fn init() {
    // We capture in mono and there's no point to store
    // more than 60ms
    let ring = HeapRb::<f32>::new(((DEFAULT_RATE / 1000) * 60) as usize);
    let (capture_producer, capture_consumer) = ring.split();

    let ring = HeapRb::<f32>::new((DEFAULT_RATE * DEFAULT_CHANNELS) as usize);
    let (playback_producer, playback_consumer) = ring.split();

    _ = thread::Builder::new()
        .name("wasapi-loop".into())
        .spawn(move || unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .expect("Failed to init COM library");

            let mut capture = CaptureStream::new(capture_producer).expect("Failed to init capture");
            let mut playback =
                PlaybackStream::new(capture_consumer).expect("Failed to init playback");

            loop {
                let wait_result = WaitForMultipleObjects(
                    &[capture.event_handle, playback.event_handle],
                    false, // wake on any
                    2000,
                );

                // TODO: We need to recreate the streams on errors.
                // Usually it means the device has been unplugged
                if wait_result == WAIT_OBJECT_0 {
                    capture.process().unwrap();
                } else if wait_result.0 == WAIT_OBJECT_0.0 + 1 {
                    playback.process().unwrap();
                } else {
                    panic!("Timeout!");
                }
            }
        })
        .unwrap()
        .join();
}
