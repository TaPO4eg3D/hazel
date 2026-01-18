use std::{io, str::Utf8Error};

use bytes::BytesMut;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncReadExt;

use uuid::Uuid;

#[derive(Error, Debug)]
pub enum RpcError {
    #[error("TCP Stream is closed")]
    ConnectionClosed,
    #[error("TCP Io Error")]
    TCPIoError(#[from] io::Error),
    #[error("Error while processing user data (payload)")]
    BodyDeserializeError(#[from] rmp_serde::decode::Error),
    #[error("Key is invalid UTF-8 string")]
    KeyDeserializeError(#[from] Utf8Error),
    #[error("Invalid UUID")]
    InvalidUUID,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct Empty {}

pub async fn parse_rpc_method<T: AsyncReadExt + Unpin>(
    buf: &mut BytesMut,
    stream: &mut T,
) -> Result<(String, usize), RpcError> {
    let key_bytes = buf[0] as usize;

    // Read more in case if needed
    while buf.len() - 1 < key_bytes {
        let bytes_read = stream.read_buf(buf).await?;

        if bytes_read == 0 {
            return Err(RpcError::ConnectionClosed);
        }
    }

    let key = &buf[1..=key_bytes];
    let key = std::str::from_utf8(key)?;

    Ok((key.into(), key_bytes))
}

pub async fn parse_uuid<T: AsyncReadExt + Unpin>(
    buf: &mut BytesMut,
    stream: &mut T,
    start: usize,
) -> Result<(Option<Uuid>, usize), RpcError> {
    let is_tagged = buf[start];

    if is_tagged == 0 {
        return Ok((None, start + 1));
    }

    const UUID_LEN: usize = std::mem::size_of::<Uuid>();

    // Read more in case if needed
    while buf.len() - start < std::mem::size_of::<Uuid>() {
        let bytes_read = stream.read_buf(buf).await?;

        if bytes_read == 0 {
            return Err(RpcError::ConnectionClosed);
        }
    }

    let i = start + 1;
    let uuid: [u8; 16] = buf[i..i + UUID_LEN]
        .try_into()
        .map_err(|_| RpcError::InvalidUUID)?;

    let uuid = Uuid::from_bytes(uuid);

    Ok((Some(uuid), i + UUID_LEN))
}

pub async fn process_payload<'a, T: AsyncReadExt + Unpin>(
    buf: &'a mut BytesMut,
    stream: &mut T,
    start: usize,
) -> Result<(&'a [u8], usize), RpcError> {
    // Length of body is stored in four bytes
    while buf.len() - start < 4 {
        let bytes_read = stream.read_buf(buf).await?;

        if bytes_read == 0 {
            return Err(RpcError::ConnectionClosed);
        }
    }

    let body_length: [u8; 4] = buf[start..start + 4].try_into().unwrap();
    let body_length = u32::from_le_bytes(body_length) as usize;

    while buf.len() - (start + 4) < body_length {
        let bytes_read = stream.read_buf(buf).await?;

        if bytes_read == 0 {
            return Err(RpcError::ConnectionClosed);
        }
    }

    let body_start = start + 4;
    let body_end = body_start + body_length;

    let body = &buf[body_start..body_end];

    Ok((body, body_end))
}

