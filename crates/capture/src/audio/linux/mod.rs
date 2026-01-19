use std::{
    collections::VecDeque,
    sync::{Arc, atomic::AtomicBool},
    thread,
};

use anyhow::Result as AResult;
use libspa::param::{
    ParamType,
    audio::{AudioFormat, AudioInfoRaw},
    format::{MediaSubtype, MediaType},
    format_utils,
};
use nnnoiseless::DenoiseState;
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

use streaming_common::FFMpegPacketPayload;

struct CaptureStreamData {
    encoder: AudioEncoder,
    format: AudioInfoRaw,

    capture: Arc<AtomicBool>,

    enable_loopback: bool,
    /// Ring Buffer that is used to loopback captured audio.
    /// Mainly used to quickly test how your microphone sounds
    /// TODO: Change RingBuffer type since both Capture and Playback live
    /// in the same thread
    loopback_producer: HeapProd<f32>,

    /// Producer of encoded packets
    packet_producer: std::sync::mpsc::Sender<FFMpegPacketPayload>,

    enable_noise_reduction: bool,
    denoise_state: Box<DenoiseState<'static>>,

    rnnoise_queue: VecDeque<f32>,

    rnnoise_in_buff: Vec<f32>,
    rnnoise_out_buff: Vec<f32>,
}

struct CaptureStream<'a> {
    stream: StreamBox<'a>,
    stream_listener: StreamListener<CaptureStreamData>,
}

pub struct AudioDecoder {
    codec: codec::audio::Audio,
    decoder: codec::decoder::Audio,

    packet_consumer: std::sync::mpsc::Receiver<FFMpegPacketPayload>,

    decoded_frame: frame::Audio,
    decoded_frames_queue: VecDeque<f32>,
}

impl AudioDecoder {
    pub fn new(packet_consumer: std::sync::mpsc::Receiver<FFMpegPacketPayload>) -> Self {
        let codec = codec::decoder::find(ffmpeg::codec::Id::OPUS).expect("Opus codec not found");
        let context = ffmpeg::codec::context::Context::new_with_codec(codec);

        let codec = codec.audio().unwrap();

        let mut decoder = context.decoder().audio().unwrap();
        decoder.set_channel_layout(ChannelLayout::STEREO);

        Self {
            codec,
            decoder,

            packet_consumer,

            decoded_frame: frame::Audio::empty(),
            decoded_frames_queue: VecDeque::new(),
        }
    }

    fn decode(&mut self) {
        while let Ok(packet) = self.packet_consumer.try_recv() {
            self.decode_inner(packet.to_packet());
        }
    }

