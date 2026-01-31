use std::{cell::RefCell, rc::Rc, time::Instant};

use gpui::{
    Animation, AnyElement, App, Bounds, Element, ElementId, GlobalElementId, Hitbox,
    HitboxBehavior, InspectorElementId, IntoElement, MouseMoveEvent, Pixels, Window,
};

pub trait HoverAnimationExt {
    /// Render this component or element with a hover-triggered animation.
    ///
    /// On hover enter, the animation runs from 0.0 to 1.0 over the given duration.
    /// On hover exit, the animation runs from the current value back to 0.0.
    ///
    /// The animator callback receives the element and a delta value between 0.0 and 1.0
    /// representing the current animation progress.
    ///
    /// # Example
    ///
    /// ```
    /// div()
    ///     .id("animated-div")
    ///     .size_20()
    ///     .bg(gpui::blue())
    ///     .with_hover_animation(
    ///         "hover-fade",
    ///         Animation::new(Duration::from_millis(200)).with_easing(ease_in_out),
    ///         |div, delta| div.bg(gpui::blue().opacity(0.5 + delta * 0.5)),
    ///     )
    /// ```
    fn with_hover_animation(
        self,
        id: impl Into<ElementId>,
        animation: Animation,
        animator: impl Fn(Self, f32) -> Self + 'static,
    ) -> HoverAnimationElement<Self>
    where
        Self: Sized,
    {
        HoverAnimationElement {
            id: id.into(),
            element: Some(self),
            animation,
            animator: Box::new(animator),
        }
    }
}

impl<E: IntoElement + 'static> HoverAnimationExt for E {}

#[derive(Clone)]
struct HoverAnimationState {
    is_hovered: bool,
    progress: f32,
    animation_start: Instant,
    animating_in: bool,
}

impl Default for HoverAnimationState {
    fn default() -> Self {
        Self {
            is_hovered: false,
            progress: 0.,
            animation_start: Instant::now(),
            animating_in: false,
        }
    }
}

/// A GPUI element that applies a hover-triggered animation to another element.
///
/// On hover enter, the animation runs from 0.0 to 1.0.
/// On hover exit, the animation runs from the current value back to 0.0.
pub struct HoverAnimationElement<E> {
    id: ElementId,
    element: Option<E>,
    animation: Animation,
    animator: Box<dyn Fn(E, f32) -> E + 'static>,
}

impl<E> HoverAnimationElement<E> {
    /// Returns a new [`HoverAnimationElement<E>`] after applying the given function
    /// to the element being animated.
    pub fn map_element(mut self, f: impl FnOnce(E) -> E) -> HoverAnimationElement<E> {
        self.element = self.element.map(f);
        self
    }
}

impl<E: IntoElement + 'static> IntoElement for HoverAnimationElement<E> {
    type Element = HoverAnimationElement<E>;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: IntoElement + 'static> Element for HoverAnimationElement<E> {
    type RequestLayoutState = (AnyElement, f32);
    type PrepaintState = (Hitbox, f32);

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (crate::LayoutId, Self::RequestLayoutState) {
        window.with_element_state(
            global_id.unwrap(),
            |state: Option<Rc<RefCell<HoverAnimationState>>>, window| {
                let state = state.unwrap_or_default();

                let (layout_id, (element, delta)) = {
                    let state = state.borrow_mut();

                    let animation_duration = self.animation.duration.as_secs_f32();
                    let elapsed = state.animation_start.elapsed().as_secs_f32();
                    let raw_delta = elapsed / animation_duration;

                    let delta = if state.animating_in {
                        raw_delta.min(1.0)
                    } else {
                        let start_progress = if state.progress > 0.0 {
                            state.progress
                        } else {
                            0.0
                        };
                        let remaining = 1.0 - raw_delta.min(1.0);
                        (start_progress * remaining).max(0.0)
                    };

                    let eased_delta = (self.animation.easing)(delta);

                    let animation_complete =
                        (state.is_hovered && delta >= 1.0) || (!state.is_hovered && delta <= 0.0);

                    if !animation_complete {
                        window.request_animation_frame();
                    }

                    let element = self.element.take().expect("should only be called once");
                    let animated_element = (self.animator)(element, eased_delta);
                    let mut element = animated_element.into_any_element();

                    let layout_id = element.request_layout(window, cx);

                    (layout_id, (element, delta))
                };

                ((layout_id, (element, delta)), state)
            },
        )
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let (inner_element, delta) = element;
        inner_element.prepaint(window, cx);

        (window.insert_hitbox(bounds, HitboxBehavior::Normal), *delta)
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        (hitbox, delta): &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let hitbox = hitbox.clone();
        let delta = *delta;

        window.with_element_state(
            global_id.unwrap(),
            |state: Option<Rc<RefCell<HoverAnimationState>>>, window| {
                let state = state.unwrap_or_default();
                let current_view = window.current_view();

                window.on_mouse_event({
                    let state = state.clone();

                    move |_: &MouseMoveEvent, _phase, window, cx| {
                        let mut state = state.borrow_mut();
                        let is_hovered = hitbox.is_hovered(window);

                        let was_hovered = state.is_hovered;
                        let hover_changed = is_hovered != was_hovered;

                        if hover_changed {
                            state.animation_start = Instant::now();
                            state.animating_in = is_hovered;
                            state.progress = delta;

                            cx.notify(current_view);
                        }

                        state.is_hovered = is_hovered;
                    }
                });

                ((), state)
            },
        );
        let (inner_element, _delta) = element;
        inner_element.paint(window, cx);
    }
}
