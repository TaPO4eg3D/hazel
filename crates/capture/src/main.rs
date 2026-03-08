use capture::video::{
    encode::{VAAPIEncoderParams, VAAPIEncoder},
    linux::screengrab::start_streaming,
};

#[tokio::main]
async fn main() {
    start_streaming().await.unwrap();
}
