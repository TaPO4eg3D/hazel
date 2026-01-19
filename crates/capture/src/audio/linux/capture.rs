use std::{collections::VecDeque, sync::{Arc, atomic::AtomicBool}};

use anyhow::Result as AResult;
use libspa::param::{
    ParamType,
    audio::{AudioFormat, AudioInfoRaw},
    format::{MediaSubtype, MediaType},
    format_utils,
};
use nnnoiseless::DenoiseState;
use pipewire::{
    self as pw, main_loop::MainLoopRc, properties::properties, spa::{self, pod::Pod}, stream::{Stream, StreamBox, StreamListener}
};
use ringbuf::{HeapProd, traits::Producer};

use crate::audio::DEFAULT_RATE;

struct RnnoiseState {
    enable_noise_reduction: bool,
    denoise_state: Box<DenoiseState<'static>>,

    rnnoise_queue: VecDeque<f32>,

    rnnoise_in_buff: Vec<f32>,
    rnnoise_out_buff: Vec<f32>,
}

enum Denoiser {
    Rnnoise(RnnoiseState)
}

/// This data is shared across all Pipewire events
struct CaptureStreamData {
    format: AudioInfoRaw,

    /// Producer of captured samples
    samples_producer: HeapProd<f32>,

}

pub(crate) struct CaptureStream {
    pub stream: pw::stream::StreamRc,
    stream_listener: StreamListener<CaptureStreamData>,
}

impl CaptureStream {
    const STREAM_NAME: &'static str = "HAZEL Audio Capture";

    fn on_param_change(
        _stream: &Stream,
        user_data: &mut CaptureStreamData,
        id: u32,
        param: Option<&libspa::pod::Pod>,
    ) {
        let Some(param) = param else { return };
        if id != ParamType::Format.as_raw() {
            return;
        }

        let (media_type, media_subtype) = match format_utils::parse_format(param) {
            Ok(v) => v,
            Err(_) => return,
        };

        if media_type != MediaType::Audio || media_subtype != MediaSubtype::Raw {
            return;
        }

        let _ = user_data.format.parse(param);
    }

    fn on_process(stream: &Stream, this: &mut CaptureStreamData) {
        let Some(mut buffer) = stream.dequeue_buffer() else {
            return;
        };

        let datas = buffer.datas_mut();
        if datas.is_empty() {
            return;
        }

        let data = &mut datas[0];

        let chunk = data.chunk();
        let size = chunk.size() as usize;
        let offset = chunk.offset() as usize;

        if size == 0 {
            return;
        }

        if let Some(slice) = data.data()
            && offset + size <= slice.len()
        {
            let valid_bytes = &slice[offset..offset + size];

            let captured_samples = unsafe {
                std::slice::from_raw_parts(
                    valid_bytes.as_ptr() as *const f32,
                    valid_bytes.len() / size_of::<f32>(),
                )
            };

            captured_samples
                .iter()
                .for_each(|&s| {
                    _ = this.samples_producer.try_push(s)
                });

            // Encode everything we've captured
            // if this.enable_noise_reduction {
            //     this.rnnoise_queue.extend(captured_samples);
            //
            //     while this
            //         .rnnoise_queue
            //         .pop_slice(&mut this.rnnoise_in_buff, false)
            //         > 0
            //     {
            //         // As described in the `process_frame` documentation
            //         for sample in this.rnnoise_in_buff.iter_mut() {
            //             *sample = (32767.5 * (*sample) - 0.5).round();
            //         }
            //
            //         this.denoise_state
            //             .process_frame(&mut this.rnnoise_out_buff, &this.rnnoise_in_buff);
            //
            //         for sample in this.rnnoise_out_buff.iter_mut() {
            //             *sample = ((*sample) + 0.5) / 32767.5;
            //         }
            //
            //         this.encoder.encode(&this.rnnoise_out_buff);
            //     }
            // } else {
            //     this.encoder.encode(captured_samples);
            // }
            //
            // while let Some(packet) = this.encoder.packet_buff.pop_front() {
            //     _ = this.packet_producer.send(packet);
            // }
        }
    }

    pub(crate) fn new(
        core: pw::core::CoreRc,
        samples_producer: HeapProd<f32>,
    ) -> AResult<Self> {
        let capture_stream = pw::stream::StreamRc::new(
            core,
            Self::STREAM_NAME,
            properties! {
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_ROLE => "Communication",
                *pw::keys::MEDIA_CATEGORY => "Capture",
            },
        )?;

        let mut audio_info = spa::param::audio::AudioInfoRaw::new();
        audio_info.set_format(AudioFormat::F32LE);
        audio_info.set_rate(DEFAULT_RATE);

        // Microphone capture is most likely already in MONO,
        // but we enforce it just to be sure
        audio_info.set_channels(1);
        let mut position = [0; spa::param::audio::MAX_CHANNELS];
        position[0] = libspa::sys::SPA_AUDIO_CHANNEL_MONO;
        audio_info.set_position(position);

        let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pw::spa::pod::Value::Object(pw::spa::pod::Object {
                type_: libspa::sys::SPA_TYPE_OBJECT_Format,
                id: libspa::sys::SPA_PARAM_EnumFormat,
                properties: audio_info.into(),
            }),
        )
        .unwrap()
        .0
        .into_inner();

        let mut params = [Pod::from_bytes(&values).unwrap()];

        let stream_data = CaptureStreamData {
            format: Default::default(),
            samples_producer,
        };

        let listener = capture_stream
            .add_local_listener_with_user_data(stream_data)
            .process(CaptureStream::on_process)
            .param_changed(CaptureStream::on_param_change)
            .register()?;

        // Disabled by default
        capture_stream.set_active(false);
        capture_stream.connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )?;

        Ok(Self {
            stream: capture_stream,
            stream_listener: listener,
        })
    }
}
