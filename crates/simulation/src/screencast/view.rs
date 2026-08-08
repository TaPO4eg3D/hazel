use std::path::PathBuf;

use gpui::{App, Render, Window, div};

pub struct ScreenCastView {
    file: PathBuf,
}

impl ScreenCastView {
    pub fn new(file: PathBuf, window: &mut Window, cx: &mut App) -> Self {
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
    }
}
