use std::{
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

pub mod audio;
#[cfg(target_os = "linux")]
pub mod video;

#[derive(Default)]
struct CaptureNotifierInner {
    state: Mutex<CaptureNotifierState>,
    condvar: Condvar,
}

#[derive(Clone, Copy, Default)]
pub struct CaptureNotifierState {
    pub ping: bool,
    pub is_audio_ready: bool,
    pub is_screen_ready: bool,
}

#[derive(Clone)]
pub struct CaptureNotifier {
    inner: Arc<CaptureNotifierInner>,
}

pub enum WaitResult {
    Ready(CaptureNotifierState),
    Timeout,
}

impl CaptureNotifier {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(CaptureNotifierInner::default()),
        }
    }

    pub fn wait(&self, timeout: Duration) -> WaitResult {
        let mut state = self.inner.state.lock().unwrap();

        let mut timed_out = false;
        while !state.is_audio_ready && !state.is_screen_ready {
            if timed_out {
                return WaitResult::Timeout;
            }

            let (guard, result) = self.inner.condvar.wait_timeout(state, timeout).unwrap();

            state = guard;
            timed_out = result.timed_out();
        }

        let result = *state;

        state.is_audio_ready = false;
        state.is_screen_ready = false;
        state.ping = false;

        WaitResult::Ready(result)
    }

    pub(crate) fn notify_audio(&self) {
        let mut state = self.inner.state.lock().unwrap();
        state.is_audio_ready = true;

        self.inner.condvar.notify_one();
    }

    pub fn notify_ping(&self) {
        let mut state = self.inner.state.lock().unwrap();
        state.ping = true;

        self.inner.condvar.notify_one();
    }

    pub(crate) fn notify_screen(&self) {
        let mut state = self.inner.state.lock().unwrap();
        state.is_screen_ready = true;

        self.inner.condvar.notify_one();
    }
}
