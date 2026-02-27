use gpui::{App, Window};

pub mod animation;
pub mod chat_state;
pub mod collapsable_card;
pub mod context_popover;
pub mod left_sidebar;
pub mod streaming_state;

pub type EventCallback<T> = Box<dyn Fn(&T, &mut Window, &mut App)>;
