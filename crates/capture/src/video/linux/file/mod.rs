use std::{
    io::Write as _,
    os::fd::AsRawFd as _,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use gpui::DMABuffer;
use smol::channel::{self, Receiver, Sender};

use ffmpeg_next::{
    codec, format, frame, media,
    software::scaling::{Context as ScalingContext, flag::Flags as ScalingFlags},
};

use crate::video::linux::file::vulkan::{DmaBufferPoolOptions, VkDmaBufferPool};

mod vulkan;

// Streams a video file as a sequence of DMA-BUFs.
// Main purpose is an emulation of zero-copy screencapturing
// in a predictable way.
//
// It loads the encoded file in memory (via memfd) and replays it with
// a specified FPS in a loop
pub struct FileStreamer {
    frametime: Duration,
    frame_rx: Receiver<gpui::DMABuffer>,
}

struct PlayingContext<const N: usize> {
    path: String,
    dma_pool: VkDmaBufferPool<N>,
    scaler: ScalingContext,
    frame_tx: Sender<gpui::DMABuffer>,
    frametime: Duration,
    next_frame: Instant,
}

impl FileStreamer {
    pub fn new(file_path: impl AsRef<Path>, fps: f64) -> Self {
        if !file_path.as_ref().exists() {
            panic!("Invalid file path");
        }

        let content = std::fs::read(file_path).expect("Failed to read the file");

        let mfd_options = memfd::MemfdOptions::default();
        let mfd = mfd_options
            .create("file-stream")
            .expect("Failed to create memfd file");

        mfd.as_file()
            .write_all(&content)
            .expect("Failed to write data");

        let (frame_tx, frame_rx) = channel::bounded(1);
        let frametime = Duration::from_secs_f64(1.0 / fps);

        let instance = Self {
            frametime,
            frame_rx,
        };
        instance.start_streaming_thread(mfd, frametime, frame_tx);

        instance
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

    fn play<const N: usize>(ctx: &mut PlayingContext<N>) {
        let mut input = format::input(&ctx.path).expect("Failed to load memfile");

        let stream = input.streams().best(media::Type::Video).unwrap();
        let stream_index = stream.index();

        let context = codec::context::Context::from_parameters(stream.parameters()).unwrap();

        let mut decoder = context.decoder().video().unwrap();

        for (stream, packet) in input.packets() {
            if stream.index() != stream_index {
                continue;
            }

            decoder.send_packet(&packet).unwrap();

            // Not really efficient but hey, we're testing stuff
            let mut decoded_frame = frame::Video::empty();
            let mut rgba_frame = frame::Video::empty();

            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                let now = Instant::now();
                if ctx.next_frame > now {
                    thread::sleep(ctx.next_frame - now);
                }

                ctx.scaler.run(&decoded_frame, &mut rgba_frame).unwrap();
                let data = Self::rgba_tightly_packed(&rgba_frame);
                let buff = ctx.dma_pool.push_image(&data);
                ctx.frame_tx.send_blocking(buff).unwrap();

                let now = Instant::now();
                ctx.next_frame = now + ctx.frametime;
            }
        }
    }

    fn start_streaming_thread(
        &self,
        mfd: memfd::Memfd,
        frametime: Duration,
        frame_tx: Sender<DMABuffer>,
    ) {
        thread::spawn(move || {
            ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Error);

            let path = format!("/proc/self/fd/{}", mfd.as_raw_fd());
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

            let dma_pool = VkDmaBufferPool::<12>::new(DmaBufferPoolOptions {
                width,
                height,
                vk_format: ash::vk::Format::R8G8B8A8_UNORM,
            });

            let mut ctx = PlayingContext {
                path,
                scaler,
                dma_pool,
                frametime,
                frame_tx,
                next_frame: Instant::now(),
            };

            loop {
                Self::play(&mut ctx);
            }
        });
    }

    pub async fn recv_frame(&mut self) -> DMABuffer {
        self.frame_rx.recv().await.unwrap()
    }
}
