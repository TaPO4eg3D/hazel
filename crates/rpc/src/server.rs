use std::{collections::HashMap, pin::Pin, sync::Arc};

use serde::{Serialize, de::DeserializeOwned};

use castaway::cast;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{tcp::OwnedReadHalf, TcpListener, TcpStream},
    sync::mpsc,
};

use anyhow::Result as AResult;
use bytes::BytesMut;

use rmp_serde::Serializer;
use uuid::Uuid;

use crate::common::{parse_rpc_method, parse_uuid, process_payload};

pub type DynHandler<C> = Box<
    dyn for<'a> Fn(
            Option<Uuid>,
            &'a mut BytesMut,
            &'a mut OwnedReadHalf,
            C,
            RpcWriter,
            usize,
        ) -> Pin<Box<dyn Future<Output = AResult<usize>> + Send + 'a>>
        + Send
        + Sync,
>;

#[derive(Clone, Debug)]
pub struct RpcWriter {
    inner: mpsc::Sender<Vec<u8>>,
}

impl RpcWriter {
    fn new(sender: mpsc::Sender<Vec<u8>>) -> Self {
        Self {
            inner: sender
        }
    }

    pub async fn write<T: Response>(&self, key: String, value: T, uuid: Option<Uuid>) {
        if let Some(body_bytes) = value.bytes() {
            let key_bytes = key.as_bytes();

            let key_len = u8::try_from(key_bytes.len())
                .expect("Key is way too big"); // TODO: Do not fail

            let body_len = u32::try_from(body_bytes.len())
                .expect("Body is way too big"); // TODO: Do not fail

            let mut response = Vec::<u8>::with_capacity(body_len as usize);

            response.push(key_len);
            response.extend_from_slice(key_bytes);

            if let Some(value) = uuid {
                response.push(true as u8);
                response.extend_from_slice(value.as_bytes())
            } else {
                response.push(false as u8);
            }

            response.extend_from_slice(&body_len.to_le_bytes());
            response.extend_from_slice(&body_bytes);

            let _ = self.inner.send(response).await;
        }
    }
}

pub struct RpcRouter<AppState, ConnState>
{
    state: AppState,
    on_connect_hook: Arc<dyn Fn(RpcWriter) -> ConnState + Send + Sync + 'static>,
    routing_table: HashMap<String, DynHandler<ConnState>>,
}

pub trait Response {
    fn bytes(&self) -> Option<Vec<u8>>;
}

impl<T: Serialize> Response for T {
    fn bytes(&self) -> Option<Vec<u8>> {
        if cast!(self, &()).is_ok() {
            None
        } else {
            let mut buf = Vec::new();

            self.serialize(&mut Serializer::new(&mut buf)).unwrap();

            Some(buf)
        }
    }
}

impl<AppState, ConnState> RpcRouter<AppState, ConnState>
where
    AppState: Clone + Send + Sync + 'static,
    ConnState: Clone + Send + Sync + 'static,
{
    pub fn new<F>(state: AppState, f: F) -> Self
    where
        F: Fn(RpcWriter) -> ConnState + Send + Sync + 'static
    {
        Self {
            state,
            on_connect_hook: Arc::new(f),
            routing_table: HashMap::new(),
        }
    }

    pub fn register<In, Out, F, Fut>(mut self, key: &str, handler: F) -> Self
    where
        In: DeserializeOwned + Send + 'static,
        Out: Response + Send,
        F: Fn(AppState, ConnState, In) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Out> + Send + 'static,
    {
        let _key = key.to_string();

        let wrapped: DynHandler<ConnState> = {
            let state = self.state.clone();
            let handler = Arc::new(handler);

            Box::new(move |uuid, buf, stream, conn_state, writer, start| {
                let _key = _key.to_string();

                let state = state.clone();
                let handler = Arc::clone(&handler);

                let fut = async move {
                    let (payload_bytes, bytes_read) = process_payload(buf, stream, start).await?;
                    let payload = rmp_serde::from_slice::<In>(payload_bytes)?;
                    let data = handler(state, conn_state, payload).await;

                    writer.write(_key, data, uuid).await;

                    Ok(bytes_read)
                };

                Box::pin(fut)
            })
        };

        self.routing_table.insert(key.into(), wrapped);
        println!("Table: {:?}", self.routing_table.keys());

        self
    }
}

async fn process_connection<AppState, ConnState>(
    router: Arc<RpcRouter<AppState, ConnState>>,
    stream: TcpStream,
) -> AResult<ConnState>
where
    AppState: Clone + Send + Sync + 'static,
    ConnState: Clone + Send + Sync + 'static
{
    let mut buf = BytesMut::with_capacity(1024);
    let (mut reader, mut writer) = stream.into_split();

    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(16);

    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if writer.write_all(&msg).await.is_err() {
                break;
            }
        }
    });

    let rpc_writer = RpcWriter::new(tx);
    let conn_state = (router.on_connect_hook)(rpc_writer.clone());

    loop {
        if buf.is_empty() {
            let bytes_read = reader.read_buf(&mut buf).await?;

            if bytes_read == 0 {
                return Ok(conn_state);
            }
        }

        let (method, bytes_read) = parse_rpc_method(&mut buf, &mut reader).await?;
        let (uuid, bytes_read) = parse_uuid(&mut buf, &mut reader, bytes_read + 1).await?;

        let f = router.routing_table.get(&method)
            .unwrap(); // TODO: Do not fail and report incorrect endpoint name

        let conn_state = conn_state.clone();
        let rpc_writer = rpc_writer.clone();

        let bytes_read = (f)(uuid, &mut buf, &mut reader, conn_state, rpc_writer, bytes_read).await?;

        if buf.len() > bytes_read {
            buf = buf.split_off(bytes_read);
        } else {
            buf.clear();
        }
    }
}

pub async fn serve<AppState, ConnState>(
    addr: &str,
    router: RpcRouter<AppState, ConnState>,
    on_disconnect: impl Fn(AppState, ConnState) -> Pin<
        Box<dyn Future<Output = ()> + Send + Sync>
    > + Send + Sync + 'static,
)
where
    AppState: Clone + Send + Sync + 'static,
    ConnState: Clone + Send + Sync + 'static,
{
    let listener = TcpListener::bind(addr)
        .await
        .expect("Failed to open a TCP Listener");

    let router = Arc::new(router);
    let on_disconnect = Arc::new(on_disconnect);

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                println!("Got a connection: {addr}");

                let router = Arc::clone(&router);
                let on_disconnect = on_disconnect.clone();

                tokio::spawn(async move {
                    let state = router.state.clone();

                    let conn_state = process_connection(router, stream)
                        .await
                        .unwrap();

                    on_disconnect(state, conn_state).await;
                });
            }
            Err(_) => {
                todo!();
            }
        }
    }
}
