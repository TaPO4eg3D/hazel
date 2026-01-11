use serde::{Deserialize, Serialize};

use crate::models::common::Id;

#[derive(Hash, PartialEq, Eq, Debug)]
pub struct User;

#[derive(Hash, PartialEq, Eq, Debug)]
pub struct Media;

#[derive(Hash, PartialEq, Eq, Debug)]
pub struct Message;

#[derive(Hash, PartialEq, Eq, Debug)]
pub struct VoiceChannel;

#[derive(Hash, PartialEq, Eq, Debug)]
pub struct TextChannel;

#[derive(Hash, PartialEq, Eq, Debug)]
pub struct Group;

pub type MsgId = Id<Message>;
pub type MediaId = Id<Media>;

pub type UserId = Id<User>;
pub type TextChannelId = Id<TextChannel>;
pub type VoiceChannelId = Id<VoiceChannel>;

pub type GroupId = Id<Group>;

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
