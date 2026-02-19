use std::collections::VecDeque;

use streaming_common::{DATA_BUFF_SIZE, EncodedAudioPacket};

use crate::audio::{DEFAULT_BIT_RATE, DEFAULT_RATE, VecDequeExt as _};

const INPUT_BUFFER_SIZE: usize = (DEFAULT_RATE as usize / 1000) * 20;

/// Instance of the Opus encoder. Please note that Opus is
/// a stateful codec, hence each client MUST have its own instance
/// of this encoder. Otherwise, encoding artifacts are guaranteed
pub struct AudioEncoder {
    /// Instance of the Opus FFmpeg encoder
    encoder: opus::Encoder,

    /// Buffer where encoder outputs the result. Reused for every
    /// encoder pass
    output_buffer: [u8; DATA_BUFF_SIZE],
    input_buffer: [f32; INPUT_BUFFER_SIZE],

    /// Opus requires a specific number of samples to
    /// be supplied into it. This buffer is used to accumulate
    /// enough amount of them
    samples_queue: VecDeque<f32>,
    packet_queue: VecDeque<EncodedAudioPacket>,
}

impl AudioEncoder {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let mut encoder =
            opus::Encoder::new(DEFAULT_RATE, opus::Channels::Mono, opus::Application::Voip)
                .expect("Failed to init encoder");

        encoder
            .set_bitrate(opus::Bitrate::Bits(DEFAULT_BIT_RATE as i32))
            .unwrap();
        encoder.set_inband_fec(true).unwrap();

        // TODO: Ideally Packet Loss% should adjust dynamically.
        // We need to implement some sort of a report of how much packets
        // are lost between all the client in the connected channel.
        // ...and what's next? We can spin up several encoders
        // to serve different range of packet losses
        encoder.set_packet_loss_perc(10)
            .unwrap();

        Self {
            encoder,
            samples_queue: VecDeque::new(),
            packet_queue: VecDeque::new(),

            input_buffer: [0.; INPUT_BUFFER_SIZE],
            output_buffer: [0; DATA_BUFF_SIZE],
        }
    }

    pub fn pop_packet(&mut self) -> Option<EncodedAudioPacket> {
        self.packet_queue.pop_front()
    }

    pub fn encode(&mut self, samples: &[f32]) {
        self.samples_queue.extend(samples);

        while self
            .samples_queue
            .pop_slice(&mut self.input_buffer[..], false)
            > 0
        {
            if let Ok(n) = self
                .encoder
                .encode_float(&self.input_buffer[..], &mut self.output_buffer[..])
            {
                self.packet_queue
                    .push_back(EncodedAudioPacket::new(&self.output_buffer[..n]));
            }
        }
    }
}
