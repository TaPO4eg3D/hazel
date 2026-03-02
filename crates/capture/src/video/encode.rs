use std::{
    ffi::{CString, c_uint, c_void},
    ptr,
};

use ffmpeg_next::{
    Rational, codec, encoder,
    ffi::{
        AV_OPT_SEARCH_CHILDREN, AVBufferRef, AVBufferSrcParameters, AVFilter, AVFilterContext,
        AVFilterGraph, AVHWFramesContext, AVOptionType, AVPixelFormat, av_buffer_ref,
        av_buffer_unref, av_buffersrc_parameters_alloc, av_buffersrc_parameters_set, av_free,
        av_hwdevice_ctx_create, av_hwframe_ctx_alloc, av_hwframe_ctx_init, av_opt_set_array,
        avfilter_free, avfilter_get_by_name, avfilter_graph_alloc, avfilter_graph_alloc_filter,
        avfilter_graph_free, avfilter_init_str,
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

    unsafe fn into_raw(self) -> *mut AVBufferRef {
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

impl HWFrameContext {
    unsafe fn into_raw(self) -> *mut AVBufferRef {
        let ptr = self.0;
        std::mem::forget(self);

        ptr
    }
}

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

struct Filter {
    ptr: *const AVFilter,
    ctx: *mut AVFilterContext,
}

impl Drop for Filter {
    fn drop(&mut self) {
        unsafe {
            avfilter_free(self.ctx);
        }
    }
}

impl Filter {
    fn find(name: &str) -> Option<Self> {
        let name = CString::new(name).unwrap();

        unsafe {
            let ptr = avfilter_get_by_name(name.as_ptr());

            if ptr.is_null() {
                None
            } else {
                Some(Self {
                    ptr,
                    ctx: std::ptr::null_mut(),
                })
            }
        }
    }

    fn commit_to_graph(&self) -> Option<()> {
        let err = unsafe { avfilter_init_str(self.ctx, ptr::null()) };
        if err < 0 { None } else { Some(()) }
    }
}

struct BufferFilterBuilder {
    filter: Filter,
    params: *mut AVBufferSrcParameters,
}

impl BufferFilterBuilder {
    fn new(filter: Filter) -> Option<Self> {
        unsafe {
            let params = av_buffersrc_parameters_alloc();

            if params.is_null() {
                None
            } else {
                Some(Self { filter, params })
            }
        }
    }

    fn build(self) -> Option<Filter> {
        let ctx = self.filter.ctx;

        let err = unsafe { av_buffersrc_parameters_set(ctx, self.params) };
        let value = if err < 0 { None } else { Some(self.filter) };

        unsafe {
            av_free(self.params as *mut _);
        }

        value
    }

    fn set_width(self, width: i32) -> Self {
        unsafe {
            (*self.params).width = width;
        }

        self
    }

    fn set_height(self, height: i32) -> Self {
        unsafe {
            (*self.params).height = height;
        }

        self
    }

    fn set_time_base(self, value: Rational) -> Self {
        unsafe {
            (*self.params).time_base = value.into();
        }

        self
    }

    fn set_aspect_ratio(self, value: Rational) -> Self {
        unsafe {
            (*self.params).sample_aspect_ratio = value.into();
        }

        self
    }

    fn set_format(self, format: AVPixelFormat) -> Self {
        unsafe {
            (*self.params).format = format as i32;
        }

        self
    }

    fn set_hw_frame_ctx(self, ctx: HWFrameContext) -> Self {
        unsafe {
            (*self.params).hw_frames_ctx = ctx.into_raw();
        }

        self
    }
}

struct BufferSinkFilterBuilder {
    filter: Filter,
}

impl BufferSinkFilterBuilder {
    fn build(self) -> Filter {
        self.filter
    }

    fn set_pixel_formats(self, formats: &[AVPixelFormat]) -> Option<Self> {
        let opt_name = CString::new("pixel_formats").unwrap();

        unsafe {
            let err = av_opt_set_array(
                self.filter.ctx as *mut _,
                opt_name.as_ptr(),
                AV_OPT_SEARCH_CHILDREN,
                0,
                formats.len() as c_uint,
                AVOptionType::AV_OPT_TYPE_PIXEL_FMT,
                formats.as_ptr() as *const c_void,
            );

            if err < 0 { None } else { Some(self) }
        }
    }
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

    fn alloc_filter_by_name(&self, filter_name: &str, node_name: &str) -> Option<Filter> {
        let mut filter = Filter::find(filter_name)?;

        let node_name = CString::new(node_name).unwrap();
        unsafe {
            filter.ctx = avfilter_graph_alloc_filter(self.0, filter.ptr, node_name.as_ptr());

            if filter.ctx.is_null() {
                None
            } else {
                Some(filter)
            }
        }
    }

    fn alloc_buffer_filter(&self, node_name: &str) -> Option<BufferFilterBuilder> {
        let filter = self.alloc_filter_by_name("buffer", node_name)?;

        BufferFilterBuilder::new(filter)
    }

    fn alloc_buffersink_filter(&self, node_name: &str) -> Option<BufferSinkFilterBuilder> {
        let filter = self.alloc_filter_by_name("buffersink", node_name)?;

        Some(BufferSinkFilterBuilder { filter })
    }
}

pub struct VideoEncoder {}

impl VideoEncoder {
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

        let graph = Graph::new().expect("Failed ot alloc filter graph");

        let source_filter = graph
            .alloc_buffer_filter("Source")
            .expect("Failed to alloc BufferFilter params")
            .set_format(AVPixelFormat::AV_PIX_FMT_VAAPI)
            .set_hw_frame_ctx(hw_frame_ctx.clone())
            .set_width(params.width as i32)
            .set_height(params.height as i32)
            .set_time_base(time_base)
            .set_aspect_ratio(Rational(1, 1))
            .build()
            .expect("Failed to set BufferFilter params");
        source_filter.commit_to_graph();

        let sink_filter = graph
            .alloc_buffersink_filter("Sink")
            .expect("Failed to alloc `buffersink` filter")
            .set_pixel_formats(&[AVPixelFormat::AV_PIX_FMT_VAAPI])
            .expect("Failed to set Pixel Formats")
            .build();
        sink_filter.commit_to_graph();

        video.set_width(params.height);
        video.set_height(params.height);
        video.set_time_base(time_base);

        // Self::add_source_filter(&params, &hw_frame_ctx, &mut graph);
        // Self::add_sink_filter(&params, &mut graph);

        // video.open().expect("Failed to open the codec");
    }
}
