use ringbuf::{HeapCons, traits::Consumer};
use windows::Win32::{
    Foundation::HANDLE,
    Media::Audio::{
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient,
        IAudioRenderClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX,
        eConsole, eRender,
    },
    System::{
        Com::{CLSCTX_ALL, CoCreateInstance},
        Threading::CreateEventW,
    },
};
use windows_core::HSTRING;

use crate::audio::{DEFAULT_RATE, windows::try_get_device};

// TODO: Implement Drop
pub(crate) struct PlaybackStream {
    pub(crate) event_handle: HANDLE,
    pub(crate) playback_consumer: Option<HeapCons<f32>>,
    pub(crate) active_device: String,

    audio_client: IAudioClient,
    render_client: IAudioRenderClient,

    format_ptr: *mut WAVEFORMATEX,
    buffer_frame_count: u32,
}

fn try_activate_device(
    enumerator: &IMMDeviceEnumerator,
    preffered_device: &Option<HSTRING>,
) -> Option<(IMMDevice, IAudioClient)> {
    let Some(device) = try_get_device(enumerator, preffered_device, eRender) else {
        return None;
    };

    unsafe {
        let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None).ok()?;

        Some((device, audio_client))
    }
}

impl PlaybackStream {
    pub(crate) fn new(
        playback_consumer: HeapCons<f32>,
        preffered_device: Option<String>,
    ) -> windows::core::Result<Self> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

            let preffered_device = preffered_device.map(|value| HSTRING::from(value));
            let (device, audio_client) = match try_activate_device(&enumerator, &preffered_device) {
                Some(value) => value,
                None => {
                    let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
                    let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

                    (device, audio_client)
                }
            };

            let device_id = device.GetId()?.to_string()?;

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
                active_device: device_id,
                playback_consumer: Some(playback_consumer),
                audio_client,
                render_client,
                buffer_frame_count,
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

                if let Some(consumer) = self.playback_consumer.as_mut() {
                    for slot in buffer.iter_mut() {
                        if let Some(sample) = consumer.try_pop() {
                            *slot = sample;
                        } else {
                            *slot = 0.;
                        }
                    }
                }

                self.render_client
                    .ReleaseBuffer(num_frames_available as u32, 0)?;
            }
        }

        Ok(())
    }
}
