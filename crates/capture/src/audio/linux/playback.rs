use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::Result as AResult;
use pipewire::{
    self as pw,
    core::CoreRc,
    properties::properties,
    spa::{self, pod::Pod},
    stream::{Stream, StreamListener, StreamRc},
};
use ringbuf::{traits::Consumer, HeapCons};

use crate::audio::{PlaybackSchedulerRecv, VecDequeExt as _, DEFAULT_CHANNELS, DEFAULT_RATE};

struct PlaybackStreamData {
    last: Instant,

    scheduler: PlaybackSchedulerRecv,
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

        let requested = buffer.requested() as usize;
        let datas = buffer.datas_mut();

        if datas.is_empty() {
            return;
        }
        let data = &mut datas[0];

        let stride = std::mem::size_of::<f32>() * DEFAULT_CHANNELS as usize;
        if let Some(slice) = data.data() {
            let n_frames = slice.len() / stride;
            let n_frames = n_frames.min(requested);

            let n_samples = n_frames * DEFAULT_CHANNELS as usize;

            let output_samples = unsafe {
                std::slice::from_raw_parts_mut(slice.as_mut_ptr() as *mut f32, n_samples)
            };

            let chunk = data.chunk_mut();
            this.scheduler.pop_slice(output_samples);

            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = stride as i32;
            *chunk.size_mut() = (n_samples * std::mem::size_of::<f32>()) as u32;
        }
    }

    pub(crate) fn new(core: CoreRc, scheduler: PlaybackSchedulerRecv) -> AResult<Self> {
        let playback_stream = StreamRc::new(
            core,
            Self::STREAM_NAME,
            properties! {
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_ROLE => "Communication",
                *pw::keys::MEDIA_CATEGORY => "Playback",
                *pw::keys::AUDIO_CHANNELS => "2",
                *pw::keys::NODE_LATENCY => "512/48000",
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
            scheduler,
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
