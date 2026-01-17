use rpc::models::common::APIError;
use sea_orm::DbErr;

pub trait DbErrReponseCompat {
    fn into_api_error<E: std::fmt::Debug>(self) -> APIError<E>;
}

impl DbErrReponseCompat for DbErr {
    fn into_api_error<E: std::fmt::Debug>(self) -> APIError<E> {
        log::error!("Database Error: {self:?}");

        APIError::ServerError
    }
}