    fn decode_inner(&mut self, packet: Packet) {
        self.decoder.send_packet(&packet).unwrap();

        while self.decoder.receive_frame(&mut self.decoded_frame).is_ok() {
            let channels = self.decoded_frame.channels();
            let format = self.decoded_frame.format();

            let is_planar = format == format::Sample::F32(format::sample::Type::Planar);

            match (is_planar, channels) {
                (true, 2) => {
                    // Convert into F32::Packed
                    let left = self.decoded_frame.plane::<f32>(0);
                    let right = self.decoded_frame.plane::<f32>(1);

                    for (l, r) in left.iter().zip(right.iter()) {
                        self.decoded_frames_queue.push_back(*l);
                        self.decoded_frames_queue.push_back(*r);
                    }
                }
                (false, 2) => {
                    // Already Packed stereo
                    // TODO: Fix, it won't work because of the bug in `ffpeg-next`,
                    // it does not account for channels when stereo is packed
                    let data = self.decoded_frame.plane::<f32>(0);

                    self.decoded_frames_queue.extend(data)
                }
                (_, 1) => {
                    // Convert mono to stereo by duplicating
                    let data = self.decoded_frame.plane::<f32>(0);

                    for sample in data {
                        self.decoded_frames_queue.push_back(*sample);
                        self.decoded_frames_queue.push_back(*sample);
                    }
                }
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

    fn pop_slice_with(&mut self, out: &mut [T], partial: bool, f: impl Fn(T, T) -> T) -> usize;
}

impl<T: Clone + Copy> VecDequeExt<T> for VecDeque<T> {
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

    #[inline(always)]
    fn pop_slice_with(&mut self, out: &mut [T], partial: bool, f: impl Fn(T, T) -> T) -> usize {
        if !partial && self.len() < out.len() {
            return 0;
        }

        let length = self.len().min(out.len());
        (0..length).for_each(|idx| {
            let value = self.pop_front().unwrap();
            out[idx] = f(out[idx], value);
        });

        length
    }
}

trait StreamingCompatFrom {
    fn to_packet(&self) -> Packet;
}

trait StreamingCompatInto {
    fn to_payload(&self) -> FFMpegPacketPayload;
}

impl StreamingCompatFrom for FFMpegPacketPayload {
    fn to_packet(&self) -> Packet {
        let mut packet = Packet::new(self.data.len());

        packet.set_pts(Some(self.pts));

        packet.set_flags(codec::packet::Flags::from_bits_truncate(self.flags));
        let data = packet
            .data_mut()
            .expect("Should be present because Packet::new");

        data.copy_from_slice(&self.data);

        packet
    }
}

impl StreamingCompatInto for Packet {
    fn to_payload(&self) -> FFMpegPacketPayload {
        FFMpegPacketPayload {
            pts: self.pts().unwrap(),

            flags: self.flags().bits(),
            data: self.data().unwrap_or_default().to_vec(),
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

    fn on_process(stream: &Stream, this: &mut CaptureStreamData) {
        if !this.capture.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }

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

            let captured_samples = unsafe {
                std::slice::from_raw_parts(
                    valid_bytes.as_ptr() as *const f32,
                    valid_bytes.len() / size_of::<f32>(),
                )
            };

            // Encode everything we've captured
            if this.enable_noise_reduction {
                this.rnnoise_queue.extend(captured_samples);

                while this
                    .rnnoise_queue
                    .pop_slice(&mut this.rnnoise_in_buff, false)
                    > 0
                {
                    // As described in the `process_frame` documentation
                    for sample in this.rnnoise_in_buff.iter_mut() {
                        *sample = (32767.5 * (*sample) - 0.5).round();
                    }

                    this.denoise_state
                        .process_frame(&mut this.rnnoise_out_buff, &this.rnnoise_in_buff);

                    for sample in this.rnnoise_out_buff.iter_mut() {
                        *sample = ((*sample) + 0.5) / 32767.5;
                    }

                    this.encoder.encode(&this.rnnoise_out_buff);
                }
            } else {
                this.encoder.encode(captured_samples);
            }

            if this.enable_loopback {
                this.loopback_producer.push_slice(captured_samples);
            }

            while let Some(packet) = this.encoder.packet_buff.pop_front() {
                _ = this.packet_producer.send(packet);
            }
        }
    }

    fn new(
        core: &'a pw::core::CoreRc,
        packet_producer: std::sync::mpsc::Sender<FFMpegPacketPayload>,
        loopback_producer: HeapProd<f32>,
        capture: Arc<AtomicBool>,
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

        // Microphone capture is most likely already in MONO,
        // but we enforce it just to be sure
        audio_info.set_channels(1);
        let mut position = [0; spa::param::audio::MAX_CHANNELS];
        position[0] = libspa::sys::SPA_AUDIO_CHANNEL_MONO;
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
            capture,
            format: Default::default(),
            encoder: AudioEncoder::new(),

            enable_loopback: false,
            loopback_producer,

            packet_producer,

            enable_noise_reduction: true,
            denoise_state: DenoiseState::new(),
            rnnoise_queue: VecDeque::new(),
            rnnoise_in_buff: vec![0.0; DenoiseState::FRAME_SIZE],
            rnnoise_out_buff: vec![0.0; DenoiseState::FRAME_SIZE],
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

/// Struct that represents a connected user
/// to a voice chat
pub struct PlaybackClientState {
    pub user_id: i32,
    pub decoder: AudioDecoder,

    pub is_talking: Arc<AtomicBool>,
}

pub enum PlaybackClientMessage {
    AddClient(PlaybackClientState),
    RemoveClient(i32),
}

struct PlaybackStreamData {
    /// Ring Buffer that is used to loopback captured audio.
    /// Mainly used to quickly test how your microphone sounds
    loopback_consumer: HeapCons<f32>,

    clients: Vec<PlaybackClientState>,
    client_reciever: std::sync::mpsc::Receiver<PlaybackClientMessage>,
}

struct PlaybackStream<'a> {
    stream: StreamBox<'a>,
    stream_listener: StreamListener<PlaybackStreamData>,
}

impl<'a> PlaybackStream<'a> {
    const STREAM_NAME: &'static str = "HAZEL Audio Playback";

    fn on_process(stream: &Stream, user_data: &mut PlaybackStreamData) {
        // Check if have new clients we must listen to, or remove one
        if let Ok(msg) = user_data.client_reciever.try_recv() {
            match msg {
                PlaybackClientMessage::AddClient(client) => {
                    user_data.clients.push(client);
                }
                PlaybackClientMessage::RemoveClient(id) => {
                    user_data.clients.retain(|client| client.user_id != id);
                }
            }
        }

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
            for client in user_data.clients.iter_mut() {
                client.decoder.decode()
            }

            let output_samples = unsafe {
                std::slice::from_raw_parts_mut(slice.as_mut_ptr() as *mut f32, slice.len() / 4)
            };

            // Cleanup to ensure we don't mix garbage
            output_samples.iter_mut().for_each(|i| *i = 0.);

            // Mix multiple clients into a single stream
            let mut max_read_count = 0;
            for client in user_data.clients.iter_mut() {
                let read_count = client.decoder.decoded_frames_queue.pop_slice_with(
                    output_samples,
                    true,
                    |old, new| (old + new).min(1.),
                );

                max_read_count = max_read_count.max(read_count);
            }

            let chunk = data.chunk_mut();

            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = stride as i32;
            *chunk.size_mut() = (output_samples.len() * 4) as u32;
        }
    }

    fn new(
        core: &'a pw::core::CoreRc,
        loopback_consumer: HeapCons<f32>,
        client_reciever: std::sync::mpsc::Receiver<PlaybackClientMessage>,
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
            clients: vec![],
            client_reciever,
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

#[derive(Clone)]
pub struct Audio {
    capture: Arc<AtomicBool>,
    clients_sender: std::sync::mpsc::Sender<PlaybackClientMessage>,
}

pub struct RegisteredClient {
    pub user_id: i32,
    pub packet_sender: std::sync::mpsc::Sender<FFMpegPacketPayload>,
    pub is_talking: Arc<AtomicBool>,
}

impl Audio {
    pub fn new() -> AResult<(Self, std::sync::mpsc::Receiver<FFMpegPacketPayload>)> {
        // To test not encoded capture (when tweaking settings for example)
        let ring = HeapRb::new((DEFAULT_RATE * 2) as usize);
        let (loopback_producer, loopback_consumer) = ring.split();

        let (packet_sender, packet_reciever) = std::sync::mpsc::channel();
        let (clients_sender, clients_reciever) = std::sync::mpsc::channel();

        let capture = Arc::new(AtomicBool::new(false));

        let _capture = Arc::clone(&capture);
        thread::spawn(move || {
            pw::init();
            ffmpeg::init().unwrap();

            let mainloop = pw::main_loop::MainLoopRc::new(None)?;
            let context = pw::context::ContextRc::new(&mainloop, None)?;
            let core = context.connect_rc(None)?;

            let _capture = CaptureStream::new(&core, packet_sender, loopback_producer, _capture)?;
            let _playback = PlaybackStream::new(&core, loopback_consumer, clients_reciever)?;

            mainloop.run();

            Ok::<_, anyhow::Error>(())
        });

        Ok((
            Audio {
                capture,
                clients_sender,
            },
            packet_reciever,
        ))
    }

    pub fn remove_client(&self, client: RegisteredClient) {
        _ = self
            .clients_sender
            .send(PlaybackClientMessage::RemoveClient(client.user_id));
    }

    pub fn register_client(&self, user_id: i32) -> RegisteredClient {
        let (packet_sender, packet_consumer) = std::sync::mpsc::channel();

        let is_talking = Arc::new(AtomicBool::new(false));
        let decoder = AudioDecoder::new(packet_consumer);

        let state = PlaybackClientState {
            user_id,
            decoder,
            is_talking: is_talking.clone(),
        };

        _ = self
            .clients_sender
            .send(PlaybackClientMessage::AddClient(state));

        RegisteredClient {
            user_id,
            packet_sender,
            is_talking,
        }
    }

    pub fn is_capturing(&self) -> bool {
        self.capture.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn set_capture(&self, value: bool) {
        self.capture
            .store(value, std::sync::atomic::Ordering::SeqCst);
    }
}
