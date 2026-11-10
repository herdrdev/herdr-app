use std::{cell::Cell, rc::Rc};

use gpui::{
    Anchor, AnyElement, App, Bounds, Div, ElementId, InteractiveElement, Interactivity,
    IntoElement, ParentElement, Pixels, Point, RenderOnce, StatefulInteractiveElement,
    StyleRefinement, Styled, Window, canvas, deferred, div, px,
};

use crate::{Align, Placement, Positioner, ResolvedPosition, StyledExt as _};

/// Distance kept between a popup and the window edge.
const WINDOW_MARGIN: Pixels = px(8.);

/// Deferred paint priority for interactive surfaces that must appear above dialogs.
pub const POPUP_PRIORITY: usize = 100;

#[derive(Default)]
struct PopupAnchorState {
    bounds: Bounds<Pixels>,
    captured: bool,
}

/// An unstyled trigger and anchored popup host.
///
/// `Popup` owns trigger measurement, anchor-point calculation, first-frame
/// synchronization, deferred rendering, and window-edge snapping. Callers own
/// open state, interaction, popup content, appearance, and motion.
#[derive(IntoElement)]
pub struct Popup {
    id: ElementId,
    base: gpui::Stateful<Div>,
    style: StyleRefinement,
    anchor: Anchor,
    offset: Pixels,
    on_position: Option<Box<dyn Fn(ResolvedPosition, Bounds<Pixels>)>>,
    trigger: AnyElement,
    content: Option<AnyElement>,
}

impl Popup {
    pub fn new(id: impl Into<ElementId>, trigger: impl IntoElement) -> Self {
        let id = id.into();
        Self {
            base: div().id(id.clone()),
            id,
            style: StyleRefinement::default(),
            anchor: Anchor::TopLeft,
            offset: px(0.),
            on_position: None,
            trigger: trigger.into_any_element(),
            content: None,
        }
    }

    pub fn anchor(mut self, anchor: impl Into<Anchor>) -> Self {
        self.anchor = anchor.into();
        self
    }

    /// Gap from the trigger along the anchor's outward direction, zero by default.
    pub fn offset(mut self, offset: Pixels) -> Self {
        self.offset = offset;
        self
    }

    /// Observe resolved popup and trigger bounds before content prepaint.
    pub fn on_position(
        mut self,
        callback: impl Fn(ResolvedPosition, Bounds<Pixels>) + 'static,
    ) -> Self {
        self.on_position = Some(Box::new(callback));
        self
    }

    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }

    pub fn resolved_corner(anchor: Anchor, trigger_bounds: Bounds<Pixels>) -> Point<Pixels> {
        match anchor {
            Anchor::TopLeft => trigger_bounds.origin,
            Anchor::TopCenter => trigger_bounds.top_center(),
            Anchor::TopRight => trigger_bounds.top_right(),
            Anchor::BottomLeft => Point {
                x: trigger_bounds.origin.x,
                y: trigger_bounds.origin.y - trigger_bounds.size.height,
            },
            Anchor::BottomCenter => Point {
                x: trigger_bounds.top_center().x,
                y: trigger_bounds.origin.y - trigger_bounds.size.height,
            },
            Anchor::BottomRight => Point {
                x: trigger_bounds.top_right().x,
                y: trigger_bounds.origin.y - trigger_bounds.size.height,
            },
            Anchor::LeftCenter | Anchor::RightCenter => trigger_bounds.origin,
        }
    }
}

/// The side of the trigger an anchor puts the popup on, and how it lines up
/// along that side: the popup's top-left corner on the trigger's bottom-left
/// is "below, start-aligned". Placing by side rather than by corner is what
/// lets the popup flip to the other side when it would not fit.
fn anchor_side(anchor: Anchor) -> (Placement, Align) {
    match anchor {
        Anchor::TopLeft => (Placement::Bottom, Align::Start),
        Anchor::TopCenter => (Placement::Bottom, Align::Center),
        Anchor::TopRight => (Placement::Bottom, Align::End),
        Anchor::BottomLeft => (Placement::Top, Align::Start),
        Anchor::BottomCenter => (Placement::Top, Align::Center),
        Anchor::BottomRight => (Placement::Top, Align::End),
        Anchor::LeftCenter => (Placement::Right, Align::Center),
        Anchor::RightCenter => (Placement::Left, Align::Center),
    }
}

