use std::collections::VecDeque;

use ffmpeg_next::{ChannelLayout, Packet, codec, encoder, format, frame};
use streaming_common::FFMpegPacketPayload;

use crate::audio::{DEFAULT_BIT_RATE, DEFAULT_RATE, StreamingCompatInto as _, VecDequeExt as _};

/// Instance of the Opus encoder. Please note that Opus is 
/// a stateful codec, hence each client MUST have its own instance
/// of this encoder. Otherwise, encoding artifacts are guaranteed
struct AudioEncoder {
    /// Instance of the Opus FFmpeg encoder
    encoder: encoder::audio::Encoder,

    /// Buffer of raw samples. Reused for every encoder pass
    raw_frame: frame::audio::Audio,

    /// Buffer for encoded data. Reused for every encoder pass
    encoded_packet: Packet,

    /// Opus requires a specific number of samples to
    /// be supplied into it. This buffer is used to accumulate
    /// enough amount of them
    frame_queue: VecDeque<f32>,

    /// Number of frames successfully encoded.
    /// Used to properly position frames on decoding step
    pts_counter: i64,

    /// One audio frame could result in multiple encoded packets,
    /// we store all of them in this buffer. This is the "output" of
    /// [`Self::encode`] function
    encoded_packets: VecDeque<FFMpegPacketPayload>,

}

impl AudioEncoder {
    fn new() -> Self {
        let codec = encoder::find(codec::Id::OPUS).expect("Opus codec not found");
        let context = codec::context::Context::new_with_codec(codec);

        let codec = codec.audio().unwrap();

        let mut encoder = context.encoder().audio().unwrap();

        encoder.set_rate(DEFAULT_RATE as i32);
        encoder.set_channel_layout(ChannelLayout::MONO);
        encoder.set_format(format::Sample::F32(format::sample::Type::Packed));

        encoder.set_bit_rate(DEFAULT_BIT_RATE);
        encoder.set_time_base((1, DEFAULT_RATE as i32));

        let encoder = encoder.open_as(codec).unwrap();

        // Just a note for myself, in case I forget that shit again:
        // `frame_size` means number of samples **PER** channel
        let frame_size = encoder.frame_size() as usize;

        Self {
            encoder,
            raw_frame: frame::audio::Audio::new(
                format::Sample::F32(format::sample::Type::Packed),
                frame_size,
                ChannelLayout::MONO,
            ),
            pts_counter: 0,

            encoded_packet: Packet::empty(),
            encoded_packets: VecDeque::new(),

            frame_queue: VecDeque::new(),
        }
    }

    fn pop_packet(&mut self) -> Option<FFMpegPacketPayload> {
        self.encoded_packets.pop_front()
    }

    /// Encoded provided `samples`. This could result in multiple encoded packets.
    /// Packets can be extracted by using [`Self::pop_packet`] function.
    fn encode(&mut self, samples: &[f32]) {
        self.frame_queue.extend(samples);

        loop {
            // We have to use unsafe because of the bug in `ffpeg-next`. 
            // It does not account for channels when we have packed samples
            let plane = unsafe {
                std::slice::from_raw_parts_mut(
                    (*self.raw_frame.as_mut_ptr()).data[0] as *mut f32,
                    self.raw_frame.samples() * self.raw_frame.channels() as usize,
                )
            };

            if self.frame_queue.pop_slice(plane, false) == 0 {
                break;
            }

            self.raw_frame.set_pts(Some(self.pts_counter));
            self.encoder.send_frame(&self.raw_frame).unwrap();

            let (new_pts, _) = self
                .pts_counter
                .overflowing_add(self.encoder.frame_size() as i64);

            self.pts_counter = new_pts;

            while self
                .encoder
                .receive_packet(&mut self.encoded_packet)
                .is_ok()
            {
                let encoded_data = self.encoded_packet.data().unwrap_or_default();

                if encoded_data.is_empty() {
                    continue;
                }

                self.encoded_packets.push_back(self.encoded_packet.to_payload())
            }
        }
    }
}
