use std::{
    cell::RefCell, collections::HashMap, ops::Sub, rc::Rc, sync::{
        Arc, Condvar, Mutex, RwLock,
        atomic::{AtomicPtr, Ordering},
    }, task::{Poll, Waker}, thread::{self, Thread}
};

use pipewire::{self as pw, channel, properties::properties, registry, sys::pw_proxy_add_listener, types::ObjectType};
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::*};

use ffmpeg_next::{self as ffmpeg};

use crate::audio::{
    AudioDevice, AudioLoopCommand, DEFAULT_CHANNELS, DEFAULT_RATE, DeviceRegistry,
    linux::{capture::CaptureStream, playback::PlaybackStream},
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

// fn device_set_route_properties(
//     device: &AudioDevice,
//     route_index: i32,
//     route_device: i32,
// ) {
//     let mut route_properties = Vec::new();
//     route_properties.push(Property {
//         key: libspa_sys::SPA_PARAM_ROUTE_index,
//         flags: PropertyFlags::empty(),
//         value: Value::Int(route_index),
//     });
//     route_properties.push(Property {
//         key: libspa_sys::SPA_PARAM_ROUTE_device,
//         flags: PropertyFlags::empty(),
//         value: Value::Int(route_device),
//     });
//
//     route_properties.push(Property {
//         key: libspa_sys::SPA_PARAM_ROUTE_save,
//         flags: PropertyFlags::empty(),
//         value: Value::Bool(true),
//     });
//     let route_properties = route_properties;
//
//     let values = PodSerializer::serialize(
//         std::io::Cursor::new(Vec::new()),
//         &Value::Object(Object {
//             type_: libspa_sys::SPA_TYPE_OBJECT_ParamRoute,
//             id: libspa_sys::SPA_PARAM_Route,
//             properties: route_properties,
//         }),
//     );
//
//     if let Ok((values, _)) = values {
//         if let Some(pod) = Pod::from_bytes(&values.into_inner()) {
//             device.set_param(ParamType::Route, 0, pod);
//         }
//     }
// }

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
        pw_sender: pw_sender.clone(),
        playback_producer,
    };

    let _device_registry = DeviceRegistry::new(pw_sender);
    let device_registry = _device_registry.clone();

    thread::Builder::new()
        .name("pipewire-loop".into())
        .spawn(move || {
            pw::init();
            ffmpeg::init().unwrap();

            let mainloop = pw::main_loop::MainLoopRc::new(None)?;
            let context = pw::context::ContextRc::new(&mainloop, None)?;
            let core = context.connect_rc(None)?;

            let registry = core.get_registry_rc()?;
            let capture = CaptureStream::new(core.clone(), capture_notifier, capture_producer)?;
            let capture_stream = capture.stream.clone();

            let playback = PlaybackStream::new(core.clone(), playback_consumer)?;
            let playback_stream = playback.stream.clone();

            let routing_meta: Rc<RefCell<Option<pw::metadata::Metadata>>> = Default::default();

            let _listener = registry
                .clone()
                .add_listener_local()
                .global({
                    let capture_stream = capture_stream.clone();
                    let playback_stream = playback_stream.clone();

                    let routing_meta = routing_meta.clone();
                    let device_registry = device_registry.clone();

                    move |obj| {
                        let Some(props) = obj.props else {
                            return;
                        };

                        match obj.type_ {
                            ObjectType::Node => {
                                let Some(class) = props.get(*pw::keys::MEDIA_CLASS) else {
                                    return;
                                };

                                let display_name = props
                                    .get(*pw::keys::NODE_NICK)
                                    .or_else(|| props.get(*pw::keys::NODE_NAME))
                                    .unwrap_or("Unknown Device");

                                let Some(name) = props
                                    .get(*pw::keys::NODE_NAME)
                                    .or_else(|| props.get("object.serial"))
                                else {
                                    println!("Invalid Pipewire object: {}!", obj.id);
                                    return;
                                };

                                match class {
                                    "Audio/Sink" => {
                                        if device_registry.device_exists(obj.id) {
                                            return;
                                        }

                                        device_registry.add_output(AudioDevice {
                                            id: obj.id,
                                            name: name.into(),
                                            display_name: display_name.into(),
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
                                            name: name.into(),
                                            display_name: display_name.to_string(),
                                            // On this stage we don't know if a device is
                                            // linked to our app
                                            is_active: false,
                                        });
                                    }
                                    _ => {}
                                }
                            },
                            ObjectType::Metadata => {
                                let Some(name) = props.get("metadata.name") else {
                                    return;
                                };

                                if name == "default" {
                                    let mut routing_meta = routing_meta.borrow_mut();
                                    let node = match registry.bind(obj) {
                                        Ok(node) => node,
                                        Err(err) => {
                                            println!("Failed to bind routing metadata: {err:?}");
                                            return;
                                        }
                                    };

                                    *routing_meta = Some(node);
                                }

                            },
                            ObjectType::Link => {
                                let Some(input_node) = props.get(*pw::keys::LINK_INPUT_NODE) else {
                                    return;
                                };

                                let Some(output_node) = props.get(*pw::keys::LINK_OUTPUT_NODE)
                                else {
                                    return;
                                };

                                let input_node: u32 = input_node.parse().unwrap();
                                let output_node: u32 = output_node.parse().unwrap();

                                if input_node == capture_stream.node_id() {
                                    device_registry.mark_active_input(output_node);
                                }

                                if output_node == playback_stream.node_id() {
                                    device_registry.mark_active_output(input_node);
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
                AudioLoopCommand::SetActiveInputDevice(device) => {
                    let metadata = routing_meta.borrow();

                    if let Some(node) = &*metadata {
                        node.set_property(
                            capture_stream.node_id(),
                            "target.object",
                            None,
                            Some(&device.name)
                        );
                    }
                }
                AudioLoopCommand::SetActiveOutputDevice(device) => {
                    let metadata = routing_meta.borrow();

                    if let Some(node) = &*metadata {
                        node.set_property(
                            playback_stream.node_id(),
                            "target.object",
                            None,
                            Some(&device.name)
                        );
                    }
                }
            });

            mainloop.run();

            Ok::<_, anyhow::Error>(())
        })
        .unwrap();

    (capture, playback, _device_registry)
}
