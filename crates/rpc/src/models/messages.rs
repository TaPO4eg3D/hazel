use serde::{Deserialize, Serialize};

use crate::models::markers::{GroupId, MediaId, MsgId, TextChannelId, UserId};


#[derive(Serialize, Deserialize, Debug)]
pub enum TextMessageChannel {
	TextChannel(TextChannelId),
	Direct(UserId),
	GroupChannel(GroupId),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MessageReply {
	pub reply_to: MsgId,
	/// Full copy of the quoting part because
	/// the original message can change
	pub reply_text: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MessageContent {
	pub attached_media: Vec<MediaId>,
	pub reply: Option<MessageReply>,

	pub content: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SendMessagePayload {
	pub content: MessageContent,
	pub destination: TextMessageChannel,
}
