use pipewire as pw;
use pw::{properties::properties, spa};
use spa::pod::Pod;

use anyhow::Result as AResult;

pub const DEFAULT_RATE: u32 = 44100;
pub const DEFAULT_CHANNELS: u32 = 2;
pub const DEFAULT_VOLUME: f64 = 0.3;
pub const PI_2: f64 = std::f64::consts::PI * 2.;
pub const CHAN_SIZE: usize = std::mem::size_of::<i16>();

pub struct Audio {}

impl Audio {
    const AUDIO_STREAM_NAME: &'static str = "HAZEL";

    pub fn new() -> AResult<Self> {
        pw::init();

        // TODO: MainLoop should be created separately and be shared
        // between audio and video capture modules
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;

        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;

        let stream = pw::stream::StreamBox::new(
            &core,
            Self::AUDIO_STREAM_NAME,
            properties! {
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_ROLE => "Music",
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
        let _listener = stream
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

        stream.connect(
            spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )?;

        mainloop.run();

        Ok(Audio {})
    }
}
