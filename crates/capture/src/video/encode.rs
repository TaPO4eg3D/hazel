use std::ffi::CString;

use ffmpeg_next::{
    Rational,
    codec::{self, traits::Encoder},
    encoder,
    ffi::{
        AVBufferRef, AVBufferSrcParameters, AVFilter, AVFilterGraph, AVHWFramesContext,
        AVPixelFormat, av_buffer_ref, av_buffer_unref, av_buffersrc_parameters_alloc,
        av_hwdevice_ctx_create, av_hwframe_ctx_alloc, av_hwframe_ctx_init, avfilter_get_by_name,
        avfilter_graph_alloc, avfilter_graph_free,
    },
    filter,
    format::Pixel,
};

pub struct EncoderParams {
    pub codec_name: &'static str,

    pub height: u32,
    pub width: u32,
}

struct GPUDevice(*mut AVBufferRef);

impl GPUDevice {
    fn new() -> Option<Self> {
        let mut device_context: *mut AVBufferRef = std::ptr::null_mut();

        unsafe {
            let err = av_hwdevice_ctx_create(
                &raw mut device_context,
                ffmpeg_next::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
            );

            let device = GPUDevice(device_context);
            if err < 0 {
                // `device` will be unreffed by impl Drop
                return None;
            }

            Some(device)
        }
    }

    fn as_ptr(&self) -> *mut AVBufferRef {
        self.0
    }

    fn into_raw(self) -> *mut AVBufferRef {
        let ptr = self.0;
        std::mem::forget(self);

        ptr
    }
}

impl Clone for GPUDevice {
    fn clone(&self) -> Self {
        unsafe { GPUDevice(av_buffer_ref(self.0 as *const _)) }
    }
}

impl Drop for GPUDevice {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                av_buffer_unref(&raw mut self.0);
            }
        }
    }
}

struct HWFrameContextBuilder(*mut AVBufferRef);

impl HWFrameContextBuilder {
    fn new(device: &GPUDevice) -> Option<Self> {
        unsafe {
            let frame_ctx = av_hwframe_ctx_alloc(device.as_ptr());

            if frame_ctx.is_null() {
                return None;
            }

            Some(Self(frame_ctx))
        }
    }

    fn build(self) -> Option<HWFrameContext> {
        unsafe {
            let err = av_hwframe_ctx_init(self.0);
            let ctx = HWFrameContext(self.0);

            if err < 0 {
                // `ctx` will be unreffed by impl Drop
                return None;
            }

            Some(ctx)
        }
    }

    fn as_ctx_ptr(&self) -> *mut AVHWFramesContext {
        unsafe { (*self.0).data as *mut AVHWFramesContext }
    }

    fn set_format(self, format: AVPixelFormat) -> Self {
        unsafe {
            (*self.as_ctx_ptr()).format = format;
        }

        self
    }

    fn set_sw_format(self, format: AVPixelFormat) -> Self {
        unsafe {
            (*self.as_ctx_ptr()).sw_format = format;
        }

        self
    }

    fn set_width(self, width: i32) -> Self {
        unsafe {
            (*self.as_ctx_ptr()).width = width;
        }

        self
    }

    fn set_height(self, height: i32) -> Self {
        unsafe {
            (*self.as_ctx_ptr()).height = height;
        }

        self
    }

    fn set_initial_pool_size(self, value: i32) -> Self {
        unsafe {
            (*self.as_ctx_ptr()).initial_pool_size = value;
        }

        self
    }
}

struct HWFrameContext(*mut AVBufferRef);

impl Clone for HWFrameContext {
    fn clone(&self) -> Self {
        unsafe { HWFrameContext(av_buffer_ref(self.0 as *const _)) }
    }
}

impl Drop for HWFrameContext {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                av_buffer_unref(&raw mut self.0);
            }
        }
    }
}

struct Filter(*const AVFilter);

impl Filter {
    fn find(name: &str) -> Option<Self> {
        let name = CString::new(name).unwrap();

        unsafe {
            let ptr = avfilter_get_by_name(name.as_ptr());

            if ptr.is_null() { None } else { Some(Self(ptr)) }
        }
    }
}

struct BufferFilterBuilder {
    filter: Filter,
    params: *mut AVBufferSrcParameters,
}

impl BufferFilterBuilder {
    fn new() -> Option<Self> {
        let filter = Filter::find("buffer")?;

        unsafe {
            let params = av_buffersrc_parameters_alloc();

            if params.is_null() {
                None
            } else {
                Some(Self { filter, params })
            }
        }
    }

    fn set_format(self, format: AVPixelFormat) -> Self {
        unsafe {
            (*self.params).format = format as i32;
        }

        self
    }

    fn set_hw_frame_ctx(self, hw) {}
}

struct Graph(*mut AVFilterGraph);

impl Drop for Graph {
    fn drop(&mut self) {
        unsafe {
            avfilter_graph_free(&raw mut self.0);
        }
    }
}

impl Graph {
    fn new() -> Option<Self> {
        unsafe {
            let ptr = avfilter_graph_alloc();

            if ptr.is_null() { None } else { Some(Self(ptr)) }
        }
    }
}

pub struct VideoEncoder {}

impl VideoEncoder {
    fn add_source_filter(
        params: &EncoderParams,
        hw_frame_ctx: &HWFrameContext,
        graph: &mut filter::Graph,
    ) {
        let time_base = Rational(1, 1000000);

        let filter = BufferFilter::new().expect("Failed to create Source filter");

        let source_filter = filter::find("buffer").unwrap();

        let _source_ctx = graph
            .add(
                &source_filter,
                "Source",
                &format!(
                    "video_size={}x{}:pix_fmt=vaapi:time_base={}/{}:pixel_aspect=1/1",
                    params.width, params.height, time_base.0, time_base.1,
                ),
            )
            .expect("Failed to add source filter");
    }

    fn add_sink_filter(params: &EncoderParams, graph: &mut filter::Graph) {
        let sink_filter = filter::find("buffersink").unwrap();
        let mut sink_ctx = graph
            .add(&sink_filter, "Sink", "")
            .expect("Failed to add sink filter");

        sink_ctx.set_pixel_format(Pixel::VAAPI);
    }

    pub fn new(params: EncoderParams) {
        let codec = encoder::find_by_name(params.codec_name).expect("Failed to find Video Codec");
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

        video.set_width(params.height);
        video.set_height(params.height);
        video.set_time_base(time_base);

        let mut graph = filter::Graph::new();

        Self::add_sink_filter(&params, &mut graph);
        Self::add_source_filter(&params, &hw_ctx, &mut graph);

        video.open().expect("Failed to open the codec");
    }
}
