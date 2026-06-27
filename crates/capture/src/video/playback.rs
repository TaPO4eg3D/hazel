use std::{thread, time::Duration};

use crossbeam::channel::{self, RecvTimeoutError};
use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Producer, Split as _},
};
use rpc::models::voice::VideoSessionParams;
use streaming_common::{EncodedVideoBytes, OwnedEncodedVideoFrameChunk, StreamPacketHeader};

use crate::video::decode::VAAPIDecoder;

#[derive(Default)]
struct Chunk {
    processed: bool,
    data: Vec<u8>,
}

impl Chunk {
    fn reset(&mut self) {
        self.processed = false;
        self.data.clear();
    }

    fn set(&mut self, data: &[u8]) {
        self.processed = true;

        self.data.clear();
        self.data.extend_from_slice(&data);
    }
}

#[derive(Default)]
struct PendingFrame {
    header: StreamPacketHeader,
    processed_chunks: u32,
    in_constuction: bool,
    chunks: Vec<Chunk>,
}

impl PendingFrame {
    fn reset(&mut self) {
        self.in_constuction = false;

        for chunk in self.chunks.iter_mut() {
            chunk.reset();
        }
    }

    fn merge_chunk(&mut self, incoming_chunk: &OwnedEncodedVideoFrameChunk) {
        self.in_constuction = true;
        self.header = incoming_chunk.header.clone();

        // At first we need to make sure we have enough space to store the frame
        let total_chunks = incoming_chunk.header.shards_total as usize
            + incoming_chunk.header.recovery_shards as usize;

        if self.chunks.len() < total_chunks as usize {
            let diff = total_chunks - self.chunks.len();

            for _ in 0..diff {
                self.chunks.push(Chunk::default());
            }
        }

        let chunk = &mut self.chunks[incoming_chunk.header.shard as usize];
        if chunk.processed {
            // is it possible?
            log::warn!("Chunk {} alredy processed", incoming_chunk.header.seq);

            return;
        }

        chunk.set(&incoming_chunk.data);
    }
}

struct VideoStreamingClientState {
    decoder: VAAPIDecoder,
    framerate: f32,
    pending_frames: [PendingFrame; 4],
}

impl VideoStreamingClientState {
    fn process_chunk(&mut self, chunk: &OwnedEncodedVideoFrameChunk) {
        // At first we're trying to find a frame in construction
        // with the matching seq
        if let Some(frame) = self
            .pending_frames
            .iter_mut()
            .find(|frame| frame.in_constuction && frame.header.seq == chunk.header.seq)
        {
            frame.merge_chunk(chunk);
        }

        // Nothing found, that's a new frame.
        // Trying to find a frame we can use
        if let Some(frame) = self
            .pending_frames
            .iter_mut()
            .find(|frame| !frame.in_constuction)
        {
            frame.merge_chunk(chunk);
        }

        // All frames are claimed, remove one with the lowest seq
        let frame = self
            .pending_frames
            .iter_mut()
            .min_by_key(|frame| frame.header.seq)
            .expect("We should always have at least one such frame");

        frame.merge_chunk(chunk);
    }
}

pub enum DecodingWorkerCommand {
    AddClient((i32, VideoSessionParams)),
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

        if let Some((_, client)) = self
            .active_clients
            .iter_mut()
            .find(|(user_id, _)| user_id == &meta.user_id)
        {
            client.process_chunk(&chunk);
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
