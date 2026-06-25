use std::fmt::Debug;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

use crate::{client::Connection, server::RpcWriter};

#[derive(Serialize, Deserialize, Debug)]
pub enum TransportLayerErr {
    LargeBody,
    IncorrectPayload,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ServerErr {
    InternalErr,
    Transport(TransportLayerErr),
}

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum APIError<T: Debug> {
    Err(T),
    ServerErr(ServerErr),
    Unauthorized,
}

pub type APIResult<T, E> = Result<T, APIError<E>>;

pub trait RPCMethod {
    type Request: Serialize;
    type Response: DeserializeOwned + Debug;
    type ResponseError: DeserializeOwned + Debug;

    fn key() -> &'static str;

    #[allow(async_fn_in_trait)]
    async fn execute(
        connection: &Connection,
        payload: &Self::Request,
    ) -> APIResult<Self::Response, Self::ResponseError> {
        connection
            .execute(Self::key(), payload)
            .await
            .expect("invalid params") // TODO: Do not panic
    }
}

pub trait RPCNotification: Serialize + DeserializeOwned {
    fn key() -> &'static str;

    #[allow(async_fn_in_trait)]
    async fn notify(self, writer: &RpcWriter)
    where
        Self: Sized,
    {
        writer.write(Self::key().into(), self, None).await
    }
}
