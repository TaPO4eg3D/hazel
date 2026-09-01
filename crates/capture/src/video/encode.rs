use core::panic;

use ffmpeg_next::{
    Dictionary, Rational, codec,
    encoder::{self},
    ffi::{
        AV_BUFFERSRC_FLAG_KEEP_REF, AV_CODEC_FLAG_CLOSED_GOP, AVFrame, AVPacket, AVPixelFormat,
        EAGAIN, av_buffer_ref, av_buffersink_get_frame, av_buffersink_get_hw_frames_ctx,
        av_buffersrc_add_frame_flags, av_frame_alloc, av_frame_free, av_frame_unref,
        av_packet_alloc, av_packet_free, av_packet_unref, avcodec_receive_packet,
        avcodec_send_frame,
    },
};
use ringbuf::{
    HeapCons, HeapProd,
    traits::{Consumer, Producer},
};

use crate::video::wrapper::{
    DrmFrame, DrmInfo, Filter, GPUDevice, Graph, HWFrameContext, HWFrameContextBuilder, Parser,
    VAAPIFrame,
};

pub struct VAAPIEncoderParams {
    pub height: u32,
    pub width: u32,

    pub bitrate: u32,
    pub framerate: u32,

    pub empty_frame_queue: HeapCons<Vec<u8>>,
    pub ready_frame_queue: HeapProd<Vec<u8>>,
}

pub struct VAAPIEncoder {
    encoder: codec::encoder::video::Encoder,
    _graph: Graph,

    sink_filter: Filter,
    source_filter: Filter,

    hw_frame_ctx: HWFrameContext,
    out_frame: *mut AVFrame,

    packet: *mut AVPacket,

    pub(crate) empty_frame_queue: HeapCons<Vec<u8>>,
    pub(crate) ready_frame_queue: HeapProd<Vec<u8>>,
}

impl Drop for VAAPIEncoder {
    fn drop(&mut self) {
        unsafe {
            av_packet_free(&raw mut self.packet);
            av_frame_free(&raw mut self.out_frame);
        }
    }
}

impl VAAPIEncoder {
    pub fn alloc_frame(&self, drm_info: &DrmInfo) -> VAAPIFrame {
        let drm_frame = DrmFrame::new(drm_info);
        VAAPIFrame::new(drm_frame, self.hw_frame_ctx.clone())
    }

    #[hotpath::measure]
    pub fn encode(&mut self, hw_frame: &mut VAAPIFrame, pts: i64) {
        unsafe {
            (*hw_frame.av_frame).pts = pts;

            let err = av_buffersrc_add_frame_flags(
                self.source_filter.ctx,
                hw_frame.av_frame,
                AV_BUFFERSRC_FLAG_KEEP_REF as i32,
            );

            if err < 0 {
                panic!("Error feeding the filtergraph!");
            }

            // Pulling out the result of the filter graph
            let err = av_buffersink_get_frame(self.sink_filter.ctx, self.out_frame);
            if err == -EAGAIN {
                return;
            } else if err < 0 {
                panic!("Failed to process a frame")
            }

            let err = avcodec_send_frame(self.encoder.as_mut_ptr(), self.out_frame);
            // Unref the frame to release the VAAPI surface back to the pool.
            av_frame_unref(self.out_frame);

            if err < 0 {
                panic!("Failed to encode the frame");
            }

            loop {
                let ret = avcodec_receive_packet(self.encoder.as_mut_ptr(), self.packet);
                if ret != 0 {
                    break;
                }

                let Some(mut frame) = self.empty_frame_queue.try_pop() else {
                    print!("Can't claim an empty frame!");

                    continue;
                };

                frame.clear();

                (*self.packet).stream_index = 0;
                let buf =
                    std::slice::from_raw_parts((*self.packet).data, (*self.packet).size as usize);

                frame.extend_from_slice(buf);
                if self.ready_frame_queue.try_push(frame).is_err() {
                    print!("No space for the encoded frame!");
                }

                // Unref the packet to release the encoded bitstream buffer.
                av_packet_unref(self.packet);
            }
        }
    }