impl Styled for Popup {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl InteractiveElement for Popup {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for Popup {}

impl RenderOnce for Popup {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state =
            window.use_keyed_state((self.id, "anchor"), cx, |_, _| PopupAnchorState::default());
        let trigger_bounds = Rc::new(Cell::new(state.read(cx).bounds));
        let anchor_visible = Rc::new(Cell::new(true));

        let root = self
            .base
            .child(self.trigger)
            .child(
                canvas(
                    {
                        let state = state.clone();
                        let trigger_bounds = trigger_bounds.clone();
                        let anchor_visible = anchor_visible.clone();
                        move |bounds, window, cx| {
                            trigger_bounds.set(bounds);
                            // A trigger scrolled out of its clipping parent
                            // takes the popup with it.
                            anchor_visible.set(window.content_mask().bounds.intersects(&bounds));
                            let first = state.update(cx, |state, _| {
                                let first = !state.captured;
                                state.bounds = bounds;
                                state.captured = true;
                                first
                            });
                            if first {
                                window.request_animation_frame();
                            }
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
                .top_0()
                .left_0(),
            )
            .refine_style(&self.style);

        let Some(content) = self.content else {
            return root;
        };
        if !state.read(cx).captured {
            return root;
        }

        let (placement, align) = anchor_side(self.anchor);
        let positioner = Positioner::side(trigger_bounds.get())
            .placement(placement)
            .align(align)
            .offset(self.offset)
            .tracked_trigger_bounds(trigger_bounds.clone())
            .tracked_anchor_visible(anchor_visible);
        let positioner = if let Some(callback) = self.on_position {
            positioner.on_position(move |position| callback(position, trigger_bounds.get()))
        } else {
            positioner
        };
        root.child(
            deferred(
                positioner
                    .margin(WINDOW_MARGIN)
                    // The host blocks the mouse, so no caller has to remember:
                    // what a popup covers belongs to the popup.
                    .occlude()
                    .child(content),
            )
            .with_priority(POPUP_PRIORITY),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, px};

    #[test]
    fn resolved_corner_preserves_existing_anchor_math() {
        let bounds = Bounds {
            origin: Point::new(px(100.), px(100.)),
            size: gpui::Size::new(px(200.), px(50.)),
        };
        assert_eq!(
            Popup::resolved_corner(Anchor::TopCenter, bounds),
            Point::new(px(200.), px(100.))
        );
        assert_eq!(
            Popup::resolved_corner(Anchor::BottomRight, bounds),
            Point::new(px(300.), px(50.))
        );
    }

    struct Harness;

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            Popup::new(
                "popup",
                div()
                    .debug_selector(|| "popup-trigger".into())
                    .size(px(100.)),
            )
            .content(
                div()
                    .debug_selector(|| "popup-content".into())
                    .size(px(20.)),
            )
        }
    }

    /// A caller that styles its own surface — a hover card, a dropdown — does
    /// not have to remember to block the mouse. The host does it, so the panel
    /// a popup covers stops reacting to a pointer that is over the popup.
    struct OcclusionHarness {
        background_hovered: Rc<Cell<bool>>,
        content_hovered: Rc<Cell<bool>>,
    }

    impl Render for OcclusionHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let background = self.background_hovered.clone();
            let content = self.content_hovered.clone();
            div()
                .relative()
                .size(px(200.))
                .child(
                    div()
                        .id("background")
                        .absolute()
                        .size_full()
                        .on_mouse_move(move |_, _, _| background.set(true)),
                )
                .child(
                    Popup::new("popup", div().size(px(100.))).content(
                        div()
                            .id("content")
                            .size(px(40.))
                            .on_mouse_move(move |_, _, _| content.set(true)),
                    ),
                )
        }
    }

