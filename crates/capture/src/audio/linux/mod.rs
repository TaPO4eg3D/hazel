use std::collections::VecDeque;

use anyhow::Result as AResult;
use libspa::param::{
    ParamType,
    audio::{AudioFormat, AudioInfoRaw},
    format::{MediaSubtype, MediaType},
    format_utils,
};
use pipewire::{
    self as pw,
    properties::properties,
    spa::{self, pod::Pod},
    stream::{Stream, StreamBox, StreamListener},
};
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::*};

use ffmpeg::codec::encoder;
use ffmpeg::{ChannelLayout, format};
use ffmpeg_next::{self as ffmpeg, Packet, codec, frame};

pub const DEFAULT_RATE: u32 = 48000;
pub const DEFAULT_CHANNELS: u32 = 2;

struct CaptureStreamData {
    format: AudioInfoRaw,
    enable_loopback: bool,

    encoder: AudioEncoder,

    /// Ring Buffer that is used to loopback captured audio.
    /// Mainly used to quickly test how your microphone sounds
    /// TODO: Change RingBuffer type since both Capture and Playback live
    /// in the same thread
    loopback_producer: HeapProd<f32>,
}

struct CaptureStream<'a> {
    stream: StreamBox<'a>,
    stream_listener: StreamListener<CaptureStreamData>,
}

struct AudioDecoder {
    codec: codec::audio::Audio,
    decoder: codec::decoder::Audio,

    decoded_frame: frame::Audio,
    decoded_frames_queue: VecDeque<f32>,
}

impl AudioDecoder {
    pub fn new() -> Self {
        let codec = codec::decoder::find(ffmpeg::codec::Id::OPUS).expect("Opus codec not found");
        let context = ffmpeg::codec::context::Context::new_with_codec(codec);

        let codec = codec.audio().unwrap();

        let mut decoder = context.decoder().audio().unwrap();
        decoder.set_channel_layout(ChannelLayout::STEREO);

        Self {
            codec,
            decoder,

            decoded_frame: frame::Audio::empty(),
            decoded_frames_queue: VecDeque::new(),
        }
    }

    fn decode(&mut self, packet: Packet) {
        self.decoder.send_packet(&packet)
            .unwrap();

        while self.decoder.receive_frame(&mut self.decoded_frame).is_ok() {
            let channels = self.decoded_frame.channels();
            let format = self.decoded_frame.format();

            let is_planar = format == format::Sample::F32(format::sample::Type::Planar);

            match (is_planar, channels)  {
                (true, 2) => {
                    // Convert into F32::Packed
                    let left = self.decoded_frame.plane::<f32>(0);
                    let right = self.decoded_frame.plane::<f32>(1);

                    for (l, r) in left.iter().zip(right.iter()) {
                        self.decoded_frames_queue.push_back(*l);
                        self.decoded_frames_queue.push_back(*r);
                    }
                },
                (false, 2) => {
                    // Already Packed stereo
                    // TODO: Fix, it won't work because of the bug in `ffpeg-next`,
                    // it does not account for channels when stereo is packed
                    let data = self.decoded_frame.plane::<f32>(0);

                    self.decoded_frames_queue.extend(data)
                },
                (_, 1) => {
                    // Convert mono to stereo by duplicating
                    let data = self.decoded_frame.plane::<f32>(0);

                    for sample in data {
                        self.decoded_frames_queue.push_back(*sample);
                        self.decoded_frames_queue.push_back(*sample);
                    }
                },
                _ => unimplemented!("Unhandled case: {:?}", (is_planar, channels)),
            }
        }
    }
}

trait VecDequeExt<T> {
    /// Fill the passed buffer with content from the Deque.
    /// If `partial` is set to:
    ///     - true: the function tries to fill as much as possible
    ///     - false: the function returns immediately if the Deque has not enough data
    ///
    /// Return: how much items are copied to the passed buffer
    fn pop_slice(&mut self, out: &mut [T], partial: bool) -> usize;
}

