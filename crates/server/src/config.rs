use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct VoiceChannel {
    pub name: String,
    #[serde(default)]
    pub max_participants: u32
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TextChannel {
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    /// TCP address and port
    pub tcp_addr: String,
    /// UDP address and port
    pub udp_addr: String,

    /// List of text channels that will be present on the server
    pub text_channels: Vec<TextChannel>,

    /// List of voice channels that will be present on the server
    pub voice_channels: Vec<TextChannel>,
}