    pub fn new(
        VAAPIEncoderParams {
            height,
            width,
            bitrate,
            framerate,
            empty_frame_queue,
            ready_frame_queue,
        }: VAAPIEncoderParams,
    ) -> Self {
        let codec = encoder::find_by_name("h264_vaapi").expect("Failed to find Video Codec");
        let mut video = codec::Context::new_with_codec(codec)
            .encoder()
            .video()
            .expect("Failed to alloc codec context");

        let time_base = Rational(1, framerate as i32);

        let device = GPUDevice::new().expect("Failed to open GPU Device");
        let hw_frame_ctx = HWFrameContextBuilder::new(&device)
            .expect("Failed to allocate memory on GPU")
            .set_format(AVPixelFormat::AV_PIX_FMT_VAAPI)
            // TODO: We should accept this as a parameter (comes from the format negotiation)
            .set_sw_format(AVPixelFormat::AV_PIX_FMT_BGR0)
            .set_width(width as i32)
            .set_height(height as i32)
            .set_initial_pool_size(20)
            .build()
            .expect("Failed to build HWFrameContext");

        let graph = Graph::new().expect("Failed ot alloc filter graph");
        let source_filter = graph
            .create_buffer_filter("Source", |this| {
                this.set_format(AVPixelFormat::AV_PIX_FMT_VAAPI)
                    .set_hw_frame_ctx(hw_frame_ctx.clone())
                    .set_width(width as i32)
                    .set_height(height as i32)
                    .set_time_base(time_base)
                    .set_aspect_ratio(Rational(1, 1))
            })
            .expect("Failed to create buffer filter");

        let sink_filter = graph
            .create_buffersink_filter("Sink", |this| {
                this.set_pixel_formats(&[AVPixelFormat::AV_PIX_FMT_VAAPI])
                    .expect("Failed to set pixel format")
            })
            .expect("Failed to create buffersink filter");

        // Create the connections to the filter graph
        //
        // The in/out swap is not a mistake:
        //
        //   ----------       -----------------------------      --------
        //   | Source | ----> | in -> filter_graph -> out | ---> | Sink |
        //   ----------       -----------------------------      --------
        //
        // The 'in' of filter_graph is the output of the Source buffer
        // The 'out' of filter_graph is the input of the Sink buffer
        Parser::new(&graph)
            .output("in", &source_filter, 0)
            .input("out", &sink_filter, 0)
            .with_gpu_device(device)
            .parse("scale_vaapi=format=nv12:out_range=full");

        graph.config().expect("Failed to configure the graph");

        // TODO: Make a safe wrapper for that, I am feeling a bit lazy atm
        unsafe {
            // The (input of the) sink is the output of the whole filter.
            let filter_output = *(*sink_filter.ctx).inputs;
            let video_ctx = video.as_mut_ptr();

            video.set_width((*filter_output).w as u32);
            video.set_height((*filter_output).h as u32);

            // Make keyframes self-contained
            (*video_ctx).flags = AV_CODEC_FLAG_CLOSED_GOP as i32;
            // Produce keyframe every two seconds worth of frames
            // TODO: Move on fully on-demand IDR generation
            (*video_ctx).gop_size = framerate as i32 * 2;
            // B-Frames require buffering, disabling them
            (*video_ctx).max_b_frames = 0;

            // Effectively CBR
            (*video_ctx).bit_rate = bitrate as i64;
            (*video_ctx).rc_max_rate = bitrate as i64;
            (*video_ctx).rc_min_rate = bitrate as i64;

            (*video_ctx).pix_fmt =
                std::mem::transmute::<i32, AVPixelFormat>((*filter_output).format);

            // NOTE: Encoder drop will unref this
            (*video_ctx).hw_frames_ctx =
                av_buffer_ref(av_buffersink_get_hw_frames_ctx(sink_filter.ctx));

            video.set_time_base((*filter_output).time_base);
            video.set_frame_rate(Some(Rational(framerate as i32, 1)));
            video.set_aspect_ratio((*filter_output).sample_aspect_ratio);
        }

        let out_frame = unsafe { av_frame_alloc() };
        if out_frame.is_null() {
            panic!("Failed to alloc out frame");
        }

        let packet = unsafe { av_packet_alloc() };
        if packet.is_null() {
            panic!("Failed to alloc encoder packet");
        }

        let mut encoder_options = Dictionary::new();
        // Disable internal buffering in GPU
        encoder_options.set("async_depth", "1");
        // Always generate full IDR (a real random-access point, not just an I-frame)
        encoder_options.set("idr_interval", "0");

        let encoder = video
            .open_with(encoder_options)
            .expect("Failed to open the codec");

        Self {
            encoder,
            sink_filter,
            source_filter,
            _graph: graph,
            hw_frame_ctx,
            out_frame,
            packet,
            empty_frame_queue,
            ready_frame_queue,
        }
    }
}
