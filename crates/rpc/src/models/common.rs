use std::{fmt::Debug, marker::PhantomData};

use serde::{Deserialize, Serialize};
use thiserror::Error;

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
