use std::{
    ops::Sub, sync::{
        Arc, Condvar, Mutex, RwLock,
        atomic::{AtomicPtr, Ordering},
    }, task::{Poll, Waker}, thread::{self, Thread}
};

use pipewire::{self as pw, channel, registry, types::ObjectType};
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::*};

use ffmpeg_next::{self as ffmpeg};

use crate::audio::{
    AudioDevice, AudioLoopCommand, DEFAULT_CHANNELS, DEFAULT_RATE, DeviceRegistry, linux::{capture::CaptureStream, playback::PlaybackStream}
};

pub mod capture;
pub mod playback;

/// Wakes up a sleeping thread when data
/// is available for consumption
#[derive(Clone, Default)]
pub(crate) struct Notifier {
    thread: Arc<Mutex<Option<Thread>>>,
}

impl Notifier {
    pub fn new() -> Self {
        Self {
            thread: Arc::new(Mutex::new(None)),
        }
    }

    pub fn notify(&self) {
        let handle = {
            let guard = self.thread.lock().unwrap();

            guard.clone()
        };

        if let Some(thread) = handle {
            thread.unpark();
        }
    }

    pub fn listen_updates(&self) {
        let mut guard = self.thread.lock().unwrap();
        *guard = Some(std::thread::current());
    }
}

pub(crate) struct LinuxCapture {
    capture_notifier: Notifier,

    pw_sender: pw::channel::Sender<AudioLoopCommand>,
    capture_consumer: HeapCons<f32>,
}

impl LinuxCapture {
    pub fn pop(&mut self, buf: &mut [f32]) -> usize {
        if self.capture_consumer.occupied_len() == 0 {
            std::thread::park();
        }

        self.capture_consumer.pop_slice(buf)
    }

    pub fn get_controller(&self) -> pw::channel::Sender<AudioLoopCommand> {
        self.pw_sender.clone()
    }

    pub fn listen_updates(&mut self) {
        self.capture_notifier.listen_updates();
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

pub(crate) fn init() -> (LinuxCapture, LinuxPlayback, DeviceRegistry) {
    // We capture in mono and there's no point to store
    // more than 60ms
    let ring = HeapRb::new(((DEFAULT_RATE / 1000) * 60) as usize);
    let (capture_producer, capture_consumer) = ring.split();

    let ring = HeapRb::new((DEFAULT_RATE * DEFAULT_CHANNELS) as usize);
    let (playback_producer, playback_consumer) = ring.split();

    let (pw_sender, pw_receiver) = pw::channel::channel::<AudioLoopCommand>();

    let capture_notifier = Notifier::new();

    let capture = LinuxCapture {
        capture_consumer,

        pw_sender: pw_sender.clone(),
        capture_notifier: capture_notifier.clone(),
    };

    let playback = LinuxPlayback {
        pw_sender,
        playback_producer,
    };

    let _device_registry = DeviceRegistry::default();
    let device_registry = _device_registry.clone();

    thread::Builder::new()
        .name("pipewire-loop".into())
        .spawn(move || {
            pw::init();
            ffmpeg::init().unwrap();

            let mainloop = pw::main_loop::MainLoopRc::new(None)?;
            let context = pw::context::ContextRc::new(&mainloop, None)?;
            let core = context.connect_rc(None)?;

            let registry = core.get_registry()?;
            let capture = CaptureStream::new(core.clone(), capture_notifier, capture_producer)?;
            let capture_stream = capture.stream.clone();

            let playback = PlaybackStream::new(core.clone(), playback_consumer)?;
            let playback_stream = playback.stream.clone();

            let _listener = registry
                .add_listener_local()
                .global({
                    let capture_stream = capture_stream.clone();
                    let playback_stream = playback_stream.clone();

                    let device_registry = device_registry.clone();

                    move |obj| {
                        let Some(props) = obj.props else {
                            return;
                        };

                        match obj.type_ {
                            ObjectType::Node => {
                                let Some(class) = props.get("media.class") else {
                                    return;
                                };

                                let node_name = props
                                    .get("node.nick")
                                    .or_else(|| props.get("node.name"))
                                    .unwrap_or("Unknown Device");

                                match class {
                                    "Audio/Sink" => {
                                        if device_registry.device_exists(obj.id) {
                                            return;
                                        }

                                        device_registry.add_output(AudioDevice {
                                            id: obj.id,
                                            name: node_name.to_string(),
                                            // On this stage we don't know if a device is
                                            // linked to our app
                                            is_active: false,
                                        });
                                    }
                                    "Audio/Source" => {
                                        if device_registry.device_exists(obj.id) {
                                            return;
                                        }

                                        device_registry.add_input(AudioDevice {
                                            id: obj.id,
                                            name: node_name.to_string(),
                                            // On this stage we don't know if a device is
                                            // linked to our app
                                            is_active: false,
                                        });
                                    }
                                    _ => {}
                                }
                            }
                            ObjectType::Link => {
                                let Some(input_node) = props.get("link.input.node") else {
                                    return;
                                };

                                let Some(output_node) = props.get("link.output.node") else {
                                    return;
                                };

                                let input_node: u32 = input_node.parse().unwrap();
                                let output_node: u32 = output_node.parse().unwrap();

                                if input_node == capture_stream.node_id() {
                                    device_registry.set_active_input(output_node);
                                }

                                if output_node == playback_stream.node_id() {
                                    device_registry.set_active_output(input_node);
                                }
                            }
                            _ => {}
                        }
                    }
                })
                .global_remove(move |id| {
                    device_registry.remove_device(id);
                })
                .register();

            // TODO: Maybe it's better to emit a loop event
            // and deactivate inside the event handler (to clean up leftovers)
            let _attached = pw_receiver.attach(mainloop.loop_(), move |msg| match msg {
                AudioLoopCommand::SetEnabledCapture(active) => {
                    _ = capture_stream.set_active(active);
                }
                AudioLoopCommand::SetEnabledPlayback(active) => {
                    _ = playback_stream.set_active(active);
                }
            });

            mainloop.run();

            Ok::<_, anyhow::Error>(())
        })
        .unwrap();

    (capture, playback, _device_registry)
}
