use std::thread;

use pipewire::{self as pw};
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::*};

use ffmpeg_next::{self as ffmpeg};

use crate::audio::{DEFAULT_RATE, linux::{capture::CaptureStream, playback::PlaybackStream}};

pub mod capture;
pub mod playback;

enum PipewireCommand {
    SetCapture(bool),
    SetPlayback(bool),
}

pub struct LinuxCapture {
    pw_sender: pw::channel::Sender<PipewireCommand>,
    capture_consumer: HeapCons<f32>,
}

impl LinuxCapture {
    fn set_active(&self, active: bool) {
        _ = self.pw_sender.send(PipewireCommand::SetCapture(active));
    }
}

pub struct LinuxPlayback {
    pw_sender: pw::channel::Sender<PipewireCommand>,
    playback_producer: HeapProd<f32>,
}

impl LinuxPlayback {
    pub fn push(&mut self, data: &[f32]) {
        self.playback_producer.push_slice(data);
    }

    pub fn set_active(&self, active: bool) {
        _ = self.pw_sender.send(PipewireCommand::SetCapture(active));
    }
}

fn init() -> (LinuxCapture, LinuxPlayback) {
    let ring = HeapRb::new((DEFAULT_RATE * 2) as usize);
    let (capture_producer, capture_consumer) = ring.split();

    let ring = HeapRb::new((DEFAULT_RATE * 2) as usize);
    let (playback_producer, playback_consumer) = ring.split();

    let (pw_sender, pw_receiver) = pw::channel::channel::<PipewireCommand>();

    let capture = LinuxCapture {
        pw_sender: pw_sender.clone(),
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

        let capture = CaptureStream::new(core.clone(), capture_producer)?;
        let capture_stream = capture.stream.clone();

        let playback = PlaybackStream::new(core, playback_consumer)?;
        let playback_stream = playback.stream.clone();

        // TODO: Maybe it's better to emit a loop event
        // and deactivate inside the event handler (to clean up leftovers)
        let _attached = pw_receiver.attach(mainloop.loop_(), move |msg| match msg {
            PipewireCommand::SetCapture(active) => {
                _ = capture_stream.set_active(active);
            },
            PipewireCommand::SetPlayback(active) => {
                _ = playback_stream.set_active(active);
            }
        });

        mainloop.run();

        Ok::<_, anyhow::Error>(())
    });

    (capture, playback)
}
