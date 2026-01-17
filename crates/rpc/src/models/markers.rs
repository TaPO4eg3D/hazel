use std::marker::PhantomData;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub struct Id<T> {
    pub value: i32,
    pub _marker: PhantomData<fn() -> T>,
}

impl<T> Id<T> {
    pub fn new(v: i32) -> Self {
        Self {
            value: v,
            _marker: PhantomData,
        }
    }
}

pub trait TaggedEntity<T> {
    fn tagged_id(&self) -> Id<T>;
}

#[macro_export]
macro_rules! tag_entity {
    ($model:ident, $tag:ty) => {
        impl rpc::models::markers::TaggedEntity<$tag> for Model {
            fn tagged_id(&self) -> rpc::models::markers::Id<$tag> {
                rpc::models::markers::Id::new(self.id)
            }
        }
    };
}

#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub struct User;
pub type UserId = Id<User>;

#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub struct Media;
pub type MediaId = Id<Media>;

#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub struct Message;
pub type MsgId = Id<Message>;

#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub struct VoiceChannel;
pub type VoiceChannelId = Id<VoiceChannel>;

#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub struct TextChannel;
pub type TextChannelId = Id<TextChannel>;

#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub struct Group;
pub type GroupId = Id<Group>;