impl<T> VecDequeExt<T> for VecDeque<T> {
    #[inline(always)]
    fn pop_slice(&mut self, out: &mut [T], partial: bool) -> usize {
        if !partial && self.len() < out.len() {
            return 0;
        }

        let length = self.len().min(out.len());
        (0..length).for_each(|idx| {
            out[idx] = self.pop_front().unwrap();
        });

        length
    }
}

/// To simulate Packet over the network
#[derive(Debug)]
struct PacketPayload {
    dts: Option<i64>,
    pts: Option<i64>,

    flags: i32,
    data: Vec<u8>,
}

impl From<PacketPayload> for Packet {
    fn from(val: PacketPayload) -> Self {
        let mut packet = Packet::new(val.data.len());

        packet.set_dts(val.dts);
        packet.set_pts(val.pts);

        packet.set_flags(codec::packet::Flags::from_bits_truncate(val.flags));
        let data = packet
            .data_mut()
            .expect("Should be present because Packet::new");

        data.copy_from_slice(&val.data);

        packet
    }
}

impl From<&Packet> for PacketPayload {
    fn from(packet: &Packet) -> Self {
        Self {
            dts: packet.dts(),
            pts: packet.pts(),

            flags: packet.flags().bits(),
            data: packet.data().unwrap_or_default().to_vec(),
        }
    }
}

struct AudioEncoder {
    codec: codec::audio::Audio,
    encoder: encoder::audio::Encoder,

    raw_frame: frame::audio::Audio,
    encoded_packet: Packet,

    frame_queue: VecDeque<f32>,

    encoded_producer: HeapProd<PacketPayload>,
    fill_encoded_buffer: bool,

    pts_counter: i64,
}

impl AudioEncoder {
    fn new(encoded_producer: HeapProd<PacketPayload>) -> Self {
        let codec = encoder::find(ffmpeg::codec::Id::OPUS).expect("Opus codec not found");
        let context = ffmpeg::codec::context::Context::new_with_codec(codec);

        let codec = codec.audio().unwrap();

        let mut encoder = context.encoder().audio().unwrap();

        encoder.set_rate(DEFAULT_RATE as i32);
        encoder.set_channel_layout(ChannelLayout::STEREO);
        encoder.set_format(format::Sample::F32(format::sample::Type::Packed));

        encoder.set_bit_rate(128000);
        encoder.set_time_base((1, DEFAULT_RATE as i32));

        let encoder = encoder.open_as(codec).unwrap();

        // Just a note for myself, in case I forget that shit again:
        // `frame_size` (means number of samples **PER** channel)
        let frame_size = encoder.frame_size() as usize;

        Self {
            codec,
            encoder,
            raw_frame: frame::audio::Audio::new(
                format::Sample::F32(format::sample::Type::Packed),
                frame_size,
                ChannelLayout::STEREO,
            ),
            encoded_producer,
            fill_encoded_buffer: true,
            pts_counter: 0,
            encoded_packet: Packet::empty(),
            frame_queue: VecDeque::new(),
        }
    }

    fn encode(&mut self, samples: &[f32]) {
        self.frame_queue.extend(samples);

        loop {
            // Because of a bug in `ffpeg-next`, it does not account
            // for channels when we have packed audio
            let plane = unsafe {
                std::slice::from_raw_parts_mut(
                    (*self.raw_frame.as_mut_ptr())
                        .data[0] as *mut f32,
                    self.raw_frame.samples() * self.raw_frame.channels() as usize,
                )
            };

            if self.frame_queue.pop_slice(plane, false) == 0 {
                break;
            }

            self.raw_frame.set_pts(Some(self.pts_counter));
            self.encoder.send_frame(&self.raw_frame).unwrap();

            let (new_pts, _) = self.pts_counter.overflowing_add(
                self.encoder.frame_size() as i64,
            );

            self.pts_counter = new_pts;

            while self
                .encoder
                .receive_packet(&mut self.encoded_packet)
                .is_ok()
            {
                if self.fill_encoded_buffer {
                    if self.encoded_packet.data().unwrap_or_default().is_empty() {
                        continue;
                    }

                    self.encoded_producer
                        .try_push((&self.encoded_packet).into())
                        .unwrap()
                }
            }
        }
    }
}

