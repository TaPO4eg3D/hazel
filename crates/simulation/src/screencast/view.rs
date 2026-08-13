use std::path::PathBuf;

use gpui::{App, DMABuffer, ParentElement, Render, Styled, Window, div, surface};
use gpui_component::{StyledExt, cyan};

pub struct ScreenCastView {
    file: DMABuffer,
}

impl ScreenCastView {
    pub fn new(file: DMABuffer, window: &mut Window, cx: &mut App) -> Self {
        Self { file }
    }
}

impl Render for ScreenCastView {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::prelude::Context<Self>,
    ) -> impl gpui::prelude::IntoElement {
        div()
            .flex()
            .size_full()
            .child(surface(self.file.clone()).size_full())
            .child(div().size_full().bg(cyan(100)))
    }
}
