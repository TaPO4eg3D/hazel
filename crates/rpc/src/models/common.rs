use std::fmt::Debug;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

use crate::{client::Connection, server::RpcWriter};

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum APIError<T: Debug> {
    Err(T),
    ServerError,
    Unauthorized,
}

pub type APIResult<T, E> = Result<T, APIError<E>>;

#[macro_export]
macro_rules! check_auth {
    ($conn_state:ident) => {
        if let Ok(value) = $conn_state.read() {
            if !value.is_authenticated() {
                return Err(APIError::Unauthorized);
            }
        } else {
            log::error!("Poisoned ConnectionState lock");

            return Err(APIError::Unauthorized);
        }
    };
}

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
            .expect("invalid params")
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
