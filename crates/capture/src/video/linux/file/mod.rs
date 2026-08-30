use std::{
    io::Write as _,
    os::fd::AsRawFd as _,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use crossbeam::channel;
use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Producer as _, Split as _},
};

use ffmpeg_next::{
    codec, format, frame, media,
    software::scaling::{Context as ScalingContext, flag::Flags as ScalingFlags},
};

use crate::{
    CaptureNotifier,
    video::{
        encode::{VAAPIEncoder, VAAPIEncoderParams},
        frames::{FramePool, FrameRecv, FrameSender, frame_channel},
        linux::{
            ActiveVideoStream,
            file::vulkan::{DmaBufferPoolOptions, VkDmaBufferPool},
        },
        wrapper::{DrmInfo, VAAPIFrame},
    },
};

mod vulkan;

const DEFAULT_FRAMERATE: u32 = 30;
const DEFAULT_BITRATE: u32 = 16 * 1000_u32.pow(2);

// Streams a video file as a sequence of DMA-BUFs.
// Main purpose is an emulation of zero-copy screencapturing
// in a predictable way.
//
// It loads the encoded file in memory (via memfd) and replays it with
// a specified FPS in a loop
pub struct FileStreamer {
    mfd: memfd::Memfd,
    params: FileStreamerParams,
    frametime: Duration,
}

struct FileStreamerParams {
    fps: f64,

    default_mode: FileVideoStreamMode,

    notifier: CaptureNotifier,
    command_rx: Option<crossbeam::channel::Receiver<FileVideoStreamCommand>>,

    empty_frames_cons: Option<HeapCons<Vec<u8>>>,
    ready_frames_prod: Option<HeapProd<Vec<u8>>>,

    preview_tx: FrameSender<gpui::DMABuffer>,
}

struct PlayingContext<const N: usize> {
    pts: i64,

    path: String,
    dma_pool: VkDmaBufferPool<N>,

    scaler: ScalingContext,
    next_frame: Instant,

    mode: FileVideoStreamMode,

    command_rx: crossbeam::channel::Receiver<FileVideoStreamCommand>,
    vaapi_cache: Vec<(gpui::DMABuffer, VAAPIFrame)>,

    encoder: VAAPIEncoder,
}

impl FileStreamer {
    fn new(file_path: impl AsRef<Path>, params: FileStreamerParams) -> Self {
        if !file_path.as_ref().exists() {
            panic!("Invalid file path");
        }

        let content = std::fs::read(file_path.as_ref()).expect("Failed to read the file");

        let mfd_options = memfd::MemfdOptions::default();
        let mfd = mfd_options
            .create("file-stream")
            .expect("Failed to create memfd file");

        mfd.as_file()
            .write_all(&content)
            .expect("Failed to write data");

        let frametime = Duration::from_secs_f64(1.0 / params.fps);

        Self {
            frametime,
            mfd,
            params,
        }
    }

    fn rgba_tightly_packed(frame: &frame::Video) -> Vec<u8> {
        let width = frame.width() as usize;
        let height = frame.height() as usize;

        let row_bytes = width * 4;
        let stride = frame.stride(0);
        let src = frame.data(0);

        if stride == row_bytes {
            return src[..row_bytes * height].to_vec();
        }

        let mut dst = vec![0u8; row_bytes * height];

        for y in 0..height {
            let src_row = &src[y * stride..y * stride + row_bytes];
            let dst_row = &mut dst[y * row_bytes..(y + 1) * row_bytes];

            dst_row.copy_from_slice(src_row);
        }

        dst
    }

