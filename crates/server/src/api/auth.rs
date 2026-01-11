use rpc::{
    common::Empty,
    models::auth::{
        GetCurrentUserError, GetSessionKeyError, GetSessionKeyPayload, GetSessionKeyResponse,
        LoginError, LoginPayload, SessionKey,
    },
    server::RpcRouter,
};

use sha2::{Digest, Sha256};

use crate::{AppState, ConnectionState};
use crate::entity::user::{self, Entity as User};

use sea_orm::{DbErr, entity::*, query::*};

const KEY: &[u8] = b"TODO";

async fn get_session_key(
    state: AppState,
    _conn_state: ConnectionState,
    GetSessionKeyPayload { login, password }: GetSessionKeyPayload,
) -> Result<GetSessionKeyResponse, GetSessionKeyError> {
    let password = Sha256::digest(password.as_bytes());
    let password = format!("{:x}", password);

    let user = User::find()
        .filter(user::Column::Username.eq(&login))
        .one(&state.db)
        .await
        .map_err(|err| {
            log::error!("Failure when fetching a User: {:?} ({})", login, err,);

            GetSessionKeyError::ServerError
        })?;

    match user {
        Some(user) => {
            if user.password == password {
                let key = SessionKey::new(user.id, KEY);

                Ok(GetSessionKeyResponse::ExistingUser(key))
            } else {
                Err(GetSessionKeyError::UserAlreadyExists)
            }
        }
        None => {
            let user = user::ActiveModel {
                username: Set(login),
                password: Set(password),
                banned: Set(false),
                ..Default::default()
            };

            let user = user.insert(&state.db).await.map_err(|err| match err {
                DbErr::RecordNotInserted => GetSessionKeyError::UserAlreadyExists,
                _ => {
                    log::error!("Error while inserting: {err}");

                    GetSessionKeyError::ServerError
                }
            })?;

            let key = SessionKey::new(user.id, KEY);

            Ok(GetSessionKeyResponse::NewUser(key))
        }
    }
}

async fn login(
    state: AppState,
    conn_state: ConnectionState,
    LoginPayload { session_key }: LoginPayload,
) -> Result<(), LoginError> {
    if !session_key.verify(b"TODO") {
        return Err(LoginError::InvalidSesssionKey);
    }

    if session_key.is_expired() {
        return Err(LoginError::SessionKeyExpired);
    }

    let user = User::find()
        .filter(user::Column::Id.eq(session_key.body.user_id))
        .one(&state.db)
        .await
        .map_err(|err| {
            log::error!("Failure when fetching a User: {:?} ({})", session_key, err,);

            LoginError::ServerError
        })?
        .ok_or(LoginError::UserNotFound)?;

    let mut conn_state = conn_state.write().map_err(|err| {
        log::error!(
            "Failed to get a connection state: {:?} ({})",
            session_key,
            err,
        );

        LoginError::ServerError
    })?;

    conn_state.user = Some(user);

    Ok(())
}

async fn get_current_user(
    _state: AppState,
    conn_state: ConnectionState,
    _: Empty,
) -> Result<Option<i32>, GetCurrentUserError> {
    let conn_state = conn_state.read().map_err(|err| {
        log::error!("Failed to get a connection state: {}", err,);

        GetCurrentUserError::ServerError
    })?;

    Ok(conn_state.user.as_ref().map(|user| user.id))
}

pub fn merge(router: RpcRouter<AppState, ConnectionState>) -> RpcRouter<AppState, ConnectionState> {
    router
        .register("Login", login)
        .register("GetCurrentUser", get_current_user)
        .register("GetSessionKey", get_session_key)
}
