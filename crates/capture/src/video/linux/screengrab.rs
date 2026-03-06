use std::os::fd::OwnedFd;

use anyhow::Result as AResult;
use ashpd::{
    desktop::{
        PersistMode,
        screencast::{
            CursorMode, Screencast, SelectSourcesOptions, SourceType, Stream as ASHPDStream,
        },
    },
    enumflags2::BitFlags,
};
use drm_fourcc::DrmModifier;
use ffmpeg_next::format::open;

use libspa::{
    param::format::FormatProperties,
    pod::{ChoiceValue, Pod, PropertyFlags},
    utils::ChoiceFlags,
};
use libva::{Display, VAEntrypoint, VAProfile};
use pipewire::{
    self as pw,
    core::CoreRc,
    properties::properties,
    stream::{Stream, StreamListener, StreamRc},
};

use crate::video::encode::VideoEncoder;

async fn open_portal() -> ashpd::Result<(u32, OwnedFd)> {
    let proxy = Screencast::new().await?;
    let session = proxy.create_session(Default::default()).await?;

    let mut sources = BitFlags::empty();
    sources.insert(SourceType::Monitor);

    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Embedded)
                .set_sources(Some(sources))
                .set_multiple(false) // NOTE: Potential experimentation with streaming multiple sources
                .set_persist_mode(PersistMode::DoNot),
        )
        .await?;

    let response = proxy
        .start(&session, None, Default::default())
        .await?
        .response()?;

    let stream = response.streams().first().unwrap();

    let fd = proxy
        .open_pipe_wire_remote(&session, Default::default())
        .await?;

    Ok((stream.pipe_wire_node_id(), fd))
}

#[derive(Default)]
struct ScreencastStreamData {
    encoder: Option<VideoEncoder>,
    format: pw::spa::param::video::VideoInfoRaw,
}

struct ScreencastStream {
    _stream: StreamRc,
    _listener: StreamListener<ScreencastStreamData>,
}

impl ScreencastStream {
    fn new(node_id: u32, core: CoreRc) -> AResult<Self> {
        let stream = pw::stream::StreamRc::new(
            core.clone(),
            "hazel-screencapture",
            properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Screen",
            },
        )?;

        let listener = stream
            .add_local_listener_with_user_data(ScreencastStreamData::default())
            .param_changed(Self::on_param_changed)
            .process(Self::on_process)
            .register()
            .unwrap();

        let obj = pw::spa::pod::object!(
            pw::spa::utils::SpaTypes::ObjectParamFormat,
            pw::spa::param::ParamType::EnumFormat,
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::MediaType,
                Id,
                pw::spa::param::format::MediaType::Video
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::MediaSubtype,
                Id,
                pw::spa::param::format::MediaSubtype::Raw
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::VideoFormat,
                Choice,
                Enum,
                Id,
                pw::spa::param::video::VideoFormat::RGB,
                pw::spa::param::video::VideoFormat::RGBA,
                pw::spa::param::video::VideoFormat::RGBx,
                pw::spa::param::video::VideoFormat::BGRx,
                pw::spa::param::video::VideoFormat::YUY2,
                pw::spa::param::video::VideoFormat::I420,
            ),
            pw::spa::pod::Property {
                key: FormatProperties::VideoModifier.as_raw(),
                flags: PropertyFlags::MANDATORY | PropertyFlags::DONT_FIXATE,
                value: pw::spa::pod::Value::Choice(ChoiceValue::Long(libspa::utils::Choice(
                    ChoiceFlags::empty(),
                    libspa::utils::ChoiceEnum::Enum {
                        default: u64::from(DrmModifier::Linear) as i64,
                        alternatives: vec![u64::from(DrmModifier::Invalid) as i64,],
                    }
                )))
            },
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::VideoSize,
                Choice,
                Range,
                Rectangle,
                pw::spa::utils::Rectangle {
                    width: 320,
                    height: 240
                },
                pw::spa::utils::Rectangle {
                    width: 1,
                    height: 1
                },
                pw::spa::utils::Rectangle {
                    width: 4096,
                    height: 4096
                }
            ),
            pw::spa::pod::property!(
                pw::spa::param::format::FormatProperties::VideoFramerate,
                Choice,
                Range,
                Fraction,
                pw::spa::utils::Fraction { num: 25, denom: 1 },
                pw::spa::utils::Fraction { num: 0, denom: 1 },
                pw::spa::utils::Fraction {
                    num: 1000,
                    denom: 1
                }
            ),
        );

        let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pw::spa::pod::Value::Object(obj),
        )
        .unwrap()
        .0
        .into_inner();

        let mut params = [pw::spa::pod::Pod::from_bytes(&values).unwrap()];

        stream.connect(
            pw::spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )?;

        Ok(Self { _stream: stream, _listener: listener })
    }

    fn on_param_changed(
        stream: &Stream,
        this: &mut ScreencastStreamData,
        id: u32,
        param: Option<&Pod>,
    ) {
        let Some(param) = param else {
            return;
        };

        if id != pw::spa::param::ParamType::Format.as_raw() {
            return;
        }

        let (media_type, media_subtype) = match pw::spa::param::format_utils::parse_format(param) {
            Ok(v) => v,
            Err(_) => return,
        };

        if media_type != pw::spa::param::format::MediaType::Video
            || media_subtype != pw::spa::param::format::MediaSubtype::Raw
        {
            return;
        }

        this.format
            .parse(param)
            .expect("Failed to parse param changed to VideoInfoRaw");

        println!("got video format:");
        println!("{:?}", this.format);
        println!(
            "\tsize: {}x{}",
            this.format.size().width,
            this.format.size().height
        );
        println!("{:?}", param.type_());
        println!(
            "\tframerate: {}/{}",
            this.format.framerate().num,
            this.format.framerate().denom
        );

        // Initialize encoder once format is known
    }

    fn on_process(stream: &Stream, this: &mut ScreencastStreamData) {
    }
}

pub async fn start_streaming() -> AResult<()> {
    let (node_id, fd) = open_portal().await.expect("failed to open portal");

    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_fd_rc(fd, None)?;

    let stream = ScreencastStream::new(node_id, core).expect("Failed to create screencast stream");

    mainloop.run();

    Ok(())
}
