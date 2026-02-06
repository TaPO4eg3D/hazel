use std::{
    marker::PhantomData,
    sync::{Arc, Weak},
    time::Duration,
};

use bytes::BytesMut;
use dashmap::DashMap;
use serde::{Serialize, de::DeserializeOwned};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::{
        mpsc::{self, Receiver as MPSCReceiver, Sender as MPSCSender},
        oneshot::{self, Sender as OneshotSender},
    },
    time,
};
use uuid::Uuid;

use crate::{common::{parse_rpc_method, parse_uuid, process_payload}, models::common::RPCNotification};

use anyhow::Result as AResult;

type UuidMap = Arc<DashMap<Uuid, OneshotSender<Vec<u8>>>>;

type KeyMapInner = DashMap<String, Vec<(Uuid, MPSCSender<Vec<u8>>)>>;

type KeyMap = Arc<KeyMapInner>;

#[derive(Clone, Debug)]
pub struct Connection {
    outcome_sender: MPSCSender<TCPTraffic>,

    /// Subscription on a specific response
    uuid_map: UuidMap,

    /// General subscription for an event
    key_map: KeyMap,
}

type TCPTraffic = (String, Vec<u8>);

pub struct Subscription<T> {
    uuid: Uuid,
    event: String,

    rx: MPSCReceiver<Vec<u8>>,

    key_map: Weak<KeyMapInner>,

    _marker: PhantomData<T>,
}

impl<T: DeserializeOwned> Subscription<T> {
    fn new(event: &str, key_map: Weak<KeyMapInner>) -> (MPSCSender<Vec<u8>>, Self) {
        let (tx, rx) = mpsc::channel(24);

        (
            tx,
            Self {
                uuid: Uuid::new_v4(),
                event: event.into(),
                rx,
                key_map,
                _marker: PhantomData,
            },
        )
    }

    pub async fn recv(&mut self) -> Option<T> {
        let data = self.rx.recv().await?;

        match rmp_serde::from_slice::<T>(&data) {
            Ok(data) => Some(data),
            Err(err) => {
                println!("Invalid data: {err:?}");

                None
            }
        }
    }
}

impl<T> Drop for Subscription<T> {
    fn drop(&mut self) {
        let Some(key_map) = self.key_map.upgrade() else {
            return;
        };

        let Some(mut subscriptions) = key_map.get_mut(&self.event) else {
            return;
        };

        subscriptions.retain(|(uuid, _)| *uuid != self.uuid);
    }
}

impl Connection {
    const TIMEOUT_SEC: usize = 10;

    async fn setup_tcp_reader_task(
        key_map: KeyMap,
        uuid_map: UuidMap,
        conn_sender: MPSCSender<()>,
        mut reader_recv: MPSCReceiver<OwnedReadHalf>,
    ) {
        let mut reader = None;
        let mut buf = BytesMut::with_capacity(1024);

        loop {
            if reader.is_none() {
                match reader_recv.recv().await {
                    Some(value) => reader = Some(value),
                    None => todo!(),
                }
            }

            // Safety: safe due to check above
            let _reader = reader.as_mut().unwrap();

            if buf.is_empty() {
                let bytes_read = _reader.read_buf(&mut buf).await.unwrap();

                // Connection is closed...
                if bytes_read == 0 {
                    // Notify parent tasks
                    if conn_sender.send(()).await.is_err() {
                        todo!();
                    }

                    // ...so we're waiting for a new reader
                    reader = None;

                    continue;
                }
            }

            // TODO: Handle errors properly
            let (method, bytes_read) = parse_rpc_method(&mut buf, _reader).await.expect("TODO");

            let (uuid, bytes_read) = parse_uuid(&mut buf, _reader, bytes_read + 1)
                .await
                .expect("TODO");
            let (payload_bytes, bytes_read) = process_payload(&mut buf, _reader, bytes_read)
                .await
                .expect("TODO");

            if let Some(uuid) = uuid {
                #[allow(clippy::collapsible_if)]
                if let Some((_, sender)) = uuid_map.remove(&uuid) {
                    _ = sender.send(payload_bytes.to_vec());
                }
            }

            if let Some(senders) = key_map.get(&method) {
                for (_, sender) in senders.iter() {
                    _ = sender.send(payload_bytes.to_vec()).await;
                }
            }

            if buf.len() > bytes_read {
                buf = buf.split_off(bytes_read);
            } else {
                buf.clear();
            }
        }
    }

