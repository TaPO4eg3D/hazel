use rpc::{models::common::{APIError, RPCMethod}, server::RpcRouter};
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

pub trait RPCServer: RPCMethod {
    async fn handle(
        app_state: AppState,
        connection_state: ConnectionState,
        req: Self::Request,
    ) -> Self::Response;
}

#[macro_export]
macro_rules! register_endpoints {
    ($router:expr, $($endpoint:ident),+ $(,)?) => {
        $router
            $(
                .register(
                    $endpoint::key(), 
                    $endpoint::handle
                )
            )+
    };
}
