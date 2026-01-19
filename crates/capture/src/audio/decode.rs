use std::collections::VecDeque;

use ffmpeg_next::{ChannelLayout, Packet, codec, format, frame};

/// Instance of the Opus decoder. Please note that Opus is
/// a stateful codec, hence each client MUST have its own instance
/// of this decoder. Otherwise, encoding artifacts are guaranteed
pub struct AudioDecoder {
    /// Instance of the Opus FFmpeg decoder
    decoder: codec::decoder::Audio,

    /// Buffer of decoded samples. Reused for every decoder pass
    decoded_frame: frame::Audio,

    /// That's the "output" of [`Self::decode`] function
    decoded_samples: VecDeque<f32>,
}

impl AudioDecoder {
    pub fn new() -> Self {
        let codec = codec::decoder::find(codec::Id::OPUS).expect("Opus codec is not found");
        let context = codec::context::Context::new_with_codec(codec);

        let mut decoder = context.decoder().audio().unwrap();
        decoder.set_channel_layout(ChannelLayout::STEREO);

        Self {
            decoder,

            decoded_frame: frame::Audio::empty(),
            decoded_samples: VecDeque::new(),
        }
    }

    fn decode(&mut self, packet: Packet) {
        self.decoder.send_packet(&packet).unwrap();

        while self.decoder.receive_frame(&mut self.decoded_frame).is_ok() {
            let channels = self.decoded_frame.channels();
            let format = self.decoded_frame.format();

            let is_planar = match format {
                format::Sample::F32(layout) => matches!(layout, format::sample::Type::Planar),
                format => {
                    panic!("Unexpected decoded samples format: {format:?}");
                }
            };

            match (is_planar, channels) {
                (true, 2) => { // Planar => F32::Packed
                    let left = self.decoded_frame.plane::<f32>(0);
                    let right = self.decoded_frame.plane::<f32>(1);

                    for (l, r) in left.iter().zip(right.iter()) {
                        self.decoded_samples.push_back(*l);
                        self.decoded_samples.push_back(*r);
                    }
                }
                (false, 2) => { // Already packed STEREO
                    // We have to use unsafe because of the bug in `ffpeg-next`. 
                    // It does not account for channels when we have packed samples
                    let data = unsafe {
                        std::slice::from_raw_parts(
                            (*self.decoded_frame.as_ptr()).data[0] as *mut f32,
                            self.decoded_frame.samples() * self.decoded_frame.channels() as usize,
                        )
                    };

                    self.decoded_samples.extend(data);
                }
                (_, 1) => { // Mono (which should not happen by the way but just in case)
                    let data = self.decoded_frame.plane::<f32>(0);

                    for sample in data {
                        self.decoded_samples.push_back(*sample);
                        self.decoded_samples.push_back(*sample);
                    }
                }
                _ => unimplemented!("Unexpected decoder output: {:?}", (is_planar, channels)),
            }
        }
    }
}
