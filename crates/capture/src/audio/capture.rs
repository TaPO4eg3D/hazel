use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use ringbuf::HeapCons;

use crate::audio::{
    AudioLoopCommand, PlatformLoopController, encode::AudioEncoder,
};

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

pub(crate) type Notifier = Arc<(Mutex<bool>, Condvar)>;
pub struct Capture {
    is_enabled: Arc<AtomicBool>,
    platform_loop_controller: PlatformLoopController,

    notifier: Notifier,

    pub encoder: AudioEncoder,
    pub samples_buffer: HeapCons<f32>,
}

pub enum WaitResult {
    Ready,
    Timeout,
}

impl Capture {
    pub(crate) fn new(
        notifier: Notifier,
        samples_buffer: HeapCons<f32>,
        controller: PlatformLoopController,
    ) -> Self {
        Self {
            is_enabled: Arc::new(AtomicBool::new(false)),
            samples_buffer,
            platform_loop_controller: controller,
            encoder: AudioEncoder::new(),
            notifier,
        }
    }

    pub fn wait(&self, timeout: Duration) -> WaitResult {
        let mut ready = self.notifier.0.lock().unwrap();

        loop {
            let result = self.notifier.1.wait_timeout(ready, timeout).unwrap();

            ready = result.0;
            if result.1.timed_out() {
                return WaitResult::Timeout;
            }

            if *ready {
                *ready = false;

                return WaitResult::Ready;
            }
        }
    }

    pub fn get_controller(&self) -> CaptureController {
        CaptureController {
            is_enabled: self.is_enabled.clone(),
            platform_loop_controller: self.platform_loop_controller.clone(),
        }
    }
}
