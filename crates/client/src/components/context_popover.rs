use std::{cell::RefCell, rc::Rc};

use gpui::{
    AnyElement, App, Corner, Div, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior,
    InspectorElementId, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, Pixels, Point, RenderOnce, StyleRefinement, Styled, Window, anchored, deferred,
    div, px,
};
use gpui_component::StyledExt;

/// A extension trait for adding a context menu to an element.
pub trait ContextPopover: ParentElement + Styled {
    /// Add a context menu to the element.
    ///
    /// This will changed the element to be `relative` positioned, and add a child `ContextMenu` element.
    /// Because the `ContextMenu` element is positioned `absolute`, it will not affect the layout of the parent element.
    fn context_menu(
        self,
        id: impl Into<ElementId>,
        f: impl Fn(ContextMenuItem, &mut Window, &mut App) -> ContextMenuItem + 'static,
    ) -> ContextMenu<Self>
    where
        Self: Sized,
    {
        ContextMenu::new(id.into(), self).content(f)
    }
}

#[derive(IntoElement)]
pub struct ContextMenuItem {
    base: Div,
    style: StyleRefinement,
    children: Vec<AnyElement>,
}

impl ContextMenuItem {
    fn new() -> Self {
        Self {
            base: div(),
            style: StyleRefinement::default(),
            children: vec![],
        }
    }
}

impl ParentElement for ContextMenuItem {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for ContextMenuItem {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl InteractiveElement for ContextMenuItem {
    fn interactivity(&mut self) -> &mut gpui::Interactivity {
        self.base.interactivity()
    }
}

impl RenderOnce for ContextMenuItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.base
            .popover_style(cx)
            .occlude()
            .children(self.children)
            .refine_style(&self.style)
    }
}

impl<E: ParentElement + Styled> ContextPopover for E {}

/// A context menu that can be shown on right-click.
pub struct ContextMenu<E: ParentElement + Styled + Sized> {
    id: ElementId,
    element: Option<E>,
    on_toggle: Option<Rc<dyn Fn(&bool, &mut Window, &mut App)>>,
    content: Option<Rc<dyn Fn(ContextMenuItem, &mut Window, &mut App) -> ContextMenuItem>>,
    // This is not in use, just for style refinement forwarding.
    _ignore_style: StyleRefinement,
    anchor: Corner,
}

impl<E: ParentElement + Styled> ContextMenu<E> {
    /// Create a new context menu with the given ID.
    pub fn new(id: impl Into<ElementId>, element: E) -> Self {
        Self {
            id: id.into(),
            element: Some(element),
            on_toggle: None,
            content: None,
            anchor: Corner::TopLeft,
            _ignore_style: StyleRefinement::default(),
        }
    }

    pub fn on_toggle(mut self, on_toggle: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(on_toggle));
        self
    }

    /// Build the context popover using the given builder function.
    #[must_use]
    fn content<F>(mut self, builder: F) -> Self
    where
        F: Fn(ContextMenuItem, &mut Window, &mut App) -> ContextMenuItem + 'static,
    {
        self.content = Some(Rc::new(builder));
        self
    }

    fn with_element_state<R>(
        &mut self,
        id: &GlobalElementId,
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(&mut Self, &mut ContextMenuState, &mut Window, &mut App) -> R,
    ) -> R {
        window.with_optional_element_state::<ContextMenuState, _>(
            Some(id),
            |element_state, window| {
                let mut element_state = element_state.unwrap().unwrap_or_default();
                let result = f(self, &mut element_state, window, cx);
                (result, Some(element_state))
            },
        )
    }
}

impl<E: ParentElement + Styled> ParentElement for ContextMenu<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        if let Some(element) = &mut self.element {
            element.extend(elements);
        }
    }
}

impl<E: ParentElement + Styled> Styled for ContextMenu<E> {
    fn style(&mut self) -> &mut StyleRefinement {
        if let Some(element) = &mut self.element {
            element.style()
        } else {
            &mut self._ignore_style
        }
    }
}

impl<E: ParentElement + Styled + IntoElement + 'static> IntoElement for ContextMenu<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

struct ContextMenuSharedState {
    open: bool,
    position: Point<Pixels>,
}

pub struct ContextMenuState {
    element: Option<AnyElement>,
    shared_state: Rc<RefCell<ContextMenuSharedState>>,
}

impl Default for ContextMenuState {
    fn default() -> Self {
        Self {
            element: None,
            shared_state: Rc::new(RefCell::new(ContextMenuSharedState {
                open: false,
                position: Default::default(),
            })),
        }
    }
}

impl<E: ParentElement + Styled + IntoElement + 'static> Element for ContextMenu<E> {
    type RequestLayoutState = ContextMenuState;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let anchor = self.anchor;

        self.with_element_state(
            id.unwrap(),
            window,
            cx,
            |this, state: &mut ContextMenuState, window, cx| {
                let (position, open) = {
                    let shared_state = state.shared_state.borrow();
                    (shared_state.position, shared_state.open)
                };
                let mut content_element = None;

                if open {
                    let content = if let Some(builder) = this.content.as_ref() {
                        builder(ContextMenuItem::new(), window, cx)
                    } else {
                        ContextMenuItem::new()
                    }
                    .on_mouse_down_out({
                        let on_toggle = this.on_toggle.clone();
                        let shared_state = state.shared_state.clone();

                        move |_, window, cx| {
                            shared_state.borrow_mut().open = false;

                            if let Some(on_toggle) = on_toggle.clone() {
                                on_toggle(&false, window, cx);
                            };

                            window.refresh();
                        }
                    });

                    let position = Point {
                        x: position.x - px(10.),
                        y: position.y - px(10.),
                    };

                    content_element = Some(
                        deferred(
                            anchored().child(
                                div()
                                    .w(window.bounds().size.width)
                                    .h(window.bounds().size.height)
                                    .on_scroll_wheel(|_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .child(
                                        anchored()
                                            .position(position)
                                            .snap_to_window_with_margin(px(8.))
                                            .anchor(anchor)
                                            .child(content),
                                    ),
                            ),
                        )
                        .with_priority(1)
                        .into_any(),
                    );
                }

                let mut element = this
                    .element
                    .take()
                    .expect("Element should exists.")
                    .children(content_element)
                    .into_any_element();

                let layout_id = element.request_layout(window, cx);

                (
                    layout_id,
                    ContextMenuState {
                        element: Some(element),
                        ..Default::default()
                    },
                )
            },
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        if let Some(element) = &mut request_layout.element {
            element.prepaint(window, cx);
        }

        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(element) = &mut request_layout.element {
            element.paint(window, cx);
        }

        self.with_element_state(
            id.unwrap(),
            window,
            cx,
            |this, state: &mut ContextMenuState, window, _| {
                let shared_state = state.shared_state.clone();

                let hitbox = hitbox.clone();
                let on_toggle = this.on_toggle.clone();

                // When right mouse click, to build content menu, and show it at the mouse position.
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase.bubble()
                        && event.button == MouseButton::Right
                        && hitbox.is_hovered(window)
                    {
                        let mut shared_state = shared_state.borrow_mut();

                        shared_state.position = event.position;
                        shared_state.open = true;

                        if let Some(on_toggle) = on_toggle.clone() {
                            on_toggle(&true, window, cx);
                        };

                        window.refresh();
                    }
                });
            },
        );
    }
}
