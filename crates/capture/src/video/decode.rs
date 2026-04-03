use std::{collections::VecDeque, ffi::c_int};

use drm_fourcc::DrmFourcc;
use ffmpeg_next::{
    Frame, codec, decoder,
    ffi::{
        AV_HWFRAME_MAP_DIRECT, AV_HWFRAME_MAP_READ, AVCodecContext, AVCodecParserContext,
        AVDRMFrameDescriptor, AVFrame, AVPacket, AVPixelFormat, EAGAIN, av_frame_alloc,
        av_frame_free, av_frame_unref, av_hwframe_map, av_packet_alloc, av_packet_free,
        av_parser_close, av_parser_init, av_parser_parse2, avcodec_alloc_context3,
        avcodec_find_decoder, avcodec_free_context, avcodec_open2, avcodec_receive_frame,
        avcodec_send_packet,
    },
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

#[derive(Debug)]
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
    _device: GPUDevice,

    decoder: decoder::Video,
    parser: *mut AVCodecParserContext,

    hw_frame: Frame,
    drm_frame: Frame,

    packet: *mut AVPacket,

    pub frame_queue: VecDeque<DecodedFrame>,
}

impl Drop for VAAPIDecoder {
    fn drop(&mut self) {
        unsafe {
            av_parser_close(self.parser);
            av_packet_free(&raw mut self.packet);
        }
    }
}

impl VAAPIDecoder {
    pub fn new(params: VAAPIDecoderParams) -> Self {
        let codec = decoder::find(codec::Id::H264).unwrap();

        let mut decoder = codec::Context::new_with_codec(codec).decoder();
        let device = GPUDevice::new().expect("Failed to open GPU Device");

        unsafe {
            let ctx = decoder.as_mut_ptr();

            (*ctx).hw_device_ctx = device.clone().into_raw();
            (*ctx).width = params.width as i32;
            (*ctx).height = params.height as i32;

            (*ctx).get_format = Some(vaapi_get_format);
        }

        let decoder = decoder.video().expect("Failed to open the decoder");

        unsafe {
            let parser = av_parser_init(codec::Id::H264 as i32);
            assert!(!parser.is_null(), "Failed to init H.264 parser");

            let packet = av_packet_alloc();
            assert!(!packet.is_null(), "Failed to allocate packet");

            Self {
                _device: device,

                decoder,
                parser,
                hw_frame: Frame::empty(),
                drm_frame: Frame::empty(),
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
                    self.decoder.as_mut_ptr(),
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

                    let err = avcodec_send_packet(self.decoder.as_mut_ptr(), self.packet);
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
                let ret =
                    avcodec_receive_frame(self.decoder.as_mut_ptr(), self.hw_frame.as_mut_ptr());
                if ret == -EAGAIN || ret == -(ffmpeg_next::ffi::AVERROR_EOF as i32) {
                    break;
                }
                assert!(ret >= 0, "Decoder error: {ret}");

                // Map VAAPI surface to DRM PRIME (zero-copy), reusing drm_frame
                av_frame_unref(self.drm_frame.as_mut_ptr());
                (*self.drm_frame.as_mut_ptr()).format = AVPixelFormat::AV_PIX_FMT_DRM_PRIME as i32;

                let flags = AV_HWFRAME_MAP_READ as i32 | AV_HWFRAME_MAP_DIRECT as i32;
                let err =
                    av_hwframe_map(self.drm_frame.as_mut_ptr(), self.hw_frame.as_ptr(), flags);
                if err < 0 {
                    av_frame_unref(self.hw_frame.as_mut_ptr());

                    panic!("Failed to map VAAPI frame to DRM frame: {err}");
                }

                // Extract DRM descriptor
                let desc = (*self.drm_frame.as_mut_ptr()).data[0] as *const AVDRMFrameDescriptor;
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
                    width: (*self.drm_frame.as_ptr()).width,
                    height: (*self.drm_frame.as_ptr()).height,
                    format,
                    modifier,
                    planes,
                    pts: self.hw_frame.pts().unwrap_or(-1),
                };

                av_frame_unref(self.hw_frame.as_mut_ptr());
                self.frame_queue.push_back(decoded);
            }
        }
    }
}
