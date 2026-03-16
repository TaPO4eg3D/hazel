use std::{collections::VecDeque, ffi::c_int};

use drm_fourcc::DrmFourcc;
use ffmpeg_next::ffi::{
    av_frame_alloc, av_frame_free, av_frame_unref, av_hwframe_map, av_packet_alloc,
    av_packet_free, av_parser_close, av_parser_init, av_parser_parse2, avcodec_alloc_context3,
    avcodec_find_decoder, avcodec_free_context, avcodec_open2, avcodec_receive_frame,
    avcodec_send_packet, AVCodecContext, AVCodecParserContext, AVDRMFrameDescriptor, AVFrame,
    AVPacket, AVPixelFormat, AV_HWFRAME_MAP_DIRECT, AV_HWFRAME_MAP_READ, EAGAIN,
};

use crate::video::wrapper::{DrmPlane, GPUDevice};

unsafe extern "C" fn vaapi_get_format(
    _ctx: *mut AVCodecContext,
    pix_fmts: *const AVPixelFormat,
) -> AVPixelFormat {
    unsafe {
        let mut p = pix_fmts;

        while *p != AVPixelFormat::AV_PIX_FMT_NONE {
            if *p == AVPixelFormat::AV_PIX_FMT_VAAPI {
                return AVPixelFormat::AV_PIX_FMT_VAAPI;
            }

            p = p.add(1);
        }

        AVPixelFormat::AV_PIX_FMT_NONE
    }
}

pub struct DecodedFrame {
    pub fd: i32,
    pub width: i32,
    pub height: i32,
    pub format: DrmFourcc,
    pub modifier: u64,
    pub planes: Vec<DrmPlane>,
    pub pts: i64,
}

pub struct VAAPIDecoderParams {
    pub width: u32,
    pub height: u32,
}

pub struct VAAPIDecoder {
    decoder: *mut AVCodecContext,
    parser: *mut AVCodecParserContext,
    _device: GPUDevice,

    hw_frame: *mut AVFrame,
    drm_frame: *mut AVFrame,
    packet: *mut AVPacket,

    pub frame_queue: VecDeque<DecodedFrame>,
}

impl Drop for VAAPIDecoder {
    fn drop(&mut self) {
        unsafe {
            avcodec_free_context(&raw mut self.decoder);
            av_parser_close(self.parser);
            av_frame_free(&raw mut self.hw_frame);
            av_frame_free(&raw mut self.drm_frame);
            av_packet_free(&raw mut self.packet);
        }
    }
}

impl VAAPIDecoder {
    pub fn new(params: VAAPIDecoderParams) -> Self {
        unsafe {
            let codec = avcodec_find_decoder(ffmpeg_next::ffi::AVCodecID::AV_CODEC_ID_H264);
            assert!(!codec.is_null(), "Failed to find H.264 decoder");

            let ctx = avcodec_alloc_context3(codec);
            assert!(!ctx.is_null(), "Failed to allocate decoder context");

            let device = GPUDevice::new().expect("Failed to open GPU Device");
            (*ctx).hw_device_ctx = device.clone().into_raw();
            (*ctx).get_format = Some(vaapi_get_format);
            (*ctx).width = params.width as i32;
            (*ctx).height = params.height as i32;

            let err = avcodec_open2(ctx, codec, std::ptr::null_mut());
            assert!(err >= 0, "Failed to open H.264 decoder: {err}");

            let parser = av_parser_init(
                ffmpeg_next::ffi::AVCodecID::AV_CODEC_ID_H264 as c_int,
            );
            assert!(!parser.is_null(), "Failed to init H.264 parser");

            let hw_frame = av_frame_alloc();
            assert!(!hw_frame.is_null(), "Failed to allocate hw_frame");

            let drm_frame = av_frame_alloc();
            assert!(!drm_frame.is_null(), "Failed to allocate drm_frame");

            let packet = av_packet_alloc();
            assert!(!packet.is_null(), "Failed to allocate packet");

            Self {
                decoder: ctx,
                parser,
                _device: device,
                hw_frame,
                drm_frame,
                packet,
                frame_queue: VecDeque::new(),
            }
        }
    }

    pub fn decode(&mut self, data: &[u8]) {
        unsafe {
            let mut offset = 0usize;

            while offset < data.len() {
                let mut poutbuf: *mut u8 = std::ptr::null_mut();
                let mut poutbuf_size: c_int = 0;

                let consumed = av_parser_parse2(
                    self.parser,
                    self.decoder,
                    &raw mut poutbuf,
                    &raw mut poutbuf_size,
                    data.as_ptr().add(offset),
                    (data.len() - offset) as c_int,
                    ffmpeg_next::ffi::AV_NOPTS_VALUE,
                    ffmpeg_next::ffi::AV_NOPTS_VALUE,
                    0,
                );

                assert!(consumed >= 0, "Parser error: {consumed}");
                offset += consumed as usize;

                if poutbuf_size > 0 {
                    (*self.packet).data = poutbuf;
                    (*self.packet).size = poutbuf_size;

                    let err = avcodec_send_packet(self.decoder, self.packet);
                    if err < 0 {
                        panic!("Failed to send packet to decoder: {err}");
                    }

                    self.receive_frames();
                }
            }
        }
    }

    unsafe fn receive_frames(&mut self) {
        unsafe {
            loop {
                let ret = avcodec_receive_frame(self.decoder, self.hw_frame);
                if ret == -EAGAIN || ret == -(ffmpeg_next::ffi::AVERROR_EOF as i32) {
                    break;
                }
                assert!(ret >= 0, "Decoder error: {ret}");

                let pts = (*self.hw_frame).pts;

                // Map VAAPI surface to DRM PRIME (zero-copy), reusing drm_frame
                av_frame_unref(self.drm_frame);
                (*self.drm_frame).format = AVPixelFormat::AV_PIX_FMT_DRM_PRIME as i32;

                let flags = AV_HWFRAME_MAP_READ as i32 | AV_HWFRAME_MAP_DIRECT as i32;
                let err = av_hwframe_map(self.drm_frame, self.hw_frame, flags);
                if err < 0 {
                    av_frame_unref(self.hw_frame);
                    panic!("Failed to map VAAPI frame to DRM PRIME: {err}");
                }

                // Extract DRM descriptor
                let desc = (*self.drm_frame).data[0] as *const AVDRMFrameDescriptor;
                assert!(!desc.is_null(), "DRM descriptor is null");
                assert!((*desc).nb_objects > 0, "No DRM objects");

                let fd = (*desc).objects[0].fd;
                let modifier = (*desc).objects[0].format_modifier;
                let layer = &(*desc).layers[0];
                let format = DrmFourcc::try_from(layer.format)
                    .expect("Unknown DRM format from decoded frame");

                let mut planes = Vec::with_capacity(layer.nb_planes as usize);
                for i in 0..layer.nb_planes as usize {
                    planes.push(DrmPlane {
                        offset: layer.planes[i].offset,
                        stride: layer.planes[i].pitch,
                    });
                }

                let decoded = DecodedFrame {
                    fd,
                    width: (*self.drm_frame).width,
                    height: (*self.drm_frame).height,
                    format,
                    modifier,
                    planes,
                    pts,
                };

                av_frame_unref(self.hw_frame);
                self.frame_queue.push_back(decoded);
            }
        }
    }
}
