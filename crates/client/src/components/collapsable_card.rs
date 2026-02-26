use gpui::{
    AnyElement, Context, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, SharedString, Styled as _, div, prelude::FluentBuilder,
};
use gpui_component::{
    StyledExt as _,
    button::{Button, ButtonVariants as _},
    label::Label,
};

use crate::assets::IconName;

pub struct CollapsableCardState {
    is_collapsed: bool,
}

impl CollapsableCardState {
    pub fn new() -> Self {
        Self {
            is_collapsed: false,
        }
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.is_collapsed = !self.is_collapsed;
    }
}

impl Default for CollapsableCardState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(IntoElement)]
pub struct CollapsableCard {
    id: ElementId,

    state: Entity<CollapsableCardState>,
    content: Option<AnyElement>,

    title: Option<SharedString>,
}

impl CollapsableCard {
    pub fn new(id: impl Into<ElementId>, state: Entity<CollapsableCardState>) -> Self {
        Self {
            id: id.into(),
            state,
            content: None,
            title: None,
        }
    }

    pub fn title(mut self, value: impl Into<SharedString>) -> Self {
        self.title = Some(value.into());
        self
    }

    pub fn content(mut self, value: impl IntoElement) -> Self {
        self.content = Some(value.into_any_element());
        self
    }
}

impl RenderOnce for CollapsableCard {
    fn render(self, _window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        let is_collapsed = self.state.read(cx).is_collapsed;

        div()
            .id(self.id)
            .p_3()
            .w_full()
            .v_flex()
            .child(
                div()
                    .mb_2()
                    .w_full()
                    .flex()
                    .items_center()
                    .child(
                        Label::new(self.title.unwrap_or("".into()))
                            .text_sm()
                            .font_semibold(),
                    )
                    .child(
                        Button::new("collapse-toggle")
                            .ml_auto()
                            .cursor_pointer()
                            .icon({
                                if is_collapsed {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronDown
                                }
                            })
                            .ghost()
                            .on_click(move |_, _window, cx| {
                                self.state.update(cx, |this, cx| {
                                    this.toggle(cx);
                                });
                            }),
                    ),
            )
            .when_some(self.content, |this, content| {
                this.when(!is_collapsed, |this| this.child(content))
            })
    }
}
