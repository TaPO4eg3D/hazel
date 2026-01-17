use capture::audio::linux::Audio;
use gpui::{App, AppContext, AsyncApp, Global};

use streaming_common::FFMpegPacketPayload;

use crate::gpui_tokio::Tokio;

pub enum AudioMessage {
    PlayFromFile,
    PlayStreamingPacket(FFMpegPacketPayload),

    SetCapture(bool),
    SetPlayback(bool),
}

struct GlobalAudio {
    tx: smol::channel::Sender<AudioMessage>,
}

impl Global for GlobalAudio {}

impl GlobalAudio {
    fn process_message(audio: &mut Audio, message: AudioMessage) {
        match message {
            AudioMessage::PlayStreamingPacket(packet) => {
                audio.play_stream_packet(packet);
            },
            _ => todo!()
        }
    }

    async fn new() -> Self {
        let (tx, rx) = smol::channel::bounded(1024);

        tokio::spawn(async move {
            let mut audio = Audio::new()
                .unwrap();

            while let Ok(message) = rx.recv().await {
                Self::process_message(&mut audio, message);
            }
        });

        Self { tx }
    }
}

struct AudioManager {}

impl AudioManager {
    fn send_command<C>(cx: &C, msg: AudioMessage)
    where
        C: AppContext,
    {
    }
}

pub async fn init(cx: &mut AsyncApp) -> anyhow::Result<()> {
    let manager = Tokio::spawn(cx, GlobalAudio::new()).await?;

    cx.update(|cx| cx.set_global(manager));

    Ok(())
}