impl<'a> CaptureStream<'a> {
    const STREAM_NAME: &'static str = "HAZEL Audio Capture";

    fn on_param_change(
        _stream: &Stream,
        user_data: &mut CaptureStreamData,
        id: u32,
        param: Option<&libspa::pod::Pod>,
    ) {
        let Some(param) = param else { return };
        if id != ParamType::Format.as_raw() {
            return;
        }

        let (media_type, media_subtype) = match format_utils::parse_format(param) {
            Ok(v) => v,
            Err(_) => return,
        };

        if media_type != MediaType::Audio || media_subtype != MediaSubtype::Raw {
            return;
        }

        let _ = user_data.format.parse(param);
    }

    fn on_process(stream: &Stream, user_data: &mut CaptureStreamData) {
        let Some(mut buffer) = stream.dequeue_buffer() else {
            return;
        };

        let datas = buffer.datas_mut();
        if datas.is_empty() {
            return;
        }

        let data = &mut datas[0];

        let chunk = data.chunk();
        let size = chunk.size() as usize;
        let offset = chunk.offset() as usize;

        if size == 0 {
            return;
        }

        if let Some(slice) = data.data()
            && offset + size <= slice.len()
        {
            let valid_bytes = &slice[offset..offset + size];

            let samples = unsafe {
                std::slice::from_raw_parts(
                    valid_bytes.as_ptr() as *const f32,
                    valid_bytes.len() / size_of::<f32>(),
                )
            };

            user_data.encoder.encode(samples);

            if user_data.enable_loopback {
                user_data.loopback_producer.push_slice(samples);
            }
        }
    }

    fn new(
        core: &'a pw::core::CoreRc,
        loopback_producer: HeapProd<f32>,
        encoded_producer: HeapProd<PacketPayload>,
    ) -> AResult<Self> {
        let capture_stream = pw::stream::StreamBox::new(
            core,
            Self::STREAM_NAME,
            properties! {
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_ROLE => "Communication",
                *pw::keys::MEDIA_CATEGORY => "Capture",
            },
        )?;

        let mut audio_info = spa::param::audio::AudioInfoRaw::new();
        audio_info.set_format(AudioFormat::F32LE);
        audio_info.set_rate(DEFAULT_RATE);
        audio_info.set_channels(DEFAULT_CHANNELS);

        // TODO: There's no point in capturing STEREO channel
        // from the microphone, we're just wasting bandwidth
        let mut position = [0; spa::param::audio::MAX_CHANNELS];
        position[0] = libspa::sys::SPA_AUDIO_CHANNEL_FL;
        position[1] = libspa::sys::SPA_AUDIO_CHANNEL_FR;
        audio_info.set_position(position);

        let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pw::spa::pod::Value::Object(pw::spa::pod::Object {
                type_: libspa::sys::SPA_TYPE_OBJECT_Format,
                id: libspa::sys::SPA_PARAM_EnumFormat,
                properties: audio_info.into(),
            }),
        )
        .unwrap()
        .0
        .into_inner();

        let mut params = [Pod::from_bytes(&values).unwrap()];

        let stream_data = CaptureStreamData {
            format: Default::default(),
            loopback_producer,
            encoder: AudioEncoder::new(encoded_producer),
            enable_loopback: false,
        };

        let listener = capture_stream
            .add_local_listener_with_user_data(stream_data)
            .process(CaptureStream::on_process)
            .param_changed(CaptureStream::on_param_change)
            .register()?;

        capture_stream.connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )?;

        Ok(Self {
            stream: capture_stream,
            stream_listener: listener,
        })
    }
}

struct PlaybackStreamData {
    /// Ring Buffer that is used to loopback captured audio.
    /// Mainly used to quickly test how your microphone sounds
    loopback_consumer: HeapCons<f32>,
    encoded_consumer: HeapCons<PacketPayload>,