    async fn setup_tcp_writer_task(
        conn_sender: MPSCSender<()>,
        mut outcome_recv: MPSCReceiver<TCPTraffic>,
        mut writer_recv: MPSCReceiver<OwnedWriteHalf>,
    ) {
        let mut writer = None;

        loop {
            if writer.is_none() {
                match writer_recv.recv().await {
                    Some(value) => writer = Some(value),
                    None => return,
                }
            }

            // Safety: safe due the condition above
            let _writer = writer.as_mut().unwrap();

            let (_, bytes) = match outcome_recv.recv().await {
                Some(value) => value,
                None => return,
            };

            // TODO: Implement cancellation on timeout?
            if _writer.write_all(&bytes).await.is_err() {
                if conn_sender.send(()).await.is_err() {
                    return;
                }

                writer = None;
            }
        }
    }

    pub async fn new(addr: String) -> AResult<Self> {
        let key_map: KeyMap = Arc::new(DashMap::new());
        let uuid_map: UuidMap = Arc::new(DashMap::new());

        // Channel for outcome traffic
        let (outcome_sender, outcome_recv) = mpsc::channel::<TCPTraffic>(16);

        // Channel to report when the connection is closed
        let (conn_sender, mut conn_recv) = mpsc::channel::<()>(16);

        // Channels to supply a new reader/writer in a case if the connection is closed
        let (reader_sender, reader_recv) = mpsc::channel::<OwnedReadHalf>(16);
        let (writer_sender, writer_recv) = mpsc::channel::<OwnedWriteHalf>(16);

        // Spawn a separate task to read data from a TCP socket
        tokio::spawn({
            let uuid_map = uuid_map.clone();
            let key_map = key_map.clone();

            let conn_sender = conn_sender.clone();

            async move {
                _ = Self::setup_tcp_reader_task(key_map, uuid_map, conn_sender, reader_recv).await;
            }
        });

        // Spawn a task to write data into a TCP socket
        tokio::spawn({
            async move {
                _ = Self::setup_tcp_writer_task(conn_sender, outcome_recv, writer_recv).await;
            }
        });

        let mut count = 0_usize;

        let _addr = addr.to_string();
        tokio::spawn(async move {
            loop {
                // Try to connect as much as it's needed
                println!("Connecting...");

                let stream = match TcpStream::connect(&_addr).await {
                    Ok(conn) => {
                        count = 0;
                        conn
                    }
                    Err(_) => {
                        count += 1;

                        let delay = Self::TIMEOUT_SEC * count;
                        println!("Unable to connect, retrying in {delay} seconds");

                        time::sleep(Duration::from_secs(delay as u64)).await;

                        continue;
                    }
                };

                println!("Connected!");

                // Split the stream on reader and writer
                let (reader, writer) = stream.into_split();
                reader_sender
                    .send(reader)
                    .await
                    .expect("Reader task shoud not die");

                writer_sender
                    .send(writer)
                    .await
                    .expect("Writer task shoud not die");

                // When we receive a message, it means the connection is closed
                conn_recv
                    .recv()
                    .await
                    .expect("Reader/Writer task should not die");

                println!("Lost the connection, retrying...")
            }
        });

        Ok(Self {
            key_map,
            uuid_map,
            outcome_sender,
        })
    }

    pub fn subscribe<Out>(&self) -> Subscription<Out>
    where
        Out: RPCNotification
    {
        let key_map = Arc::downgrade(&self.key_map);
        let (sender, subscription) = Subscription::new(Out::key(), key_map);

        let uuid = subscription.uuid;
        self.key_map
            .entry(Out::key().into())
            .and_modify({
                let sender = sender.clone();

                move |v| {
                    v.push((uuid, sender));
                }
            })
            .or_insert_with(move || vec![(uuid, sender)]);

        subscription
    }

    pub async fn execute<In, Out>(&self, key: &str, payload: &In) -> AResult<Out>
    where
        In: Serialize,
        Out: DeserializeOwned,
    {
        let key_bytes = key.as_bytes();
        let key_len = u8::try_from(key_bytes.len()).expect("Key is too large");

        let bytes = rmp_serde::to_vec(payload)?;
        let len = u32::try_from(bytes.len()).expect("Payload is too large");

        let mut data = Vec::<u8>::new();
        let uuid = Uuid::new_v4();

        data.push(key_len);
        data.extend_from_slice(key_bytes);

        data.push(true as u8);
        data.extend_from_slice(uuid.as_bytes());
        data.extend_from_slice(&len.to_le_bytes());
        data.extend_from_slice(&bytes);

        // First we setup the listener...
        let (tx, rx) = oneshot::channel();
        self.uuid_map.insert(uuid, tx);

        // ...then we send the data
        self.outcome_sender
            .send((key.into(), data))
            .await
            .expect("Should be alive");

        // Waiting for the response
        // TODO: Add timeout?
        let data = rx.await.expect("Handler should not be dropped");
        self.uuid_map.remove(&uuid);

        let data = rmp_serde::from_slice::<Out>(&data)?;

        Ok(data)
    }
}
