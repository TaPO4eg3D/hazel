use std::{
    io::Cursor,
    os::fd::OwnedFd,
    sync::{Arc, Mutex},
    task::{Poll, Waker},
    thread,
};

use anyhow::Result as AResult;
use ashpd::{
    desktop::{
        PersistMode, Session,
        screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
    },
    enumflags2::BitFlags,
};
use drm_fourcc::{DrmFormat, DrmFourcc, DrmModifier};
use smallvec::smallvec;

// TODO: Both should be configurable
const DEFAULT_FRAMERATE: u32 = 60;
const DEFAULT_BITRATE: u32 = 40 * 1000_u32.pow(2);

use gpui::{DMABuffer, DMABufferPlane};
use libspa::{
    buffer::{Data, DataType, meta::MetaHeader},
    param::{
        ParamType,
        format::{FormatProperties, MediaSubtype, MediaType},
        video::VideoFormat,
    },
    pod::{ChoiceValue, Pod, Property, PropertyFlags, serialize::PodSerializer},
    sys::{
        SPA_META_Header, SPA_PARAM_BUFFERS_dataType, SPA_PARAM_META_size, SPA_PARAM_META_type,
        spa_meta_header,
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
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::Split};
use smallvec::SmallVec;
use smol::channel::{Receiver, Sender, bounded};

use crate::video::{
    decode::{DecodedFrame, VAAPIDecoder, VAAPIDecoderParams},
    encode::{VAAPIEncoder, VAAPIEncoderParams},
    wrapper::{DrmFrame, DrmInfo},
};

async fn open_portal() -> ashpd::Result<(Session<Screencast>, u32, OwnedFd)> {
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

    Ok((session, stream.pipe_wire_node_id(), fd))
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

    preview_tx: FrameSender<gpui::DMABuffer>,

    empty_frame_queue: Option<HeapCons<Vec<u8>>>,
    ready_frame_queue: Option<HeapProd<Vec<u8>>>,
}

struct ScreencastStream {
    _stream: StreamRc,
    _listener: StreamListener<ScreencastStreamData>,
}

struct ScreencastStreamParams {
    core: CoreRc,
    node_id: u32,

    preview_tx: FrameSender<gpui::DMABuffer>,

    empty_frame_queue: HeapCons<Vec<u8>>,
    ready_frame_queue: HeapProd<Vec<u8>>,
}

impl ScreencastStream {
    fn new(
        ScreencastStreamParams {
            node_id,
            core,
            preview_tx,
            empty_frame_queue,
            ready_frame_queue,
        }: ScreencastStreamParams,
    ) -> AResult<Self> {
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
                preview_tx,

                encoder: None,
                format: Default::default(),
                empty_frame_queue: Some(empty_frame_queue),
                ready_frame_queue: Some(ready_frame_queue),
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
                VideoFormat::BGR,
                VideoFormat::BGRA,
                VideoFormat::RGBx,
                VideoFormat::BGRx,
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
                        flags: vec![1 << DataType::DmaBuf.as_raw()],
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

        stream.update_params(&mut params).unwrap()
    }

    fn build_drm_frame(data: &mut Data, this: &ScreencastStreamData) -> (DrmFrame, DrmInfo) {
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
            VideoFormat::BGRA => DrmFourcc::Xrgb8888,
            VideoFormat::RGBx => DrmFourcc::Xbgr8888,
            format => todo!("Unimplemnted: {format:?}"),
        };

        let format = DrmInfo {
            width: width as i32,
            height: height as i32,
            format,
            modifier: DrmModifier::try_from(this.format.modifier()).unwrap(),
            plane_offset: offset,
            plane_stride: stride,
        };

        // TODO: Pipewire cycles a set of DMA-bufs, in theory we can cache
        // it using the file descriptor
        (
            DrmFrame::new(fd, (stride * height as i32) as usize, format),
            format,
        )
    }

    fn process_dmabuf(mut buffer: Buffer, this: &mut ScreencastStreamData) {
        let data = &mut buffer.datas_mut()[0];
        let (drm_frame, drm_info) = Self::build_drm_frame(data, this);

        let drm_fd = drm_frame.fd;
        match this.encoder.as_mut() {
            Some(encoder) => encoder.update_frame(drm_frame),
            None => {
                let width = this.format.size().width;
                let height = this.format.size().height;

                this.encoder = Some(VAAPIEncoder::new(VAAPIEncoderParams {
                    height,
                    width,
                    drm_frame,

                    framerate: DEFAULT_FRAMERATE,
                    bitrate: DEFAULT_BITRATE,

                    empty_frame_queue: this.empty_frame_queue.take().unwrap(),
                    ready_frame_queue: this.ready_frame_queue.take().unwrap(),
                }));
            }
        }

        this.preview_tx.send(DMABuffer {
            fd: drm_fd as i32,
            width: drm_info.width as u32,
            height: drm_info.height as u32,
            format: DrmFormat {
                code: drm_info.format,
                modifier: drm_info.modifier,
            },
            planes: smallvec![DMABufferPlane {
                offset: drm_info.plane_offset as usize,
                stride: drm_info.plane_stride as usize,
            }],
        });

        // `seq` advances on each frame, `pts` advances on
        // buffer update
        if let Some(header) = buffer.find_meta::<MetaHeader>() {
            let encoder = this.encoder.as_mut().unwrap();

            encoder.encode(header.seq() as i64);
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

struct FrameChannelInner<T> {
    frame: Option<T>,
    waker: Option<Waker>,

    closed: bool,
}

struct FrameSender<T> {
    inner: Arc<Mutex<FrameChannelInner<T>>>,
}

impl<T> Drop for FrameSender<T> {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        inner.closed = true;
    }
}

impl<T> FrameSender<T> {
    fn send(&self, frame: T) {
        let mut inner = self.inner.lock().unwrap();
        inner.frame = Some(frame);

        if let Some(waker) = inner.waker.take() {
            waker.wake();
        }
    }
}

struct FrameRecvFuture<T> {
    inner: Arc<Mutex<FrameChannelInner<T>>>,
}

impl<T> Future for FrameRecvFuture<T> {
    type Output = Option<T>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let mut inner = self.inner.lock().unwrap();
        if inner.closed {
            return Poll::Ready(None);
        }

        if inner.frame.is_none() {
            inner.waker = Some(cx.waker().clone());

            return Poll::Pending;
        }

        return Poll::Ready(inner.frame.take());
    }
}

pub struct FrameRecv<T> {
    inner: Arc<Mutex<FrameChannelInner<T>>>,
}

impl<T> FrameRecv<T> {
    pub fn recv(&self) -> impl Future {
        FrameRecvFuture {
            inner: self.inner.clone(),
        }
    }
}

fn frame_channel<T>() -> (FrameSender<T>, FrameRecv<T>) {
    let inner = Arc::new(Mutex::new(FrameChannelInner {
        frame: None,
        waker: None,

        closed: false,
    }));

    (
        FrameSender {
            inner: inner.clone(),
        },
        FrameRecv {
            inner: inner.clone(),
        },
    )
}

pub type ScreencastPreview = FrameRecv<gpui::DMABuffer>;

pub struct StartedScreencast {
    pw_tx: pipewire::channel::Sender<()>,

    empty_frame_queue: HeapProd<Vec<u8>>,
    ready_frame_queue: HeapCons<Vec<u8>>,
}

pub async fn start_screencast() -> AResult<(StartedScreencast, ScreencastPreview)> {
    let (_session, node_id, fd) = open_portal().await.expect("failed to open portal");

    let (pw_tx, pw_rx) = pipewire::channel::channel::<()>();
    let (preview_tx, preview_rx) = frame_channel();

    let ring = HeapRb::new(4);
    let (empty_frame_queue_prod, emtpy_frame_queue_cons) = ring.split();

    let ring = HeapRb::new(4);
    let (ready_frame_queue_prod, ready_frame_queue_cons) = ring.split();

    pw::init();

    _ = thread::spawn(move || {
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_fd_rc(fd, None)?;

        let _stream = ScreencastStream::new(ScreencastStreamParams {
            core,
            node_id,
            preview_tx,
            empty_frame_queue: emtpy_frame_queue_cons,
            ready_frame_queue: ready_frame_queue_prod,
        })
        .expect("Failed to create screencast stream");

        let _attached = pw_rx.attach(mainloop.loop_(), {
            let mainloop = mainloop.clone();

            move |_| {
                mainloop.quit();
            }
        });

        mainloop.run();

        Ok::<_, anyhow::Error>(())
    });

    Ok((
        StartedScreencast {
            pw_tx,
            empty_frame_queue: empty_frame_queue_prod,
            ready_frame_queue: ready_frame_queue_cons,
        },
        preview_rx,
    ))
}
