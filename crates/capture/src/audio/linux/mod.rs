use libspa::param::{
    ParamType,
    audio::{AudioFormat, AudioInfoRaw},
    format::{MediaSubtype, MediaType},
    format_utils,
};
use pipewire::{
    self as pw,
    stream::{Stream, StreamBox, StreamListener},
};
use pw::{properties::properties, spa};
use spa::pod::Pod;

use anyhow::Result as AResult;

pub const DEFAULT_RATE: u32 = 44100;
pub const DEFAULT_CHANNELS: u32 = 2;
pub const DEFAULT_VOLUME: f64 = 0.3;
pub const PI_2: f64 = std::f64::consts::PI * 2.;
pub const CHAN_SIZE: usize = std::mem::size_of::<i16>();

pub struct Audio {}

/// Shared data between events
struct CaptureStreamData {
    format: AudioInfoRaw,
}

struct CaptureStream<'a> {
    stream: StreamBox<'a>,
    stream_listener: StreamListener<CaptureStreamData>,
}

impl<'a> CaptureStream<'a> {
    const STREAM_NAME: &'static str = "HAZEL Audio Capture";

    /// Gets called when the stream param changes.
    /// We're only looking for format changes
    fn on_param_change(
        _stream: &Stream,
        user_data: &mut CaptureStreamData,
        id: u32,
        param: Option<&libspa::pod::Pod>,
    ) {
        // NULL means to clear the format
        let Some(param) = param else {
            return;
        };

        if param.is_none() || id != ParamType::Format.as_raw() {
            return;
        }

        let (media_type, media_subtype) = match format_utils::parse_format(param) {
            Ok(v) => v,
            Err(_) => return,
        };

        // Only accept raw audio
        if media_type != MediaType::Audio || media_subtype != MediaSubtype::Raw {
            return;
        }

        user_data
            .format
            .parse(param)
            .expect("Failed to parse param changed to AudioInfoRaw");

        println!(
            "capturing rate:{} channels:{}",
            user_data.format.rate(),
            user_data.format.channels()
        );
    }

    /// Gets called when we can take the data
    fn on_process(stream: &Stream, user_data: &mut CaptureStreamData) {
        let buffer = stream.dequeue_buffer();

        let Some(mut buffer) = buffer else {
            println!("Out of buffers to capture");

            return;
        };

        let datas = buffer.datas_mut();
        if datas.is_empty() {
            return;
        }

        let data = &mut datas[0];

        let n_channels = user_data.format.channels();
        let n_samples = data.chunk().size() / (std::mem::size_of::<f32>() as u32);

        let Some(samples) = data.data() else {
            return;
        };

        for c in 0..n_channels {
            let mut max: f32 = 0.0;

            for n in (c..n_samples).step_by(n_channels as usize) {
                let start = n as usize * std::mem::size_of::<f32>();
                let end = start + std::mem::size_of::<f32>();

                let chan = &samples[start..end];
                let f = f32::from_le_bytes(chan.try_into().unwrap());

                max = max.max(f.abs());
            }

            let peak = ((max * 30.0) as usize).clamp(0, 39);
        }
    }

    fn new(core: &'a pw::core::CoreRc) -> AResult<Self> {
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

impl Audio {
    const PLAYBACK_STREAM_NAME: &'static str = "HAZEL Audio Playback";

    pub fn new() -> AResult<Self> {
        pw::init();

        // TODO: MainLoop should be created separately and be shared
        // between audio and video capture modules
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;

        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;

        let playback_stream = pw::stream::StreamBox::new(
            &core,
            Self::PLAYBACK_STREAM_NAME,
            properties! {
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_ROLE => "Communication",
                *pw::keys::MEDIA_CATEGORY => "Playback",
                *pw::keys::AUDIO_CHANNELS => "2",
            },
        )?;

        let mut audio_info = spa::param::audio::AudioInfoRaw::new();
        audio_info.set_format(spa::param::audio::AudioFormat::S16LE);
        audio_info.set_rate(DEFAULT_RATE);
        audio_info.set_channels(DEFAULT_CHANNELS);

        let mut position = [0; spa::param::audio::MAX_CHANNELS];
        position[0] = libspa::sys::SPA_AUDIO_CHANNEL_FL;
        position[1] = libspa::sys::SPA_AUDIO_CHANNEL_FR;
        audio_info.set_position(position);

        let data: f64 = 0.0;
        let _listener = playback_stream
            .add_local_listener_with_user_data(data)
            .process(|stream, acc| match stream.dequeue_buffer() {
                None => println!("No buffer received"),
                Some(mut buffer) => {
                    let datas = buffer.datas_mut();
                    let stride = CHAN_SIZE * DEFAULT_CHANNELS as usize;
                    let data = &mut datas[0];
                    let n_frames = if let Some(slice) = data.data() {
                        let n_frames = slice.len() / stride;
                        for i in 0..n_frames {
                            *acc += PI_2 * 440.0 / DEFAULT_RATE as f64;
                            if *acc >= PI_2 {
                                *acc -= PI_2
                            }
                            let val = (f64::sin(*acc) * DEFAULT_VOLUME * 16767.0) as i16;
                            for c in 0..DEFAULT_CHANNELS {
                                let start = i * stride + (c as usize * CHAN_SIZE);
                                let end = start + CHAN_SIZE;
                                let chan = &mut slice[start..end];
                                chan.copy_from_slice(&i16::to_le_bytes(val));
                            }
                        }
                        n_frames
                    } else {
                        0
                    };
                    let chunk = data.chunk_mut();
                    *chunk.offset_mut() = 0;
                    *chunk.stride_mut() = stride as _;
                    *chunk.size_mut() = (stride * n_frames) as _;
                }
            })
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

        let capture_stream = CaptureStream::new(&core)?;

        mainloop.run();

        Ok(Audio {})
    }
}
