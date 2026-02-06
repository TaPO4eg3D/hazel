use rpc::models::{
        auth::{
            GetSessionKey, GetSessionKeyError, GetSessionKeyPayload, GetSessionKeyResponse, GetUserInfo, GetUserPayload, Login, LoginError, LoginPayload, SessionKey, UserInfo
        }, common::{APIError, RPCMethod as _, RPCNotification}, general::{UserConnectionUpdate, UserConnectionUpdateMessage}, markers::TaggedEntity
    };

use sha2::{Digest, Sha256};

use crate::{
    AppState, ConnectionState, GlobalRouter, api::common::{DbErrReponseCompat as _, RPCHandle}
};
use crate::{
    entity::user::{self, Entity as User},
    register_endpoints,
};

use sea_orm::{DbErr, entity::*, query::*};

const KEY: &[u8] = b"TODO";

impl RPCHandle for GetSessionKey {
    async fn handle(
        app_state: AppState,
        _connection_state: ConnectionState,
        GetSessionKeyPayload { login, password }: GetSessionKeyPayload,
    ) -> Self::Response {
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
                    ..Default::default()
                };

                let user = user.insert(&app_state.db).await.map_err(|err| match err {
                    DbErr::RecordNotInserted => APIError::Err(GetSessionKeyError::UserAlreadyExists),
                    _ => err.into_api_error()
                })?;

                let key = SessionKey::new(user.id, KEY);

                Ok(GetSessionKeyResponse::NewUser(key))
            }
        }
    }
}

impl RPCHandle for Login {
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

        let writers = app_state
            .connected_clients
            .iter()
            .map(|user| user.read().unwrap().writer.clone())
            .collect::<Vec<_>>();

        for writer in writers {
            UserConnectionUpdate {
                user_id,
                message: UserConnectionUpdateMessage::UserConnected,
            }.notify(&writer).await;
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
    ) -> Self::Response {
        let user = User::find_by_id(id.value)
            .one(&app_state.db)
            .await
            .map_err(DbErr::into_api_error)?;

        Ok(user
            .map(|user| UserInfo {
                id: user.tagged_id(),
                username: user.username,
            }))
    }
}

pub fn merge(router: GlobalRouter) -> GlobalRouter {
    register_endpoints!(router, Login, GetUserInfo, GetSessionKey)
}
