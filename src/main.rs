use gpui::{
    App, AppContext, Application, Bounds, Context, Entity, InteractiveElement, MouseUpEvent, ParentElement, Render, Styled, Window, WindowBounds, WindowOptions, blue, div, px, red, size, white, yellow
};

struct MainWindow {
    counters: Vec<Entity<Counter>>
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        div()
            .bg(blue())
            .size_full()
            .flex()
            .gap_x_2()
            .children(self.counters.clone())
    }
}

struct Counter {
    count: usize,
}

impl Counter {
    fn on_mouse_click(&mut self, _: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.count += 1;

        cx.notify();
    }
}

impl Render for Counter {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> impl gpui::IntoElement {
        div()
            .bg(yellow())
            .text_color(blue())
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(Self::on_mouse_click))
            .child(format!("{} times pressed!", self.count))
    }
}



fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(500.), px(500.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                let counters = (0..4).map(|i| cx.new(|_| Counter {
                    count: i
                })).collect();

                cx.new(|_| MainWindow { counters })
            },
        )
        .unwrap();
    });
}
