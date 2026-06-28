use std::{thread, time::Duration};

use crossbeam::channel::{self, RecvTimeoutError};
use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Producer, Split as _},
};
use rpc::models::{markers::UserId, voice::VideoSessionParams};
use streaming_common::{EncodedVideoBytes, OwnedEncodedVideoFrameChunk, StreamPacketHeader};

use crate::video::decode::{VAAPIDecoder, VAAPIDecoderParams};

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
    data_shards: usize,
    recovery_shards: usize,
    /// We are reusing `PendingFrame`s,
    /// this flag is used to indicate if the frame
    /// is in the process of aggregating chunks.
    /// You're supposted to raise this flag when you're
    /// claiming the frame and put it down once you're done.
    /// Maybe there's a better way to do it but it is what it is
    in_constuction: bool,
    chunks: Vec<Chunk>,
    data: Vec<u8>,
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
        let total_chunks = incoming_chunk.header.total_chunks();
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
        if incoming_chunk.header.is_fec() {
            self.recovery_shards += 1;
        } else {
            self.data_shards += 1;
        }
    }

    fn process(&mut self) -> bool {
        let total_processed = self.data_shards + self.recovery_shards;
        if total_processed < self.header.data_shards as usize {
            return false;
        }
        // We have enough chunks, it's time to build the frame
        println!(
            "FRAME: {:#?} ({}, {})",
            self.header, self.data_shards, self.recovery_shards
        );

        self.data.clear();

        let fec_needed = self.data_shards < self.header.data_shards as usize;
        if fec_needed {
            todo!("Implement FEC");
        } else {
            for chunk in self.chunks.iter() {
                if !chunk.processed {
                    continue;
                }

                self.data.extend_from_slice(&chunk.data);
            }
        }

        self.data.truncate(self.header.data_size as usize);

        true
    }
}

struct VideoStreamingClientState {
    decoder: VAAPIDecoder,
    pending_frames: Vec<PendingFrame>,
    next_seq: u64,
}

impl VideoStreamingClientState {
    fn new(decoder: VAAPIDecoder) -> Self {
        let pending_frames = (0..4).map(|_| PendingFrame::default()).collect::<Vec<_>>();

        Self {
            next_seq: 0,
            decoder,
            pending_frames,
        }
    }

    fn process_chunk(&mut self, chunk: &OwnedEncodedVideoFrameChunk) {
        // Do not process late chunks
        if chunk.header.seq < self.next_seq {
            println!("late: {}", chunk.header.seq);
            return;
        }

        // At first we're trying to find a frame in construction with the matching seq
        let frame = if let Some(frame) = self
            .pending_frames
            .iter_mut()
            .find(|frame| frame.in_constuction && frame.header.seq == chunk.header.seq)
        {
            println!("found frame");
            frame
        } else if let Some(frame) = self // Nothing found, that's a new frame. Trying to find a frame we can use
            .pending_frames
            .iter_mut()
            .find(|frame| !frame.in_constuction)
        {
            println!("new frame");
            frame
        } else {
            println!("no vacant frames");
            // All frames are claimed, remove one with the lowest seq
            self.pending_frames
                .iter_mut()
                .min_by_key(|frame| frame.header.seq)
                .expect("We should always have at least one such frame")
        };

        frame.merge_chunk(chunk);
        if frame.process() {
            println!("FRAME: {:?}", frame.data);
            self.decoder.decode(&frame.data);
            if let Some(decoded_frame) = self.decoder.frame_queue.pop_front() {
                println!("Decoded frame!");
            }

            self.next_seq = chunk.header.seq + 1;
            frame.reset();
        }
    }
}

pub enum DecodingWorkerCommand {
    AddClient((UserId, VideoSessionParams)),
    RemoveClient(UserId),
    ProcessFrameChunk,
}

struct ChunkMetadata {
    user_id: UserId,
    parsed_correctly: bool,
}

pub struct DecodingWorker {
    command_rx: channel::Receiver<DecodingWorkerCommand>,
    active_clients: Vec<(UserId, VideoStreamingClientState)>,

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

        _ = self.used_chunks.try_push(chunk);
    }

    fn add_client(&mut self, user_id: UserId, params: VideoSessionParams) {
        if self
            .active_clients
            .iter()
            .any(|(client_id, _)| *client_id == user_id)
        {
            return;
        }

        let client = VideoStreamingClientState::new(VAAPIDecoder::new(VAAPIDecoderParams {
            width: params.width,
            height: params.height,
        }));

        self.active_clients.push((user_id, client));
    }

    fn run(mut self) {
        loop {
            match self.command_rx.recv() {
                Ok(command) => match command {
                    DecodingWorkerCommand::AddClient((user_id, params)) => {
                        self.add_client(user_id, params)
                    }
                    DecodingWorkerCommand::RemoveClient(_) => todo!("Implement client removal"),
                    DecodingWorkerCommand::ProcessFrameChunk => self.process_frame_chunk(),
                },
                // Err(RecvTimeoutError::Timeout) => {
                //     todo!("handle stale streams");
                // }
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
    pub fn get_command_tx(&self) -> channel::Sender<DecodingWorkerCommand> {
        self.command_tx.clone()
    }

    pub fn process_frame(&mut self, user_id: UserId, chunk_bytes: EncodedVideoBytes<'_>) {
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
