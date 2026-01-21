use std::time::{Duration, Instant};

use anyhow::Result as AResult;
use pipewire::{
    self as pw,
    core::CoreRc,
    properties::properties,
    spa::{self, pod::Pod},
    stream::{Stream, StreamListener, StreamRc},
};
use ringbuf::{
    HeapCons,
    traits::{Consumer},
};

use crate::audio::{DEFAULT_CHANNELS, DEFAULT_RATE};

struct PlaybackStreamData {
    last: Instant,
    samples_consumer: HeapCons<f32>,
}

pub(crate) struct PlaybackStream {
    pub(crate) stream: StreamRc,

    _stream_listener: StreamListener<PlaybackStreamData>,
}

impl PlaybackStream {
    const STREAM_NAME: &'static str = "HAZEL Audio Playback";

    fn on_process(stream: &Stream, this: &mut PlaybackStreamData) {
        let Some(mut buffer) = stream.dequeue_buffer() else {
            return;
        };

        let datas = buffer.datas_mut();
        if datas.is_empty() {
            return;
        }
        let data = &mut datas[0];

        let stride = std::mem::size_of::<f32>() * DEFAULT_CHANNELS as usize;

        if let Some(slice) = data.data() {
            let output_samples = unsafe {
                std::slice::from_raw_parts_mut(slice.as_mut_ptr() as *mut f32, slice.len() / 4)
            };
            let chunk = data.chunk_mut();

            // TODO: It should not be here (and not like that), move
            if this.last.elapsed() > Duration::from_millis(120) {
                while this.samples_consumer.pop_slice(output_samples) > 0 {}

                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = stride as i32;
                *chunk.size_mut() = 4;

                this.last = Instant::now();

                return;
            }
            this.last = Instant::now();

            let written_frames = this.samples_consumer.pop_slice(output_samples);

            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = stride as i32;
            *chunk.size_mut() = (output_samples.len() * 4) as u32;

            if written_frames > 0 {
                *chunk.size_mut() = (written_frames * 4) as u32;
            } else {
                output_samples[0] = 0.0;
                *chunk.size_mut() = 4;
            }

        }
    }

    pub(crate) fn new(core: CoreRc, samples_consumer: HeapCons<f32>) -> AResult<Self> {
        let playback_stream = StreamRc::new(
            core,
            Self::STREAM_NAME,
            properties! {
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_ROLE => "Communication",
                *pw::keys::MEDIA_CATEGORY => "Playback",
                *pw::keys::AUDIO_CHANNELS => "2",
                *pw::keys::NODE_LATENCY => "1/48000",
            },
        )?;

        let mut audio_info = spa::param::audio::AudioInfoRaw::new();

        audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
        audio_info.set_rate(DEFAULT_RATE);
        audio_info.set_channels(DEFAULT_CHANNELS);

        let mut position = [0; spa::param::audio::MAX_CHANNELS];
        position[0] = libspa::sys::SPA_AUDIO_CHANNEL_FL;
        position[1] = libspa::sys::SPA_AUDIO_CHANNEL_FR;
        audio_info.set_position(position);

        let user_data = PlaybackStreamData {
            last: Instant::now(),
            samples_consumer,
        };

        let listener = playback_stream
            .add_local_listener_with_user_data(user_data)
            .process(Self::on_process)
            .register()?;

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

        playback_stream.connect(
            spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )?;

        Ok(PlaybackStream {
            stream: playback_stream,

            _stream_listener: listener,
        })
    }
}
