use gpui::{AppContext, Context, Entity, Render, SharedString, Window, div};
use gpui_component::input::InputState;

#[derive(Clone, Debug)]
pub struct TextChannel {
    pub id: u64,
    pub name: SharedString,

    pub is_active: bool,
    pub is_muted: bool,

    pub unread_messages: usize,
}

pub struct ChatState {
    _input_state: Entity<InputState>,

    pub text_channels: Vec<TextChannel>,
}

impl ChatState {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input_state = cx.new(|cx| {
            InputState::new(window, cx)
        });

        Self {
            _input_state: input_state,
            text_channels: (0..4).map(|i| {
                TextChannel {
                    id: i,
                    name: format!("Text channel {i}").into(),
                    is_active: i == 0,
                    is_muted: false,
                    unread_messages: i as usize,
                }
            }).collect(),
        }
    }
}

impl Render for ChatState {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        div()
    }
}
