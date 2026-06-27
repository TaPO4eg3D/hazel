use std::{thread, time::Duration};

use crossbeam::channel::{self, RecvTimeoutError};
use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Producer, Split as _},
};
use streaming_common::{EncodedVideoBytes, OwnedEncodedVideoFrameChunk, StreamPacketHeader};

use crate::video::decode::VAAPIDecoder;

#[derive(Default)]
struct PendingFrame {
    header: StreamPacketHeader,
    processed_chunks: u32,
    pending: bool,
    frame: Vec<u8>,
}

struct VideoStreamingClientState {
    user_id: i32,
    decoder: VAAPIDecoder,
    framerate: f32,
    pending_frames: [PendingFrame; 4],
}

pub enum DecodingWorkerCommand {
    AddClient(i32),
    RemoveClient(i32),
    ProcessFrameChunk,
}

struct ChunkMetadata {
    user_id: i32,
    parsed_correctly: bool,
}

pub struct DecodingWorker {
    command_rx: channel::Receiver<DecodingWorkerCommand>,
    active_clients: Vec<(i32, VideoStreamingClientState)>,

    used_chunks: HeapProd<OwnedEncodedVideoFrameChunk>,
    pending_chunks: HeapCons<(ChunkMetadata, OwnedEncodedVideoFrameChunk)>,
}

impl DecodingWorker {
    fn new(
        command_rx: channel::Receiver<DecodingWorkerCommand>,
        used_chunks: HeapProd<OwnedEncodedVideoFrameChunk>,
        pending_chunks: HeapCons<(ChunkMetadata, OwnedEncodedVideoFrameChunk)>,
    ) -> Self {
        Self {
            command_rx,
            used_chunks,
            pending_chunks,
            active_clients: vec![],
        }
    }

    fn process_frame_chunk(&mut self) {
        let Some((meta, chunk)) = self.pending_chunks.try_pop() else {
            return;
        };

        if !meta.parsed_correctly {
            _ = self.used_chunks.try_push(chunk);

            return;
        }
    }

    fn run(mut self) {
        let timeout = Duration::from_secs_f32(f32::MAX);

        loop {
            match self.command_rx.recv_timeout(timeout) {
                Ok(command) => match command {
                    DecodingWorkerCommand::AddClient(_) => {}
                    DecodingWorkerCommand::RemoveClient(_) => {}
                    DecodingWorkerCommand::ProcessFrameChunk => self.process_frame_chunk(),
                },
                Err(RecvTimeoutError::Timeout) => {
                    todo!("handle stale streams");
                }
                _ => unreachable!("Decoding worker should be always active"),
            }
        }
    }
}

pub struct VideoPlaybackController {
    command_tx: channel::Sender<DecodingWorkerCommand>,

    used_chunks: HeapCons<OwnedEncodedVideoFrameChunk>,
    pending_chunks: HeapProd<(ChunkMetadata, OwnedEncodedVideoFrameChunk)>,
}

impl VideoPlaybackController {
    fn process_frame(&mut self, user_id: i32, chunk_bytes: EncodedVideoBytes<'_>) {
        if let Some(mut chunk) = self.used_chunks.try_pop() {
            let result = chunk_bytes.parse(&mut chunk);

            _ = self.pending_chunks.try_push((
                ChunkMetadata {
                    user_id,
                    parsed_correctly: result.is_ok(),
                },
                chunk,
            ));
            _ = self
                .command_tx
                .send(DecodingWorkerCommand::ProcessFrameChunk);
        }
    }
}

pub fn init() -> VideoPlaybackController {
    const RING_SIZE: usize = 14;

    let buffer_ring = HeapRb::new(RING_SIZE);
    let (mut used_prod, used_cons) = buffer_ring.split();

    for _ in 0..RING_SIZE {
        _ = used_prod.try_push(OwnedEncodedVideoFrameChunk::default());
    }

    let buffer_ring = HeapRb::new(RING_SIZE);
    let (pending_prod, pending_cons) = buffer_ring.split();

    let (command_tx, command_rx) = channel::bounded::<DecodingWorkerCommand>(8);

    thread::Builder::new()
        .name("video-decoding-worker".to_string())
        .spawn(|| {
            let worker = DecodingWorker::new(command_rx, used_prod, pending_cons);

            worker.run();
        })
        .expect("Unable to spawn video-decoding-worker");

    VideoPlaybackController {
        command_tx,
        pending_chunks: pending_prod,
        used_chunks: used_cons,
    }
}
