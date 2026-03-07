use std::{
    ffi::{CString, c_int, c_uint, c_void},
    marker::PhantomData,
    ptr,
};

use ffmpeg_next::{
    Rational, codec,
    encoder::{self, Encoder, video},
    ffi::{
        AV_OPT_SEARCH_CHILDREN, AVBufferRef, AVBufferSrcParameters, AVFilter, AVFilterContext,
        AVFilterGraph, AVFilterInOut, AVHWFramesContext, AVOptionType, AVPixelFormat,
        av_buffer_ref, av_buffer_unref, av_buffersink_get_hw_frames_ctx,
        av_buffersrc_parameters_alloc, av_buffersrc_parameters_set, av_free,
        av_hwdevice_ctx_create, av_hwframe_ctx_alloc, av_hwframe_ctx_init, av_opt_set_array,
        av_strdup, avfilter_free, avfilter_get_by_name, avfilter_graph_alloc,
        avfilter_graph_alloc_filter, avfilter_graph_config, avfilter_graph_free,
        avfilter_graph_parse_ptr, avfilter_init_str, avfilter_inout_alloc, avfilter_inout_free,
    },
    filter,
    format::Pixel,
};

use crate::video::ScreenSurface;

pub struct EncoderParams {
    pub height: u32,
    pub width: u32,
}

pub struct GPUDevice(*mut AVBufferRef);

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

pub struct Filter {
    ptr: *const AVFilter,
    ctx: *mut AVFilterContext,

    is_committed: bool,
}

// impl<'a> Drop for Filter<'a> {
//     fn drop(&mut self) {
//         unsafe {
//             avfilter_free(self.ctx);
//         }
//     }
// }

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
                    is_committed: false,
                })
            }
        }
    }

    fn commit_to_graph(&mut self) -> Option<()> {
        if self.is_committed {
            return None;
        }

        let err = unsafe { avfilter_init_str(self.ctx, ptr::null()) };

        if err < 0 {
            None
        } else {
            self.is_committed = true;

            Some(())
        }
    }
}

struct BufferFilterBuilder {
    filter: Filter,
    params: *mut AVBufferSrcParameters,
}

impl<'a> BufferFilterBuilder {
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

impl<'a> BufferSinkFilterBuilder {
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

pub struct Graph(*mut AVFilterGraph);

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

    fn config(&self) -> Option<()> {
        unsafe {
            let err = avfilter_graph_config(self.0, ptr::null_mut());

            if err < 0 { None } else { Some(()) }
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

    fn create_buffer_filter(
        &self,
        node_name: &str,
        f: impl FnOnce(BufferFilterBuilder) -> BufferFilterBuilder,
    ) -> Option<Filter> {
        let filter = self.alloc_filter_by_name("buffer", node_name)?;

        let buffer_filter = BufferFilterBuilder::new(filter)?;
        let mut filter = f(buffer_filter).build()?;
        filter.commit_to_graph();

        Some(filter)
    }

    fn create_buffersink_filter<'a>(
        &self,
        node_name: &str,
        f: impl FnOnce(BufferSinkFilterBuilder) -> BufferSinkFilterBuilder,
    ) -> Option<Filter> {
        let filter = self.alloc_filter_by_name("buffersink", node_name)?;

        let filter = BufferSinkFilterBuilder { filter };
        let mut filter = f(filter).build();
        filter.commit_to_graph();

        Some(filter)
    }
}

pub struct Parser<'a> {
    graph: &'a Graph,
    inputs: *mut AVFilterInOut,
    outputs: *mut AVFilterInOut,

    gpu_device: Option<GPUDevice>,
}

impl<'a> Parser<'a> {
    pub fn new(graph: &Graph) -> Parser<'_> {
        Parser {
            graph,
            inputs: ptr::null_mut(),
            outputs: ptr::null_mut(),
            gpu_device: None,
        }
    }

    pub fn input(mut self, name: &str, filter: &Filter, pad: usize) -> Self {
        unsafe {
            let input = avfilter_inout_alloc();
            if input.is_null() {
                panic!("out of memory");
            }

            let name = CString::new(name).unwrap();

            (*input).name = av_strdup(name.as_ptr());
            (*input).filter_ctx = filter.ctx;
            (*input).pad_idx = pad as c_int;
            (*input).next = ptr::null_mut();

            if self.inputs.is_null() {
                self.inputs = input;
            } else {
                (*self.inputs).next = input;
            }
        }

        self
    }

    pub fn output(mut self, name: &str, filter: &Filter, pad: usize) -> Self {
        unsafe {
            let output = avfilter_inout_alloc();

            if output.is_null() {
                panic!("out of memory");
            }

            let name = CString::new(name).unwrap();

            (*output).name = av_strdup(name.as_ptr());
            (*output).filter_ctx = filter.ctx;
            (*output).pad_idx = pad as c_int;
            (*output).next = ptr::null_mut();

            if self.outputs.is_null() {
                self.outputs = output;
            } else {
                (*self.outputs).next = output;
            }
        }

        self
    }

    pub fn with_gpu_device(mut self, gpu_device: GPUDevice) -> Self {
        self.gpu_device = Some(gpu_device);
        self
    }

    pub fn parse(mut self, spec: &str) -> Option<()> {
        unsafe {
            let spec = CString::new(spec).unwrap();

            let result = avfilter_graph_parse_ptr(
                self.graph.0,
                spec.as_ptr(),
                &mut self.inputs,
                &mut self.outputs,
                ptr::null_mut(),
            );

            avfilter_inout_free(&mut self.inputs);
            avfilter_inout_free(&mut self.outputs);

            match result {
                n if n >= 0 => {
                    // Filters that create HW frames ('hwupload', 'hwmap', ...) need
                    // AVBufferRef in their hw_device_ctx. Unfortunately, there is no
                    // simple API to do that for filters created by avfilter_graph_parse_ptr().
                    // The code below is inspired by wf-recorder
                    if let Some(device) = self.gpu_device {
                        for i in 0..(*self.graph.0).nb_filters {
                            let item = *(*self.graph.0).filters.add(i as usize);

                            (*item).hw_device_ctx = device.clone().into_raw();
                        }
                    }

                    Some(())
                }
                _ => None,
            }
        }
    }
}

pub struct VAAPIEncoder {
    encoder: codec::encoder::video::Encoder,
    graph: Graph,

    sink_filter: Filter,
    source_filter: Filter,
}

impl VAAPIEncoder {
    pub fn encode(&self, surface: ScreenSurface) {}

    pub fn new(params: EncoderParams) -> Self {
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

        // TODO: Make a safe wrapper for that, I am feeling a bit lazy
        // for this right now
        unsafe {
            // The (input of the) sink is the output of the whole filter.
            let filter_output = *(*sink_filter.ctx).inputs;

            video.set_width((*filter_output).w as u32);
            video.set_height((*filter_output).h as u32);

            (*video.as_mut_ptr()).pix_fmt =
                std::mem::transmute::<i32, AVPixelFormat>((*filter_output).format);
            (*video.as_mut_ptr()).hw_frames_ctx =
                av_buffer_ref(av_buffersink_get_hw_frames_ctx(sink_filter.ctx));

            video.set_time_base((*filter_output).time_base);
            video.set_frame_rate(Some(Rational(1, 0)));
            video.set_aspect_ratio((*filter_output).sample_aspect_ratio);
        }

        let encoder = video.open().expect("Failed to open the codec");

        Self {
            encoder,
            sink_filter,
            source_filter,
            graph,
        }
    }
}
