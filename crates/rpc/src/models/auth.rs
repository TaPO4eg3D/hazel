use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use rpc_macros::rpc_method;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

#[derive(Serialize, Deserialize, Debug)]
pub struct LoginPayload {
    pub session_key: SessionKey,
}

#[derive(Serialize, Deserialize)]
#[derive(Error, Debug)]
pub enum LoginError {
    #[error("Session Key is malformed")]
    InvalidSesssionKey,
    #[error("Session Key is expired")]
    SessionKeyExpired,
    #[error("Wasn't able to find requested User")]
    UserNotFound,
}

#[rpc_method]
pub struct Login {
    request: LoginPayload,
    response: (),
    error: LoginError,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GetSessionKeyPayload {
    pub login: String,
    pub password: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SessionKeyBody {
    pub user_id: i32,
    pub expires_at: i64,
}

impl SessionKeyBody {
    fn create_mac(&self, key: &[u8]) -> HmacSha256 {
        let mut mac = HmacSha256::new_from_slice(key)
            .expect("HMAC can take key of any size");

        let mut payload = Vec::<u8>::new();

        payload.extend_from_slice(&self.user_id.to_le_bytes());
        payload.extend_from_slice(&self.expires_at.to_le_bytes());

        mac.update(&payload);

        mac
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SessionKey {
    pub body: SessionKeyBody,
    pub sign: Vec<u8>,
}

impl SessionKey {
    pub fn new(user_id: i32, key: &[u8]) -> Self {
        let expires_at = Utc::now() + Duration::days(1); // TODO: Change it
        let timestamp = expires_at.timestamp();

        let body = SessionKeyBody {
            user_id,
            expires_at: timestamp,
        };

        let sign = body.create_mac(key)
            .finalize()
            .into_bytes()
            .to_vec();

        Self {
            body,
            sign,
        }
    }

    pub fn is_expired(&self) -> bool {
        let now = Utc::now();
        let expires_at = match DateTime::from_timestamp(self.body.expires_at, 0) {
            Some(value) => value,
            None => {
                log::error!(
                    "Can't create DateTime from the UNIX timestamp: {}",
                    self.body.expires_at,
                );

                return true;
            },
        };

        expires_at <= now
    }

    pub fn verify(&self, key: &[u8]) -> bool {
        let mac = self.body.create_mac(key);

        mac.verify_slice(&self.sign).is_ok()
    }
}

#[derive(Serialize, Deserialize)]
#[derive(Debug)]
pub enum GetSessionKeyResponse {
    ExistingUser(SessionKey),
    NewUser(SessionKey),
}

#[derive(Serialize, Deserialize)]
#[derive(Error, Debug)]
pub enum GetSessionKeyError {
    #[error("User with this login already exists")]
    UserAlreadyExists,
    #[error("Server Error")]
    ServerError,
}

#[rpc_method]
pub struct GetSessionKey {
    request: GetSessionKeyPayload,
    response: GetSessionKeyResponse,
    error: GetSessionKeyError,
}

#[derive(Serialize, Deserialize)]
#[derive(Error, Debug)]
pub enum GetCurrentUserError {
    #[error("Server Error")]
    ServerError,
}

#[rpc_method]
pub struct GetCurrentUser {
    request: (),
    response: Option<i32>,
    error: GetCurrentUserError,
}
