use core::panic;
use std::{
    ffi::{c_int, c_uint, c_void, CString},
    marker::PhantomData,
    ptr,
};

use drm_fourcc::{DrmFourcc, DrmModifier};
use ffmpeg_next::{
    Rational, codec, encoder::{self, Encoder, video}, ffi::{
        AV_BUFFERSRC_FLAG_KEEP_REF, AVFrame, AVPacket, AVPixelFormat, EAGAIN, av_buffer_ref, av_buffersink_get_frame, av_buffersink_get_hw_frames_ctx, av_buffersrc_add_frame_flags, av_frame_alloc, av_frame_free, av_packet_alloc, av_packet_free, avcodec_receive_packet, avcodec_send_frame
    }, filter, format::Pixel
};

use crate::video::wrapper::{
    DrmFrame, Filter, GPUDevice, Graph, HWFrameContextBuilder, Parser, VAAPIFrame,
};

pub struct VAAPIEncoderParams {
    pub height: u32,
    pub width: u32,
}

pub struct VAAPIEncoder {
    encoder: codec::encoder::video::Encoder,
    graph: Graph,

    sink_filter: Filter,
    source_filter: Filter,

    hw_frame: VAAPIFrame,
    out_frame: *mut AVFrame,

    packet: *mut AVPacket,
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
    pub fn encode(&mut self) {
        unsafe {
            let err = av_buffersrc_add_frame_flags(
                self.source_filter.ctx,
                self.hw_frame.av_frame,
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
            if err < 0 {
                panic!("Failed to encode the frame");
            }

            loop {
                let ret = avcodec_receive_packet(self.encoder.as_mut_ptr(), self.packet);

                // TODO: Handle better?
                if ret != 0 {
                    break;
                }

                (*self.packet).stream_index = 0;
                let buf =
                    std::slice::from_raw_parts((*self.packet).data, (*self.packet).size as usize);
            }
        }
    }

    pub fn new(params: VAAPIEncoderParams, drm_frame: DrmFrame) -> Self {
        let codec = encoder::find_by_name("h264_vaapi").expect("Failed to find Video Codec");
        let mut video = codec::Context::new_with_codec(codec)
            .encoder()
            .video()
            .expect("Failed to alloc codec context");

        let time_base = Rational(1, 1000000);

        let device = GPUDevice::new().expect("Failed to open GPU Device");
        let hw_frame_ctx = HWFrameContextBuilder::new(&device)
            .expect("Failed to allocate memory on GPU")
            .set_format(AVPixelFormat::AV_PIX_FMT_VAAPI)
            .set_sw_format(AVPixelFormat::AV_PIX_FMT_BGR0)
            .set_width(params.width as i32)
            .set_height(params.height as i32)
            .set_initial_pool_size(20)
            .build()
            .expect("Failed to build HWFrameContext");

        let graph = Graph::new().expect("Failed ot alloc filter graph");
        let source_filter = graph
            .create_buffer_filter("Source", |this| {
                this.set_format(AVPixelFormat::AV_PIX_FMT_VAAPI)
                    .set_hw_frame_ctx(hw_frame_ctx.clone())
                    .set_width(params.width as i32)
                    .set_height(params.height as i32)
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

            video.set_width((*filter_output).w as u32);
            video.set_height((*filter_output).h as u32);

            (*video.as_mut_ptr()).pix_fmt =
                std::mem::transmute::<i32, AVPixelFormat>((*filter_output).format);
            // TODO: unref at the end of lifetime
            (*video.as_mut_ptr()).hw_frames_ctx =
                av_buffer_ref(av_buffersink_get_hw_frames_ctx(sink_filter.ctx));

            video.set_time_base((*filter_output).time_base);
            video.set_frame_rate(Some(Rational(1, 0)));
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

        let hw_frame = VAAPIFrame::new(drm_frame, hw_frame_ctx.clone());
        let encoder = video.open().expect("Failed to open the codec");

        Self {
            encoder,
            sink_filter,
            source_filter,
            graph,
            hw_frame,
            out_frame,
            packet,
        }
    }
}
