use chrono::Utc;
use rpc::{
    models::{
        auth::{
            GetCurrentUserError, GetSessionKey, GetSessionKeyError, GetSessionKeyPayload,
            GetSessionKeyResponse, GetUserInfo, GetUserPayload, Login, LoginError, LoginPayload,
            SessionKey, UserInfo,
        },
        common::{APIError, APIResult, RPCMethod as _, RPCNotification},
        general::{UserConnectionUpdate, UserConnectionUpdateMessage},
        markers::TaggedEntity,
    },
    register_endpoints,
};

use sha2::{Digest, Sha256};

use crate::{
    AppRouter, AppState, ConnectionState,
    api::common::{DbErrReponseCompat as _, RPCHandle},
};
use crate::{
    api::common::NoAuthRPCHandle,
    entity::user::{self, Entity as User},
};

use sea_orm::{DbErr, entity::*, query::*};

const KEY: &[u8] = b"TODO";

impl NoAuthRPCHandle for GetSessionKey {
    async fn handle(
        app_state: AppState,
        _connection_state: ConnectionState,
        GetSessionKeyPayload { login, password }: GetSessionKeyPayload,
    ) -> APIResult<GetSessionKeyResponse, GetSessionKeyError> {
        let password = Sha256::digest(password.as_bytes());
        let password = format!("{:x}", password);

        let user = User::find()
            .filter(user::Column::Username.eq(&login))
            .one(&app_state.db)
            .await
            .map_err(DbErr::into_api_error)?;

        match user {
            Some(user) => {
                if user.password == password {
                    let key = SessionKey::new(user.id, KEY);

                    Ok(GetSessionKeyResponse::ExistingUser(key))
                } else {
                    Err(APIError::Err(GetSessionKeyError::UserAlreadyExists))
                }
            }
            None => {
                let user = user::ActiveModel {
                    username: Set(login),
                    password: Set(password),
                    banned: Set(false),
                    created_at: Set(Utc::now().naive_utc()),
                    ..Default::default()
                };

                let user = user.insert(&app_state.db).await.map_err(|err| match err {
                    DbErr::RecordNotInserted => {
                        APIError::Err(GetSessionKeyError::UserAlreadyExists)
                    }
                    _ => err.into_api_error(),
                })?;

                let key = SessionKey::new(user.id, KEY);

                Ok(GetSessionKeyResponse::NewUser(key))
            }
        }
    }
}

impl NoAuthRPCHandle for Login {
    async fn handle(
        app_state: AppState,
        connection_state: ConnectionState,
        LoginPayload { session_key }: LoginPayload,
    ) -> APIResult<(), LoginError> {
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
            let mut state = connection_state.write()?;

            state.user = Some(user);
        }

        let mut writers = vec![];
        for client in app_state.connected_clients.iter() {
            writers.push(client.read()?.writer.clone());
        }

        for writer in writers {
            UserConnectionUpdate {
                user_id,
                message: UserConnectionUpdateMessage::UserConnected,
            }
            .notify(&writer)
            .await;
        }

        app_state
            .connected_clients
            .insert(user_id, connection_state);

        Ok(())
    }
}

impl RPCHandle for GetUserInfo {
    async fn handle(
        app_state: AppState,
        _connection_state: ConnectionState,
        GetUserPayload { id }: GetUserPayload,
    ) -> APIResult<Option<UserInfo>, GetCurrentUserError> {
        let user = User::find_by_id(id.value)
            .one(&app_state.db)
            .await
            .map_err(DbErr::into_api_error)?;

        Ok(user.map(|user| UserInfo {
            id: user.tagged_id(),
            username: user.username,
        }))
    }
}

pub fn register(router: AppRouter) -> AppRouter {
    register_endpoints!(router, Login, GetUserInfo, GetSessionKey)
}
