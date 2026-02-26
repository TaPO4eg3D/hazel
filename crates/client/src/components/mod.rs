use gpui::{App, Window};

pub mod chat_state;
pub mod left_sidebar;
pub mod streaming_state;
pub mod animation;
pub mod collapsable_card;

pub type EventCallback<T> = Box<dyn Fn(&T, &mut Window, &mut App)>;