    fn play<const N: usize>(&self, cx: &mut PlayingContext<N>) {
        let mut input = format::input(&cx.path).expect("Failed to load memfile");

        let stream = input.streams().best(media::Type::Video).unwrap();
        let stream_index = stream.index();

        let context = codec::context::Context::from_parameters(stream.parameters()).unwrap();

        let mut decoder = context.decoder().video().unwrap();

        for (stream, packet) in input.packets() {
            if stream.index() != stream_index {
                continue;
            }

            match cx.mode {
                FileVideoStreamMode::Auto(_) => {
                    if let Ok(cmd) = cx.command_rx.try_recv() {
                        match cmd {
                            FileVideoStreamCommand::SwitchMode(mode) => cx.mode = mode,
                            FileVideoStreamCommand::NextFrame => {}
                        }
                    }
                }
                FileVideoStreamMode::Manual => {
                    if let Ok(cmd) = cx.command_rx.recv() {
                        match cmd {
                            FileVideoStreamCommand::SwitchMode(mode) => cx.mode = mode,
                            FileVideoStreamCommand::NextFrame => {}
                        }
                    }
                }
            }

            decoder.send_packet(&packet).unwrap();

            // Not really efficient but hey, we're testing stuff
            let mut decoded_frame = frame::Video::empty();
            let mut rgba_frame = frame::Video::empty();

            // TODO: Fix this shitty loop.
            // I don't yet know if it's okay for testing purposes
            // but it feels very wrong to rely on CPU so heavily
            // when in the real pipeline we do GPU processing End-to-End
            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                if matches!(cx.mode, FileVideoStreamMode::Auto(_)) {
                    let now = Instant::now();

                    if cx.next_frame > now {
                        thread::sleep(cx.next_frame - now);
                    }
                }

                let now = Instant::now();

                cx.scaler.run(&decoded_frame, &mut rgba_frame).unwrap();
                let data = Self::rgba_tightly_packed(&rgba_frame);

                let buff = cx.dma_pool.push_image(&data);
                let vaapi_frame = match cx.vaapi_cache.iter().position(|(dma, _)| dma == &buff) {
                    Some(idx) => &mut cx.vaapi_cache[idx].1,
                    None => {
                        let idx = cx.vaapi_cache.len();
                        let drm_info: DrmInfo = buff.clone().into();

                        let vaapi_frame = cx.encoder.alloc_frame(&drm_info);

                        cx.vaapi_cache.push((buff.clone(), vaapi_frame));
                        &mut cx.vaapi_cache[idx].1
                    }
                };

                self.params.preview_tx.send(buff);
                cx.encoder.encode(vaapi_frame, cx.pts);
                cx.pts += 1;

                self.params.notifier.notify_screen();

                cx.next_frame = now + self.frametime;
            }
        }
    }

    fn start_streaming(&mut self) {
        ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Error);

        let path = format!("/proc/self/fd/{}", self.mfd.as_raw_fd());
        let input = format::input(&path).expect("Failed to load memfile");

        let stream = input.streams().best(media::Type::Video).unwrap();
        let context = codec::context::Context::from_parameters(stream.parameters()).unwrap();

        let decoder = context.decoder().video().unwrap();

        let width = decoder.width();
        let height = decoder.height();

        let format = decoder.format();

        // Most common anyway, no point in making it generic right now
        assert!(format == format::Pixel::YUV420P);

        let scaler = ScalingContext::get(
            decoder.format(),
            width,
            height,
            format::Pixel::RGBA,
            width,
            height,
            ScalingFlags::BILINEAR,
        )
        .expect("Failed to create scaling context");

        let dma_pool = VkDmaBufferPool::<6>::new(DmaBufferPoolOptions {
            width,
            height,
            vk_format: ash::vk::Format::R8G8B8A8_UNORM,
        });

        let encoder = VAAPIEncoder::new(VAAPIEncoderParams {
            height,
            width,

            framerate: DEFAULT_FRAMERATE,
            bitrate: DEFAULT_BITRATE,

            empty_frame_queue: self.params.empty_frames_cons.take().unwrap(),
            ready_frame_queue: self.params.ready_frames_prod.take().unwrap(),
        });

        let mut ctx = PlayingContext {
            pts: 0,
            path,
            scaler,
            dma_pool,
            encoder,
            mode: self.params.default_mode,
            command_rx: self.params.command_rx.take().unwrap(),
            vaapi_cache: vec![],
            next_frame: Instant::now(),
        };

        loop {
            self.play(&mut ctx);
        }
    }
}

#[derive(Clone, Copy)]
pub enum FileVideoStreamMode {
    Auto(f64),
    Manual,
}

#[derive(Clone, Copy)]
pub enum FileVideoStreamCommand {
    SwitchMode(FileVideoStreamMode),
    NextFrame,
}

pub struct FileVideoStream {
    command_tx: crossbeam::channel::Sender<FileVideoStreamCommand>,
    pub frame_pool: FramePool,
}

impl FileVideoStream {
    pub fn set_mode(&self, mode: FileVideoStreamMode) {
        let _ = self
            .command_tx
            .send(FileVideoStreamCommand::SwitchMode(mode));
    }

    pub fn next_frame(&self) {
        let _ = self.command_tx.send(FileVideoStreamCommand::NextFrame);
    }

    pub fn close(self) {
        todo!()
    }
}

pub async fn init_screencast(
    file_path: impl AsRef<Path>,
    default_mode: FileVideoStreamMode,
    notifier: CaptureNotifier,
) -> anyhow::Result<(ActiveVideoStream, FrameRecv<gpui::DMABuffer>)> {
    let (preview_tx, preview_rx) = frame_channel();

    let ring = HeapRb::new(4);
    let (mut empty_frames_prod, empty_frames_cons) = ring.split();

    let ring = HeapRb::new(4);
    let (ready_frames_prod, ready_frames_cons) = ring.split();

    let (command_tx, command_rx) = channel::bounded(1);

    for _ in 0..4 {
        _ = empty_frames_prod.try_push(vec![]);
    }

    let mut streamer = FileStreamer::new(
        file_path,
        FileStreamerParams {
            fps: DEFAULT_FRAMERATE as f64,
            default_mode,
            notifier,
            command_rx: Some(command_rx),
            empty_frames_cons: Some(empty_frames_cons),
            ready_frames_prod: Some(ready_frames_prod),
            preview_tx,
        },
    );

    thread::spawn(move || {
        streamer.start_streaming();
    });

    Ok((
        ActiveVideoStream::File(FileVideoStream {
            command_tx,
            frame_pool: FramePool::new(empty_frames_prod, ready_frames_cons),
        }),
        preview_rx,
    ))
}
