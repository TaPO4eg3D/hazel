use capture::video::{
    encode::{EncoderParams, VideoEncoder},
    linux::screengrab::start_streaming,
};

#[tokio::main]
async fn main() {
    start_streaming().await.unwrap();
}
