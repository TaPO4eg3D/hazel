use std::{
    io::{Cursor, Write},
    os::fd::OwnedFd,
    time::Instant,
};

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
use drm_fourcc::{DrmFourcc, DrmModifier};
use ffmpeg_next::format::open;

use libspa::{
    buffer::{Data, DataType, meta::MetaHeader},
    param::{
        ParamType,
        format::{FormatProperties, MediaSubtype, MediaType},
        video::VideoFormat,
    },
    pod::{ChoiceValue, Pod, Property, PropertyFlags, serialize::PodSerializer},
    sys::{
        SPA_META_Header, SPA_PARAM_BUFFERS_blocks, SPA_PARAM_BUFFERS_buffers, SPA_PARAM_BUFFERS_dataType, SPA_PARAM_META_size, SPA_PARAM_META_type, spa_data, spa_meta_header
    },
    utils::{Choice, ChoiceEnum, ChoiceFlags, Id, SpaTypes},
};
use pipewire::{
    self as pw,
    buffer::Buffer,
    core::CoreRc,
    properties::properties,
    stream::{Stream, StreamListener, StreamRc},
};

use crate::video::{
    encode::{VAAPIEncoder, VAAPIEncoderParams},
    wrapper::{DrmFormat, DrmFrame, DrmPlane},
};

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

fn make_pod(buffer: &mut Vec<u8>, object: pw::spa::pod::Object) -> &Pod {
    PodSerializer::serialize(
        Cursor::new(&mut *buffer),
        &pw::spa::pod::Value::Object(object),
    )
    .unwrap();
    Pod::from_bytes(buffer).unwrap()
}

struct ScreencastStreamData {
    encoder: Option<VAAPIEncoder>,
    format: pw::spa::param::video::VideoInfoRaw,

    fout: std::fs::File,
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
            .add_local_listener_with_user_data(ScreencastStreamData {
                encoder: None,
                format: Default::default(),
                fout: std::fs::File::create("/tmp/screengrab.out").unwrap(),
            })
            .param_changed(Self::on_param_changed)
            .process(Self::on_process)
            .register()
            .unwrap();

        let dma_obj = pw::spa::pod::object!(
            SpaTypes::ObjectParamFormat,
            ParamType::EnumFormat,
            pw::spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
            pw::spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
            pw::spa::pod::property!(
                FormatProperties::VideoFormat,
                Choice,
                Enum,
                Id,
                VideoFormat::RGB,
                VideoFormat::RGBA,
                VideoFormat::RGBx,
                VideoFormat::BGRx,
                VideoFormat::YUY2,
                VideoFormat::I420,
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
            pw::spa::pod::Property {
                // we only want variable rate, thus bypassing compositor pacing
                key: FormatProperties::VideoFramerate.as_raw(),
                flags: PropertyFlags::empty(),
                value: pw::spa::pod::Value::Fraction(pw::spa::utils::Fraction { num: 0, denom: 1 })
            },
        );

        let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pw::spa::pod::Value::Object(dma_obj),
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

        Ok(Self {
            _stream: stream,
            _listener: listener,
        })
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
        this.encoder = None;

        println!("Format updated: {:#?}", this.format);

        // Ack the buffer type and metadata
        let data_type_obj = pw::spa::pod::object!(
            SpaTypes::ObjectParamBuffers,
            ParamType::Buffers,
            // TODO: Implement fallback to shared memory
            Property::new(
                SPA_PARAM_BUFFERS_dataType,
                pw::spa::pod::Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Flags {
                        default: 1 << DataType::DmaBuf.as_raw(),
                        flags: vec![
                            1 << DataType::DmaBuf.as_raw()
                        ],
                    },
                ))),
            ),
        );

        let meta_obj = pw::spa::pod::object!(
            SpaTypes::ObjectParamMeta,
            ParamType::Meta,
            Property::new(
                SPA_PARAM_META_type,
                pw::spa::pod::Value::Id(Id(SPA_META_Header)),
            ),
            Property::new(
                SPA_PARAM_META_size,
                pw::spa::pod::Value::Int(std::mem::size_of::<spa_meta_header>() as i32),
            ),
        );

        let mut data_type_buff = vec![];
        let mut meta_buff = vec![];

        let mut params = [
            make_pod(&mut data_type_buff, data_type_obj),
            make_pod(&mut meta_buff, meta_obj),
        ];

        stream.update_params(&mut params)
            .unwrap()
    }

    fn build_drm_frame(data: &mut Data, this: &ScreencastStreamData) -> DrmFrame {
        let data_raw = data.as_raw();
        let fd = data_raw.fd;

        let (stride, offset) = unsafe {
            let chunk = data_raw.chunk;
            ((*chunk).stride, (*chunk).offset)
        };

        let width = this.format.size().width;
        let height = this.format.size().height;

        let format = match this.format.format() {
            VideoFormat::BGRx => DrmFourcc::Xrgb8888,
            VideoFormat::RGBx => DrmFourcc::Xbgr8888,
            _ => todo!("Implement"),
        };

        let format = DrmFormat {
            width: width as i32,
            height: height as i32,
            format,
            modifier: this.format.modifier(),
        };

        DrmFrame::new(
            fd,
            (stride * height as i32) as usize,
            format,
            &[DrmPlane {
                offset: offset as isize,
                stride: stride as isize,
            }],
        )
    }

    fn process_dmabuf(mut buffer: Buffer, this: &mut ScreencastStreamData) {
        let data = &mut buffer.datas_mut()[0];
        let drm_frame = Self::build_drm_frame(data, this);

        match this.encoder.as_mut() {
            Some(encoder) => encoder.update_frame(drm_frame),
            None => {
                let width = this.format.size().width;
                let height = this.format.size().height;

                this.encoder = Some(VAAPIEncoder::new(
                    VAAPIEncoderParams { height, width },
                    drm_frame,
                ));
            }
        }

        // `seq` advances on each frame, `pts` advances on 
        // buffer update
        if let Some(header) = buffer.find_meta::<MetaHeader>() {
            let encoder = this.encoder.as_mut().unwrap();
            encoder.encode(header.seq() as i64);

            while let Some(data) = encoder.frame_queue.pop_front() {
                this.fout.write_all(&data).unwrap();
            }
        }
    }

    fn on_process(stream: &Stream, this: &mut ScreencastStreamData) {
        let mut buffer = None;

        // Drain the queue, always grab the most recent buffer
        loop {
            let Some(value) = stream.dequeue_buffer() else {
                break;
            };

            buffer = Some(value);
        }

        let Some(mut buffer) = buffer else {
            return;
        };

        let datas = buffer.datas_mut();
        if datas.is_empty() {
            return;
        }

        let data = &mut datas[0];
        match data.type_() {
            DataType::DmaBuf => {
                Self::process_dmabuf(buffer, this);
            }
            DataType::MemFd => {
                panic!("Fallback to shared memory is not yet supported");
            }
            _ => todo!("Hanlde those cases?"),
        }
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
