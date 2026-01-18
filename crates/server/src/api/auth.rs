use rpc::{
    common::Empty,
    models::{
        auth::{
            GetCurrentUser, GetCurrentUserError, GetSessionKeyError, GetSessionKeyPayload,
            GetSessionKeyResponse, Login, LoginError, LoginPayload, SessionKey,
        },
        common::{APIError, RPCMethod as _},
        markers::TaggedEntity,
    },
    server::RpcRouter,
};

use sha2::{Digest, Sha256};

use crate::{AppState, ConnectionState, api::common::{DbErrReponseCompat as _, RPCServer}};
use crate::{
    entity::user::{self, Entity as User},
    register_endpoints,
};

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

impl RPCServer for Login {
    async fn handle(
        app_state: AppState,
        connection_state: ConnectionState,
        LoginPayload { session_key }: LoginPayload,
    ) -> Self::Response {
        if !session_key.verify(b"TODO") {
            return Err(APIError::Err(LoginError::InvalidSesssionKey));
        }

        if session_key.is_expired() {
            return Err(APIError::Err(LoginError::SessionKeyExpired));
        }

        let user = User::find()
            .filter(user::Column::Id.eq(session_key.body.user_id))
            .one(&app_state.db)
            .await
            .map_err(DbErr::into_api_error)?
            .ok_or(APIError::Err(LoginError::UserNotFound))?;
        let user_id = user.tagged_id();

        {
            let mut state = connection_state.write().unwrap();

            state.user = Some(user);
        }

        app_state.connected_clients.insert(user_id, connection_state);

        Ok(())
    }
}

impl RPCServer for GetCurrentUser {
    async fn handle(
        _app_state: AppState,
        connection_state: ConnectionState,
        _req: Self::Request,
    ) -> Self::Response {
        let conn_state = connection_state.read().unwrap();

        Ok(conn_state.user.as_ref().map(|user| user.id))
    }
}

pub fn merge(router: RpcRouter<AppState, ConnectionState>) -> RpcRouter<AppState, ConnectionState> {
    let router = router
        .register("GetSessionKey", get_session_key);

    register_endpoints!(router, GetCurrentUser, Login)
}
