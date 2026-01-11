use gpui::{App, AppContext, Context, Entity, Render, Window, div};
use gpui_component::input::InputState;

pub struct Chat {
    input_state: Entity<InputState>,
}

impl Chat {
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

impl Render for Chat {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        div()
    }
}