    #[gpui::test]
    fn the_popup_surface_blocks_the_panel_it_covers(cx: &mut gpui::TestAppContext) {
        let background_hovered = Rc::new(Cell::new(false));
        let content_hovered = Rc::new(Cell::new(false));
        let (_, window) = cx.add_window_view({
            let background_hovered = background_hovered.clone();
            let content_hovered = content_hovered.clone();
            move |_, _| OcclusionHarness {
                background_hovered,
                content_hovered,
            }
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        window.update(|window, cx| window.draw(cx).clear(cx));

        window.simulate_mouse_move(
            gpui::point(px(20.), px(110.)),
            None,
            gpui::Modifiers::default(),
        );
        assert!(!background_hovered.get());
        // The surface blocks what is behind it, not its own content: the
        // hitbox goes in ahead of the children, never over them.
        assert!(content_hovered.get());

        // The same pointer outside the surface still reaches the panel, so the
        // assertion above is about occlusion and not a dead listener.
        window.simulate_mouse_move(
            gpui::point(px(150.), px(180.)),
            None,
            gpui::Modifiers::default(),
        );
        assert!(background_hovered.get());
    }

    #[gpui::test]
    fn trigger_capture_enables_deferred_content_on_the_next_frame(cx: &mut gpui::TestAppContext) {
        let (_, window) = cx.add_window_view(|_, _| Harness);
        window.update(|window, cx| window.draw(cx).clear(cx));
        window.update(|window, cx| window.draw(cx).clear(cx));

        assert_eq!(
            window.debug_bounds("popup-trigger").unwrap().size,
            gpui::Size::new(px(100.), px(100.))
        );
        assert_eq!(
            window.debug_bounds("popup-content").unwrap().size,
            gpui::Size::new(px(20.), px(20.))
        );
    }

    /// A trigger at `top` inside a `clip`-tall clipping parent, with a popup
    /// asked to open below it.
    struct PlacedHarness {
        top: Pixels,
        clip: Pixels,
    }

    impl Render for PlacedHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                div().h(self.clip).overflow_hidden().child(
                    div().absolute().top(self.top).left(px(10.)).child(
                        Popup::new(
                            "popup",
                            div()
                                .debug_selector(|| "popup-trigger".into())
                                .w(px(100.))
                                .h(px(20.)),
                        )
                        .content(
                            div()
                                .debug_selector(|| "popup-content".into())
                                .w(px(100.))
                                .h(px(60.)),
                        ),
                    ),
                ),
            )
        }
    }

    fn placed(
        cx: &mut gpui::TestAppContext,
        top: impl Fn(Pixels) -> Pixels,
        clip: impl Fn(Pixels) -> Pixels,
    ) -> (Option<Bounds<Pixels>>, Option<Bounds<Pixels>>) {
        let (view, window) = cx.add_window_view(|_, _| PlacedHarness { top: px(0.), clip: px(0.) });
        let height = window.update(|window, _| window.viewport_size().height);
        view.update(window, |view, _| {
            view.top = top(height);
            view.clip = clip(height);
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        window.update(|window, cx| window.draw(cx).clear(cx));
        (window.debug_bounds("popup-trigger"), window.debug_bounds("popup-content"))
    }

    #[gpui::test]
    fn a_popup_without_room_below_opens_above_its_trigger(cx: &mut gpui::TestAppContext) {
        let (trigger, content) = placed(cx, |height| height - px(40.), |height| height);
        let (trigger, content) = (trigger.unwrap(), content.unwrap());
        // Clamped below, it would cover the trigger it belongs to.
        assert_eq!(content.bottom(), trigger.top());
        assert_eq!(content.left(), trigger.left());
    }

    #[gpui::test]
    fn a_popup_with_room_below_stays_below_its_trigger(cx: &mut gpui::TestAppContext) {
        let (trigger, content) = placed(cx, |_| px(40.), |height| height);
        assert_eq!(content.unwrap().top(), trigger.unwrap().bottom());
    }

    #[gpui::test]
    fn a_popup_leaves_with_its_trigger_when_it_is_scrolled_out_of_sight(
        cx: &mut gpui::TestAppContext,
    ) {
        // The trigger sits past the bottom of its clipping parent, the way a
        // row of a scrolled page does; the popup must not stay pinned on.
        let (_, content) = placed(cx, |_| px(200.), |_| px(100.));
        assert_eq!(content, None);
    }
}
