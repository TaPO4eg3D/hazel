use gpui::{Context, Div, IntoElement, ParentElement, Render, Window, div};


pub struct Button {
    label: Option<String>,
}

impl IntoElement for Button {
    type Element = Div;

    fn into_element(self) -> Self::Element {
        let mut root = div();

        if let Some(label) = self.label {
            root = root
                .child(label.clone())
        }

        root
    }
}

impl Button {
    pub fn label(mut self, label: &str) -> Self {
        self.label = Some(label.into());

        self
    }
}

pub fn button() -> Button  {
    Button {
        label: None,
    }
}
