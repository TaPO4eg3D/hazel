use std::thread;

use pipewire::{self as pw};
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::*};

use ffmpeg_next::{self as ffmpeg};

use crate::audio::{DEFAULT_RATE, linux::{capture::CaptureStream, playback::PlaybackStream}};

pub mod capture;
pub mod playback;

enum PipewireCommand {
    SetCapture(bool),
}

pub struct LinuxCapture {
    pw_sender: pw::channel::Sender<PipewireCommand>,
    capture_consumer: HeapCons<f32>,
}

impl LinuxCapture {
    fn set_capture(&self, capture: bool) {
        _ = self.pw_sender.send(PipewireCommand::SetCapture(capture));
    }
}

pub struct LinuxPlayback {
    playback_producer: HeapProd<f32>,
}

fn init() -> (LinuxCapture, LinuxPlayback) {
    let ring = HeapRb::new((DEFAULT_RATE * 2) as usize);
    let (capture_producer, capture_consumer) = ring.split();

    let ring = HeapRb::new((DEFAULT_RATE * 2) as usize);
    let (playback_producer, playback_consumer) = ring.split();

    let (pw_sender, pw_receiver) = pw::channel::channel::<PipewireCommand>();

    let capture = LinuxCapture {
        pw_sender,
        capture_consumer,
    };

    let playback = LinuxPlayback {
        playback_producer,
    };

    thread::spawn(move || {
        pw::init();
        ffmpeg::init().unwrap();

        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;

        let capture = CaptureStream::new(core.clone(), capture_producer)?;
        let stream = capture.stream.clone();

        let _playback = PlaybackStream::new(core, playback_consumer)?;

        pw_receiver.attach(mainloop.loop_(), move |msg| match msg {
            PipewireCommand::SetCapture(capture) => {
                stream.set_active(capture);
            }
        });

        mainloop.run();

        Ok::<_, anyhow::Error>(())
    });

    (capture, playback)
}
