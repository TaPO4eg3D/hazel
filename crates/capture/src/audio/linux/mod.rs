use std::{sync::{Arc, Condvar, Mutex}, thread};

use pipewire::{self as pw, channel};
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::*};

use ffmpeg_next::{self as ffmpeg};

use crate::audio::{AudioLoopCommand, DEFAULT_RATE, linux::{capture::CaptureStream, playback::PlaybackStream}};

pub mod capture;
pub mod playback;

pub(crate) struct LinuxCapture {
    notify_rx: crossbeam::channel::Receiver<()>,

    pw_sender: pw::channel::Sender<AudioLoopCommand>,
    capture_consumer: HeapCons<f32>,
}

impl LinuxCapture {
    pub(crate) fn pop(&mut self, buf: &mut [f32]) -> usize {
        if self.capture_consumer.occupied_len() == 0 {
            _ = self.notify_rx.recv();
        }

        self.capture_consumer.pop_slice(buf)
    }

    pub fn get_controller(&self) -> pw::channel::Sender<AudioLoopCommand> {
        self.pw_sender.clone()
    }
}

pub struct LinuxPlayback {
    pw_sender: pw::channel::Sender<AudioLoopCommand>,
    playback_producer: HeapProd<f32>,
}

impl LinuxPlayback {
    pub fn push(&mut self, data: &[f32]) {
        self.playback_producer.push_slice(data);
    }
}

pub(crate) fn init() -> (LinuxCapture, LinuxPlayback) {
    let ring = HeapRb::new((DEFAULT_RATE * 2) as usize);
    let (capture_producer, capture_consumer) = ring.split();

    let ring = HeapRb::new((DEFAULT_RATE * 2) as usize);
    let (playback_producer, playback_consumer) = ring.split();

    let (pw_sender, pw_receiver) = pw::channel::channel::<AudioLoopCommand>();

    let (notify_tx, notify_rx) = crossbeam::channel::bounded(1);

    let capture = LinuxCapture {
        pw_sender: pw_sender.clone(),
        notify_rx,
        capture_consumer,
    };

    let playback = LinuxPlayback {
        pw_sender,
        playback_producer,
    };

    thread::spawn(move || {
        pw::init();
        ffmpeg::init().unwrap();

        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;

        let capture = CaptureStream::new(core.clone(), notify_tx, capture_producer)?;
        let capture_stream = capture.stream.clone();

        let playback = PlaybackStream::new(core, playback_consumer)?;
        let playback_stream = playback.stream.clone();

        // TODO: Maybe it's better to emit a loop event
        // and deactivate inside the event handler (to clean up leftovers)
        let _attached = pw_receiver.attach(mainloop.loop_(), move |msg| match msg {
            AudioLoopCommand::SetEnabledCapture(active) => {
                _ = capture_stream.set_active(active);
            },
            AudioLoopCommand::SetEnabledPlayback(active) => {
                _ = playback_stream.set_active(active);
            }
        });

        mainloop.run();

        Ok::<_, anyhow::Error>(())
    });

    (capture, playback)
}
