use anyhow::Result as AResult;
use libspa::param::{
    ParamType,
    audio::{AudioFormat, AudioInfoRaw},
    format::{MediaSubtype, MediaType},
    format_utils,
};
use pipewire::{
    self as pw,
    properties::properties,
    spa::{self, pod::Pod},
    stream::{Stream, StreamBox, StreamListener},
};
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::*};

use ffmpeg::codec::encoder;
use ffmpeg::{ChannelLayout, format};
use ffmpeg_next::{self as ffmpeg, codec};

pub const DEFAULT_RATE: u32 = 48000;
pub const DEFAULT_CHANNELS: u32 = 2;

struct CaptureStreamData {
    format: AudioInfoRaw,
    enable_loopback: bool,

    /// Ring Buffer that is used to loopback captured audio.
    /// Mainly used to quickly test how your microphone sounds
    /// TODO: Change RingBuffer type since both Capture and Playback live
    /// in the same thread
    loopback_producer: HeapProd<f32>,
}

struct CaptureStream<'a> {
    stream: StreamBox<'a>,
    stream_listener: StreamListener<CaptureStreamData>,
}

fn create_decoder() {
    let codec = encoder::find(ffmpeg::codec::Id::OPUS).expect("Opus codec not found");
    let context = ffmpeg::codec::context::Context::new_with_codec(codec);

    let codec = codec.audio().unwrap();

    let mut decoder = context.decoder().audio().unwrap();
    decoder.set_channel_layout(ChannelLayout::STEREO);
}

struct AudioDecoder {
    // codec: codec::audio::Audio,
    // decoder: codec::decoder::Opened,
}

impl AudioDecoder {
    pub fn new() -> Self {
        let codec = encoder::find(ffmpeg::codec::Id::OPUS).expect("Opus codec not found");
        let context = ffmpeg::codec::context::Context::new_with_codec(codec);

        let codec = codec.audio().unwrap();

        let decoder = context.decoder().audio().unwrap();
        // decoder.set_channel_layout(ChannelLayout::STEREO);

        let decoder = {
            decoder.open_as(codec).unwrap()
        };

        Self {
            // codec,
            // decoder,
        }
    }
}

struct AudioEncoder {
    codec: codec::audio::Audio,
    encoder: encoder::audio::Encoder,
}

impl AudioEncoder {
    pub fn new() -> Self {
        let codec = encoder::find(ffmpeg::codec::Id::OPUS).expect("Opus codec not found");
        let context = ffmpeg::codec::context::Context::new_with_codec(codec);

        let codec = codec.audio().unwrap();

        let mut encoder = context.encoder().audio().unwrap();

        for format in codec.formats().unwrap() {
            println!("Found format: {format:?}");
        }

        for rate in codec.rates().unwrap() {
            println!("Found rates: {rate:?}");
        }

        encoder.set_rate(DEFAULT_RATE as i32);
        encoder.set_channel_layout(ChannelLayout::STEREO);
        encoder.set_format(format::Sample::F32(format::sample::Type::Packed));

        let encoder = encoder.open_as(codec).unwrap();

        Self { codec, encoder }
    }
}

impl<'a> CaptureStream<'a> {
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
        println!("Capture format changed: {:?}", user_data.format);
    }

    fn on_process(stream: &Stream, user_data: &mut CaptureStreamData) {
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

            let samples_f32 = unsafe {
                std::slice::from_raw_parts(
                    valid_bytes.as_ptr() as *const f32,
                    valid_bytes.len() / size_of::<f32>(),
                )
            };

            if user_data.enable_loopback {
                user_data.loopback_producer.push_slice(samples_f32);
            }
        }
    }

    fn new(core: &'a pw::core::CoreRc, loopback_producer: HeapProd<f32>) -> AResult<Self> {
        let capture_stream = pw::stream::StreamBox::new(
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
        audio_info.set_channels(DEFAULT_CHANNELS);

        let mut position = [0; spa::param::audio::MAX_CHANNELS];
        position[0] = libspa::sys::SPA_AUDIO_CHANNEL_FL;
        position[1] = libspa::sys::SPA_AUDIO_CHANNEL_FR;
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
            loopback_producer,
            enable_loopback: false,
        };

        let listener = capture_stream
            .add_local_listener_with_user_data(stream_data)
            .process(CaptureStream::on_process)
            .param_changed(CaptureStream::on_param_change)
            .register()?;

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

struct PlaybackStreamData {
    /// Ring Buffer that is used to loopback captured audio.
    /// Mainly used to quickly test how your microphone sounds
    loopback_consumer: HeapCons<f32>,
}

struct PlaybackStream<'a> {
    stream: StreamBox<'a>,
    stream_listener: StreamListener<PlaybackStreamData>,
}

impl<'a> PlaybackStream<'a> {
    const STREAM_NAME: &'static str = "HAZEL Audio Playback";

    fn on_process(stream: &Stream, user_data: &mut PlaybackStreamData) {
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

            let read_count = user_data.loopback_consumer.pop_slice(output_samples);

            // Fill remaining buffer with silence (zeros) if we ran out of data
            if read_count < output_samples.len() {
                (read_count..output_samples.len()).for_each(|i| {
                    output_samples[i] = 0.0;
                });
            }

            let chunk = data.chunk_mut();

            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = stride as i32;
            *chunk.size_mut() = (output_samples.len() * 4) as u32;
        }
    }

    fn new(core: &'a pw::core::CoreRc, loopback_consumer: HeapCons<f32>) -> AResult<Self> {
        let playback_stream = pw::stream::StreamBox::new(
            core,
            Self::STREAM_NAME,
            properties! {
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_ROLE => "Communication",
                *pw::keys::MEDIA_CATEGORY => "Playback",
                *pw::keys::AUDIO_CHANNELS => "2",
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

        let user_data = PlaybackStreamData { loopback_consumer };

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
            stream_listener: listener,
        })
    }
}

pub struct Audio {}

impl Audio {
    pub fn new() -> AResult<Self> {
        pw::init();
        ffmpeg::init().unwrap();

        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;

        let ring = HeapRb::<f32>::new((DEFAULT_RATE * 2) as usize);
        let (loopback_producer, loopback_consumer) = ring.split();

        let encoder = AudioEncoder::new();

        let playback = PlaybackStream::new(&core, loopback_consumer)?;
        let capture = CaptureStream::new(&core, loopback_producer)?;

        mainloop.run();

        Ok(Audio {})
    }
}
