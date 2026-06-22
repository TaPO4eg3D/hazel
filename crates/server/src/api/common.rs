use rpc::models::common::{APIError, APIResult, RPCMethod};
use sea_orm::DbErr;

use crate::{AppState, ConnectionState};

pub trait DbErrReponseCompat {
    fn into_api_error<E: std::fmt::Debug>(self) -> APIError<E>;
}

impl DbErrReponseCompat for DbErr {
    fn into_api_error<E: std::fmt::Debug>(self) -> APIError<E> {
        log::error!("Database Error: {self:?}");

        APIError::ServerError
    }
}

pub trait RPCHandle: RPCMethod {
    async fn handle(
        app_state: AppState,
        connection_state: ConnectionState,
        req: Self::Request,
    ) -> APIResult<Self::Response, Self::ResponseError>;

    async fn build(
        app_state: AppState,
        connection_state: ConnectionState,
        req: Self::Request,
    ) -> APIResult<Self::Response, Self::ResponseError> {
        if let Ok(value) = connection_state.read() {
            if !value.is_authenticated() {
                return Err(APIError::Unauthorized);
            }
        } else {
            log::error!("Poisoned ConnectionState lock");

            return Err(APIError::Unauthorized);
        }

        Self::handle(app_state, connection_state, req).await
    }
}

pub trait NoAuthRPCHandle: RPCMethod {
    async fn handle(
        app_state: AppState,
        connection_state: ConnectionState,
        req: Self::Request,
    ) -> APIResult<Self::Response, Self::ResponseError>;

    async fn build(
        app_state: AppState,
        connection_state: ConnectionState,
        req: Self::Request,
    ) -> APIResult<Self::Response, Self::ResponseError> {
        Self::handle(app_state, connection_state, req).await
    }
}
