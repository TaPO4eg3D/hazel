use std::{collections::VecDeque, ffi::c_int};

use drm_fourcc::{DrmFormat, DrmFourcc, DrmModifier};
use ffmpeg_next::{
    Frame, codec, decoder,
    ffi::{
        AV_HWFRAME_MAP_DIRECT, AV_HWFRAME_MAP_READ, AVCodecContext, AVCodecParserContext,
        AVDRMFrameDescriptor, AVPacket, AVPixelFormat, EAGAIN, av_frame_unref, av_hwframe_map,
        av_packet_alloc, av_packet_free, av_parser_close, av_parser_init, av_parser_parse2,
        avcodec_receive_frame, avcodec_send_packet,
    },
};
use gpui::{DMABuffer, DMABufferPlane};
use smallvec::SmallVec;

use crate::video::wrapper::GPUDevice;

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

pub struct VAAPIDecoderParams {
    pub width: u32,
    pub height: u32,
}

pub struct VAAPIDecoder {
    _device: GPUDevice,

    decoder: decoder::Video,
    parser: *mut AVCodecParserContext,

    hw_frame: Frame,

    drm_idx: usize,
    drm_frames: Vec<Frame>,

    packet: *mut AVPacket,

    pub frame_queue: VecDeque<DMABuffer>,
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
    const DRM_FRAME_POOL_SIZE: usize = 12;

    pub fn new(params: VAAPIDecoderParams) -> Self {
        let codec = decoder::find(codec::Id::H264).unwrap();

        let mut decoder = codec::Context::new_with_codec(codec).decoder();
        let device = GPUDevice::new().expect("Failed to open GPU Device");

        unsafe {
            let ctx = decoder.as_mut_ptr();

            (*ctx).hw_device_ctx = device.clone().into_raw();
            (*ctx).width = params.width as i32;
            (*ctx).height = params.height as i32;
            (*ctx).sw_pix_fmt = AVPixelFormat::AV_PIX_FMT_NV12;

            (*ctx).get_format = Some(vaapi_get_format);
        }

        let decoder = decoder.video().expect("Failed to open the decoder");

        unsafe {
            let parser = av_parser_init(codec::Id::H264 as i32);
            assert!(!parser.is_null(), "Failed to init H.264 parser");

            let packet = av_packet_alloc();
            assert!(!packet.is_null(), "Failed to allocate packet");

            // Like in Pipewire, we're maintaining a pool of
            // DRM Frames to not invalidate a frame we're display
            let drm_frames = (0..Self::DRM_FRAME_POOL_SIZE)
                .map(|_| Frame::empty())
                .collect::<Vec<_>>();

            Self {
                _device: device,
                decoder,
                parser,
                hw_frame: Frame::empty(),
                drm_idx: 0,
                drm_frames,
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
                        // TODO: It should not be a plain return. Some errors
                        // carry a usefull context: like if we missed a keyframe,
                        // we should ask the host to generate a new one
                        return;
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
                if ret == -EAGAIN || ret == -ffmpeg_next::ffi::AVERROR_EOF {
                    break;
                }
                assert!(ret >= 0, "Decoder error: {ret}");

                // Map VAAPI surface to DRM PRIME (zero-copy), reusing drm_frame
                let drm_frame = &mut self.drm_frames[self.drm_idx];

                av_frame_unref(drm_frame.as_mut_ptr());
                (*drm_frame.as_mut_ptr()).format = AVPixelFormat::AV_PIX_FMT_DRM_PRIME as i32;

                let flags = AV_HWFRAME_MAP_READ as i32 | AV_HWFRAME_MAP_DIRECT as i32;
                let err = av_hwframe_map(drm_frame.as_mut_ptr(), self.hw_frame.as_ptr(), flags);
                if err < 0 {
                    av_frame_unref(self.hw_frame.as_mut_ptr());

                    panic!("Failed to map VAAPI frame to DRM frame: {err}");
                }

                // Extract DRM descriptor
                let desc = (*drm_frame.as_mut_ptr()).data[0] as *const AVDRMFrameDescriptor;
                assert!(!desc.is_null(), "DRM descriptor is null");
                assert!((*desc).nb_objects > 0, "No DRM objects");

                let objects = (*desc).objects;
                let objects = &objects[..(*desc).nb_objects as usize];

                let layers = (*desc).layers;
                let layers = &layers[..(*desc).nb_layers as usize];

                // NOTE: Technically it's not correct since different
                // layers might be on different DMA-BUFs but it should work fine???
                let modifier = objects[0].format_modifier;
                let _format = DrmFourcc::try_from(layers[0].format) // Check TODO below in DrmFormat struct
                    .expect("Unknown DRM format from decoded frame");

                let mut planes: SmallVec<[DMABufferPlane; 2]> = SmallVec::new();
                for layer in layers {
                    let layer_planes = &layer.planes[..layer.nb_planes as usize];

                    for plane in layer_planes {
                        planes.push(DMABufferPlane {
                            offset: plane.offset as usize,
                            stride: plane.pitch as usize,
                        });
                    }
                }

                let fd = objects[0].fd;

                let width = (*drm_frame.as_ptr()).width as u32;
                let height = (*drm_frame.as_ptr()).height as u32;

                let decoded = DMABuffer::new(
                    fd,
                    width,
                    height,
                    DrmFormat {
                        // TODO: Format from layer is PER layer,
                        // figure out how to handle it. For now it's fine
                        // to hardcode, since VAAPI is almost always NV12
                        code: DrmFourcc::Nv12,
                        modifier: DrmModifier::from(modifier),
                    },
                    &planes,
                );

                av_frame_unref(self.hw_frame.as_mut_ptr());
                self.frame_queue.push_back(decoded);

                self.drm_idx = (self.drm_idx + 1) % self.drm_frames.len();
            }
        }
    }
}
