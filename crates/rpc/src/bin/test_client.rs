use rpc::{
    client::Connection, common::Empty, models::{
        auth::{GetCurrentUserError, GetSessionKeyError, GetSessionKeyPayload, GetSessionKeyResponse, LoginError, LoginPayload, SessionKey},
        common::Id,
        messages::{MessageContent, SendMessagePayload, TextMessageChannel, User},
    }
};

#[tokio::main]
async fn main() {
    let connection = Connection::new("localhost:9898").await.unwrap();

    let data: Result<GetSessionKeyResponse, GetSessionKeyError> = connection
        .execute(
            "GetSessionKey",
            &GetSessionKeyPayload {
                login: "test".into(),
                password: "test".into(),
            },
        )
        .await
        .unwrap();

    println!("{data:?}");

    // let session_key = data.unwrap();
    // println!("{session_key:?} ({})", session_key.verify(b"TODO"));
    //
    // let data: Result<(), LoginError> = connection
    //     .execute(
    //         "Login",
    //         &LoginPayload {
    //             session_key,
    //         }
    //     )
    //     .await
    //     .unwrap();
    //
    // println!("{data:?}");
    //
    // let current_user: Result<Option<i32>, GetCurrentUserError> = connection
    //     .execute("GetCurrentUser", &Empty {})
    //     .await
    //     .unwrap();
    //
    // println!("{current_user:?}");
}
