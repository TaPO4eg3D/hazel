use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use ringbuf::HeapCons;

use crate::audio::{AudioLoopCommand, PlatformLoopController, encode::AudioEncoder};

#[derive(Clone)]
pub struct CaptureController {
    is_enabled: Arc<AtomicBool>,
    platform_loop_controller: PlatformLoopController,
}

impl CaptureController {
    pub fn set_enabled(&self, value: bool) {
        self.is_enabled.store(value, Ordering::Relaxed);

        _ = self
            .platform_loop_controller
            .send(AudioLoopCommand::SetEnabledCapture(value));
    }
}

pub struct AudioCapture {
    pub encoder: AudioEncoder,
    pub is_enabled: Arc<AtomicBool>,
    pub samples_buffer: HeapCons<f32>,

    platform_loop_controller: PlatformLoopController,
}

impl AudioCapture {
    pub(crate) fn new(samples_buffer: HeapCons<f32>, controller: PlatformLoopController) -> Self {
        Self {
            is_enabled: Arc::new(AtomicBool::new(false)),
            samples_buffer,
            platform_loop_controller: controller,
            encoder: AudioEncoder::new(),
        }
    }

    pub fn get_controller(&self) -> CaptureController {
        CaptureController {
            is_enabled: self.is_enabled.clone(),
            platform_loop_controller: self.platform_loop_controller.clone(),
        }
    }
}
