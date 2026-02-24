use std::collections::VecDeque;

use rnnoise_sys::{DenoiseState, rnnoise_create, rnnoise_destroy, rnnoise_process_frame};

use crate::audio::VecDequeExt;

const RNN_FRAME_SIZE: usize = 480;

pub struct RNNoiseState {
    denoise_state: *mut DenoiseState,
    
    buffer: [f32; RNN_FRAME_SIZE],

    input_queue: VecDeque<f32>,
    pub output_queue: VecDeque<f32>,
}

impl RNNoiseState {
    pub fn new() -> Self {
        unsafe {
            let denoise_state = rnnoise_create(std::ptr::null_mut());

            Self {
                denoise_state,

                buffer: [0.; RNN_FRAME_SIZE],

                input_queue: VecDeque::new(),
                output_queue: VecDeque::new(),
            }
        }
    }

    pub fn process(&mut self, samples: &[f32]) {
        self.input_queue.extend(samples);

        while self.input_queue.pop_slice(&mut self.buffer, false) > 0 {
            unsafe {
                let _ = rnnoise_process_frame(
                    self.denoise_state,
                    self.buffer.as_mut_ptr(),
                    self.buffer.as_ptr(),
                );

                self.output_queue.extend(self.buffer);
            }
        }
    }
}

impl Default for RNNoiseState {
    fn default() -> Self {
        Self::new()
    }
}


impl Drop for RNNoiseState {
    fn drop(&mut self) {
        unsafe {
            rnnoise_destroy(self.denoise_state);
        }
    }
}

enum NoiseReductionLayer {
    RNNoise(RNNoiseState),
}
