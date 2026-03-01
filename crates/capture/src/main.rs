use capture::video::encode::{EncoderParams, VideoEncoder};

fn main() {
    let encoder = VideoEncoder::new(EncoderParams {
        codec_name: "h264_vaapi",
        width: 1920,
        height: 1080,
    });
}
