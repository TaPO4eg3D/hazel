use std::collections::VecDeque;

use streaming_common::EncodedAudioPacket;

use crate::audio::{DEFAULT_CHANNELS, DEFAULT_RATE};

const OUTPUT_BUFFER_SIZE: usize = ((DEFAULT_RATE as usize / 1000) * 20) * DEFAULT_CHANNELS as usize;

/// Instance of the Opus decoder. Please note that Opus is
/// a stateful codec, hence each client MUST have its own instance
/// of this decoder. Otherwise, encoding artifacts are guaranteed
pub(crate) struct AudioDecoder {
    /// Instance of the Opus decoder
    decoder: opus::Decoder,

    /// Buffer where decoder outputs the result. Reused for every
    /// decoder pass
    output_buffer: [f32; OUTPUT_BUFFER_SIZE],

    /// That's the "output" of [`Self::decode`] function
    pub(crate) decoded_samples: VecDeque<f32>,
}

impl AudioDecoder {
    #[allow(clippy::new_without_default)]
    pub(crate) fn new() -> Self {
        let decoder = opus::Decoder::new(
            DEFAULT_RATE,
            opus::Channels::Stereo,
        ).expect("Failed to initialize decoder");

        Self {
            decoder,

            output_buffer: [0.; OUTPUT_BUFFER_SIZE],
            decoded_samples: VecDeque::new(),
        }
    }

    fn decode_inner(&mut self, input: &[u8], fec: bool, out_limit: usize) {
        if let Ok(n) = self.decoder.decode_float(
            input,
            &mut self.output_buffer[..out_limit],
            fec,
        ) {

            self.output_buffer
                .iter()
                .take(n * 2) // n is per frame, we're stereo
                .for_each(|sample| self.decoded_samples.push_back(*sample));
        }
    }

    pub(crate) fn reset(&mut self) {
        _ = self.decoder.reset_state();
    }

    pub(crate) fn ask_plc(&mut self, out_limit: usize) {
        self.decode_inner(
            &[],
            false,
            out_limit,
        );
    }

    pub(crate) fn decode_fec(&mut self, packet: EncodedAudioPacket, out_limit: usize) {
        self.decode_inner(
            packet.as_slice(),
            true,
            out_limit,
        );
    }

    pub(crate) fn decode(&mut self, packet: EncodedAudioPacket) {
        self.decode_inner(
            packet.as_slice(),
            false,
            OUTPUT_BUFFER_SIZE,
        );
    }
}