    decoder: AudioDecoder,
}

struct PlaybackStream<'a> {
    stream: StreamBox<'a>,
    stream_listener: StreamListener<PlaybackStreamData>,
}

impl<'a> PlaybackStream<'a> {
    const STREAM_NAME: &'static str = "HAZEL Audio Playback";

    fn on_process(stream: &Stream, user_data: &mut PlaybackStreamData) {
        let Some(mut buffer) = stream.dequeue_buffer() else {
            return;
        };

        let datas = buffer.datas_mut();
        if datas.is_empty() {
            return;
        }
        let data = &mut datas[0];

        let stride = std::mem::size_of::<f32>() * DEFAULT_CHANNELS as usize;

        if let Some(slice) = data.data() {
            // Decode all queued encoded packets
            while let Some(packet) = user_data.encoded_consumer.try_pop() {
                user_data.decoder.decode(packet.into());
            }

            let output_samples = unsafe {
                std::slice::from_raw_parts_mut(slice.as_mut_ptr() as *mut f32, slice.len() / 4)
            };

            let read_count = user_data
                .decoder
                .decoded_frames_queue
                .pop_slice(output_samples, true);

            // If chunk is not fully filled, fill it with silence
            (read_count..output_samples.len())
                .for_each(|i| output_samples[i] = 0.);

            let chunk = data.chunk_mut();

            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = stride as i32;
            *chunk.size_mut() = (output_samples.len() * 4) as u32;
        }
    }

    fn new(
        core: &'a pw::core::CoreRc,
        loopback_consumer: HeapCons<f32>,
        encoded_consumer: HeapCons<PacketPayload>,
    ) -> AResult<Self> {
        let playback_stream = pw::stream::StreamBox::new(
            core,
            Self::STREAM_NAME,
            properties! {
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_ROLE => "Communication",
                *pw::keys::MEDIA_CATEGORY => "Playback",
                *pw::keys::AUDIO_CHANNELS => "2",
            },
        )?;

        let mut audio_info = spa::param::audio::AudioInfoRaw::new();

        audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
        audio_info.set_rate(DEFAULT_RATE);
        audio_info.set_channels(DEFAULT_CHANNELS);

        let mut position = [0; spa::param::audio::MAX_CHANNELS];
        position[0] = libspa::sys::SPA_AUDIO_CHANNEL_FL;
        position[1] = libspa::sys::SPA_AUDIO_CHANNEL_FR;
        audio_info.set_position(position);

        let user_data = PlaybackStreamData {
            loopback_consumer,
            encoded_consumer,
            decoder: AudioDecoder::new(),
        };

        let listener = playback_stream
            .add_local_listener_with_user_data(user_data)
            .process(Self::on_process)
            .register()?;

        let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pw::spa::pod::Value::Object(pw::spa::pod::Object {
                type_: libspa::sys::SPA_TYPE_OBJECT_Format,
                id: libspa::sys::SPA_PARAM_EnumFormat,
                properties: audio_info.into(),
            }),
        )
        .unwrap()
        .0
        .into_inner();

        let mut params = [Pod::from_bytes(&values).unwrap()];

        playback_stream.connect(
            spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )?;

        Ok(PlaybackStream {
            stream: playback_stream,
            stream_listener: listener,
        })
    }
}

pub struct Audio {}

impl Audio {
    pub fn new() -> AResult<Self> {
        pw::init();
        ffmpeg::init().unwrap();

        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;

        let ring = HeapRb::<f32>::new((DEFAULT_RATE * 2) as usize);
        let (loopback_producer, loopback_consumer) = ring.split();

        let ring = HeapRb::<PacketPayload>::new((DEFAULT_RATE * 2) as usize);
        let (encoded_producer, encoded_consumer) = ring.split();

        let playback = PlaybackStream::new(&core, loopback_consumer, encoded_consumer)?;
        let capture = CaptureStream::new(&core, loopback_producer, encoded_producer)?;

        mainloop.run();

        Ok(Audio {})
    }
}
