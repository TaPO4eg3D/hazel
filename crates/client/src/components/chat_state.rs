use gpui::{App, AppContext, Context, Entity, Render, SharedString, Window, div};
use gpui_component::input::InputState;

#[derive(Clone)]
pub struct TextChannel {
    pub id: u64,
    pub name: SharedString,

    pub is_active: bool,
    pub is_muted: bool,

    pub has_unread: bool,
}

pub struct ChatState {
    input_state: Entity<InputState>,
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
            input_state,
        }
    }
}

impl Render for ChatState {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        div()
    }
}
