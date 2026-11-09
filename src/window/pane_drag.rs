//! App-owned pane grips and drag gestures, independent of header mods.
use super::*;
use crate::layout::Edge;
use gpui_base::animation::ease_out_cubic;
use gpui_base::motion::{Spring, Transition, spring, transition};

/// A pane drag, from the press on its grip to the end of its landing.
pub(super) enum PaneDrag {
    /// Pressed on the grip; carried once the pointer moves `DRAG_START`.
    Pressed { pane: String, tab: String, down: Point<Pixels> },
    /// Follows the pointer at its own size; the tab is laid out as letting go
    /// here would leave it, with a dashed box where it lands.
    Carried {
        pane: String,
        tab: String,
        down: Point<Pixels>,
        pointer: Point<Pixels>,
        /// Its place when lifted.
        from: Bounds<Pixels>,
        /// The tab's panes when lifted: where the pointer drops it (the
        /// preview moving under the pointer doesn't change the answer).
        zones: Vec<(String, Bounds<Pixels>)>,
        target: Option<(String, Option<Edge>)>,
    },
    /// Let go or sent back: it glides into its place, the others slide to theirs.
    Landing { pane: String, tab: String },
}

impl PaneDrag {
    fn pane(&self) -> &str {
        match self {
            Self::Pressed { pane, .. } | Self::Carried { pane, .. } | Self::Landing { pane, .. } => pane,
        }
    }

    fn tab(&self) -> &str {
        match self {
            Self::Pressed { tab, .. } | Self::Carried { tab, .. } | Self::Landing { tab, .. } => tab,
        }
    }
}

/// Where a drag draws the tab's panes this frame (it eases them toward where
/// the layout puts them).
pub(super) struct PaneShown {
    rects: HashMap<String, Bounds<Pixels>>,
    /// The dashed box where the carried pane lands.
    slot: Bounds<Pixels>,
    /// How lifted the carried pane is: its shadow and dashed box.
    lift: f32,
    /// Its grip, which fades as it lands.
    grip: f32,
    /// The preview drawn, to see the next frame's layout once it changes.
    target: Option<(String, Option<Edge>)>,
}

/// The drag motion of the shipped tabs and spaces mods (their `SLIDE`,
/// `GLIDE` and `RAISE`): the others slide, the carried one glides into
/// place, its shadow rises.
const SLIDE: Spring = Spring::new(Duration::from_millis(200)).with_damping(1.0);
const GLIDE: Duration = Duration::from_millis(200);
const RAISE: Duration = Duration::from_millis(120);
/// How far the pointer moves before a press carries the pane.
const DRAG_START: f32 = 4.;

fn ease(duration: Duration) -> Transition {
    Transition::new(duration).ease(ease_out_cubic)
}

fn spring_bounds(key: &str, b: Bounds<Pixels>, window: &mut Window, cx: &mut App) -> Bounds<Pixels> {
    let id = SharedString::from(key.to_owned());
    let mut at = |channel: &'static str, v: Pixels| spring((id.clone(), channel), v, SLIDE, window, cx);
    let origin = point(at("x", b.origin.x), at("y", b.origin.y));
    Bounds::new(origin, size(at("w", b.size.width), at("h", b.size.height)))
}

/// Which pane, and which part of it, the pointer drops `pane` on.
fn pane_target(zones: &[(String, Bounds<Pixels>)], pane: &str, at: Point<Pixels>) -> Option<(String, Option<Edge>)> {
    zones.iter().filter(|(id, _)| id != pane).find_map(|(id, b)| Some((id.clone(), drop_zone(*b, at)?)))
}

/// The outer quarter of a pane chooses its nearest edge; the middle swaps.
fn drop_zone(bounds: Bounds<Pixels>, at: Point<Pixels>) -> Option<Option<Edge>> {
    if !bounds.contains(&at) || bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
        return None;
    }
    let x = f32::from(at.x - bounds.origin.x) / f32::from(bounds.size.width);
    let y = f32::from(at.y - bounds.origin.y) / f32::from(bounds.size.height);
    let (distance, edge) = [(x, Edge::Left), (1. - x, Edge::Right), (y, Edge::Top), (1. - y, Edge::Bottom)]
        .into_iter().min_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
    Some((distance < 0.25).then_some(edge))
}

/// The top row of a pane without a top header.
const STRIP: f32 = 18.;

fn top_strip(mut bounds: Bounds<Pixels>, row: f32, at: Point<Pixels>) -> bool {
    bounds.size.height = bounds.size.height.min(settings::z(row));
    bounds.contains(&at)
}

fn inherited_movable(root: &Node, id: &str) -> Option<bool> {
    root.path_to(id)?.iter().rev().find_map(|id| root.find(id)?.movable)
}

impl AppWindow {
    pub(super) fn pane_movable(&self, pane: &str) -> bool {
        let Some(tab) = self.model.active_tab() else { return false };
        if tab.zoomed.is_some() || tab.root.panes().len() < 2 || !tab.root.find(pane).is_some_and(Node::is_pane) {
            return false;
        }
        let owner = self.chrome.visible_blocks().into_iter().find(|n| matches!(n.content(), Some(Content::Context { .. })));
        inherited_movable(&tab.root, pane)
            .or_else(|| owner.and_then(|n| inherited_movable(&self.chrome, &n.id)))
            .unwrap_or(true)
    }

    pub(super) fn pane_grip(&self, pane: &str, header: Option<f32>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let row = header.unwrap_or(STRIP).max(STRIP);
        self.pane_grip_rows.borrow_mut().insert(pane.to_owned(), row);
        if !self.pane_movable(pane) { return None; }
        let dragging = self.pane_drag.as_ref().is_some_and(|d| d.pane() == pane);
        if self.pane_grip_hover.as_deref() != Some(pane) && !dragging { return None; }
        let opacity = self.pane_shown.as_ref().filter(|_| dragging).map_or(1., |s| s.grip);
        let pane = pane.to_owned();
        Some(div().id(SharedString::from(format!("grip-{pane}")))
            .absolute().top(settings::z((row - 18.) / 2.)).left(relative(0.5)).ml(-settings::z(14.))
            .w(settings::z(28.)).h(settings::z(18.)).rounded(settings::z(4.))
            .opacity(opacity)
            .bg(c("background").opacity(0.85))
            .hover(|s| s.bg(c("hover")))
            .flex().flex_col().items_center().justify_center().gap(settings::z(3.)).occlude()
            .group("pane-grip")
            .cursor(if dragging { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                this.pane_press(&pane, ev.position, cx);
                cx.stop_propagation();
            }))
            .children((0..2).map(|_| div().flex().gap(settings::z(3.)).children((0..3).map(|_| div().size(settings::z(2.)).rounded_full()
                .bg(if dragging { c("foreground") } else { c("dim") })
                .group_hover("pane-grip", |s| s.bg(c("foreground")))))))
            .into_any_element())
    }

    fn pane_hover(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        // Nothing hovers while a pane is held or lands, nor after until the pointer moves.
        if self.pane_drag.is_some() {
            return;
        }
        let hovered = self.model.active_tab().and_then(|tab| {
            let bounds = self.bounds.borrow();
            let rows = self.pane_grip_rows.borrow();
            tab.shown().visible_blocks().into_iter().find_map(|n| {
                let b = bounds.get(&n.id)?;
                let row = rows.get(&n.id).copied().unwrap_or(STRIP);
                (self.pane_movable(&n.id) && top_strip(*b, row, at)).then(|| n.id.clone())
            })
        });
        if hovered != self.pane_grip_hover {
            self.pane_grip_hover = hovered;
            cx.notify();
        }
    }

    pub(super) fn pane_press(&mut self, pane: &str, at: Point<Pixels>, cx: &mut Context<Self>) {
        if self.pane_drag.is_none() && self.pane_movable(pane) {
            self.pane_drag = Some(PaneDrag::Pressed {
                pane: pane.to_owned(), tab: self.model.active_tab().unwrap().id.clone(), down: at,
            });
            cx.notify();
        }
    }

    /// Whether a press or carry is under way (a landing pane takes no input).
    pub(super) fn pane_held(&self) -> bool {
        matches!(self.pane_drag, Some(PaneDrag::Pressed { .. } | PaneDrag::Carried { .. }))
    }

    fn pane_move(&mut self, at: Point<Pixels>, pressed: bool, cx: &mut Context<Self>) {
        if !self.pane_held() {
            return;
        }
        if !pressed {
            self.cancel_pane_drag(cx);
            return;
        }
        match self.pane_drag.as_mut() {
            Some(PaneDrag::Pressed { pane, tab, down }) => {
                if (at.x - down.x).abs().max((at.y - down.y).abs()) < settings::z(DRAG_START) {
                    return;
                }
                let bounds = self.bounds.borrow();
                let Some(from) = bounds.get(pane.as_str()).copied() else { return };
                let zones: Vec<_> = self.model.tab(tab).map(|t| t.root.panes().iter()
                    .filter_map(|n| Some((n.id.clone(), *bounds.get(&n.id)?))).collect()).unwrap_or_default();
                let (pane, tab, down) = (std::mem::take(pane), std::mem::take(tab), *down);
                drop(bounds);
                let target = pane_target(&zones, &pane, at);
                self.pane_drag = Some(PaneDrag::Carried { pane, tab, down, pointer: at, from, zones, target });
                self.pane_grip_hover = None;
            }
            Some(PaneDrag::Carried { pane, pointer, zones, target, .. }) => {
                *pointer = at;
                *target = pane_target(zones, pane, at);
            }
            _ => return,
        }
        cx.notify();
    }

    /// Escape, or the button let go elsewhere: a carried pane goes back.
    pub(super) fn cancel_pane_drag(&mut self, cx: &mut Context<Self>) {
        self.pane_drag = match self.pane_drag.take() {
            Some(PaneDrag::Carried { pane, tab, .. }) => Some(PaneDrag::Landing { pane, tab }),
            Some(PaneDrag::Pressed { .. }) | None => None,
            landing => landing,
        };
        cx.notify();
    }

    fn pane_release(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.pane_drag.take() {
            Some(PaneDrag::Pressed { .. }) => {}
            Some(PaneDrag::Carried { pane, tab, target, .. }) => {
                if self.pane_movable(&pane)
                    && let Some((target, edge)) = target
                    && self.model.move_pane(&pane, &target, edge)
                {
                    self.sync(window, cx);
                }
                self.pane_drag = Some(PaneDrag::Landing { pane, tab });
                cx.defer_in(window, |this, window, cx| this.restore_focus(window, cx));
            }
            other => {
                self.pane_drag = other;
                return false;
            }
        }
        cx.notify();
        true
    }

    /// The tab as letting go now would leave it.
    pub(super) fn pane_preview(&self, tab: &Tab) -> Option<Node> {
        match &self.pane_drag {
            Some(PaneDrag::Carried { pane, tab: t, target: Some((target, edge)), .. }) if *t == tab.id => {
                model::moved(&tab.root, pane, target, *edge, "pane-drag".into())
            }
            _ => None,
        }
    }

    /// Eases each pane of a dragged tab toward its place in the layout drawn
    /// last frame; ends a landing once everything is there.
    pub(super) fn pane_motion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = &self.pane_drag else {
            self.pane_shown = None;
            return;
        };
        let Some(tab) = self.model.active_tab().filter(|t| t.id == d.tab() && t.zoomed.is_none()) else {
            // Its tab went away or isn't shown: nothing left to draw it in.
            self.pane_drag = None;
            self.pane_shown = None;
            return;
        };
        if matches!(d, PaneDrag::Pressed { .. }) {
            self.pane_shown = None;
            return;
        }
        // The first frame draws it flat, so the lift has a start to rise from.
        let up = matches!(d, PaneDrag::Carried { .. }) && self.pane_shown.is_some();
        let target = match d {
            PaneDrag::Carried { target, .. } => target.clone(),
            _ => None,
        };
        let mut rects = HashMap::new();
        let mut slot = None;
        let mut settled = true;
        let bounds = self.bounds.borrow().clone();
        for n in tab.root.panes() {
            let Some(place) = bounds.get(&n.id).copied() else { continue };
            let rect = if n.id == d.pane() {
                slot = Some(spring_bounds(&format!("pane-slot:{}", n.id), place, window, cx));
                let id = SharedString::from(format!("pane-carry:{}", n.id));
                match d {
                    PaneDrag::Carried { from, down, pointer, .. } => {
                        transition(id, Bounds::new(from.origin + (*pointer - *down), from.size), ease(Duration::ZERO), window, cx)
                    }
                    _ => transition(id, place, ease(GLIDE), window, cx),
                }
            } else {
                spring_bounds(&format!("pane-at:{}", n.id), place, window, cx)
            };
            settled &= rect == place;
            rects.insert(n.id.clone(), rect);
        }
        let fade = if up { ease(RAISE) } else { ease(GLIDE) };
        let lift = transition("pane-lift", if up { 1. } else { 0. }, fade, window, cx);
        let grip = transition("pane-grip", if up || self.pane_shown.is_none() { 1. } else { 0. }, ease(GLIDE), window, cx);
        let Some(slot) = slot else {
            self.pane_drag = None;
            self.pane_shown = None;
            return;
        };
        if matches!(d, PaneDrag::Landing { .. }) && settled && lift == 0. && grip == 0. {
            self.pane_drag = None;
            self.pane_shown = None;
            return;
        }
        // A new preview is laid out this frame; the next one moves toward it.
        if self.pane_shown.as_ref().is_none_or(|s| s.target != target) {
            window.request_animation_frame();
        }
        self.pane_shown = Some(PaneShown { rects, slot, lift, grip, target });
    }

    /// A pane of a dragged tab: its slot records where the layout puts it,
    /// and the pane is drawn where its motion has it (the carried one by
    /// `pane_drag_layer`, a dashed box in its slot).
    pub(super) fn pane_in_motion(&self, n: &Node, body: impl FnOnce(&mut Context<Self>) -> AnyElement, frame: Option<&PaneFrame>, cx: &mut Context<Self>) -> Option<Div> {
        let s = self.pane_shown.as_ref().filter(|_| n.is_pane())?;
        let rect = *s.rects.get(&n.id)?;
        let d = self.pane_drag.as_ref()?;
        let at = |b: Bounds<Pixels>, child: AnyElement| anchored().position(b.origin).child(div().w(b.size.width).h(b.size.height).child(child));
        let inner = if n.id == d.pane() {
            let mut boxed = div().size_full().border_1().border_dashed().border_color(c("dim")).opacity(s.lift);
            if let Some(r) = frame.and_then(|f| f.radius) {
                boxed = boxed.rounded(settings::z(r));
            }
            at(s.slot, boxed.into_any_element())
        } else {
            at(rect, body(cx))
        };
        Some(div().relative().size_full().child(inner))
    }

    pub(super) fn pane_drag_layer(&self, focused: &HashSet<String>, cx: &mut Context<Self>) -> AnyElement {
        // The carried pane, over everything, with a shadow as it lifts.
        let carried = self.pane_shown.as_ref().zip(self.pane_drag.as_ref()).and_then(|(s, d)| {
            let tab = self.model.tab(d.tab())?;
            let n = tab.root.find(d.pane())?;
            let rect = *s.rects.get(&n.id)?;
            let frame = self.pane_frame();
            let radius = frame.and_then(|f| f.radius).map_or(px(0.), settings::z);
            let body = self.node_body(n, focused, frame, cx).size_full();
            Some(div().absolute().left(rect.origin.x).top(rect.origin.y).w(rect.size.width).h(rect.size.height)
                .child(div().absolute().inset_0().rounded(radius).bg(c("background")).shadow_lg().opacity(s.lift))
                .child(body))
        });
        let this = cx.entity().downgrade();
        div().id("pane-drag-layer").absolute().inset_0()
            .when(self.pane_held(), |d| d.occlude().cursor(CursorStyle::ClosedHand))
            .children(carried).child(canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let t = this.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = t.update(cx, |this, cx| {
                            this.pane_hover(ev.position, cx);
                            this.pane_move(ev.position, ev.pressed_button == Some(MouseButton::Left), cx);
                        });
                    }
                });
                let t = this.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && ev.button == MouseButton::Left
                        && t.update(cx, |this, cx| this.pane_release(window, cx)).unwrap_or(false)
                    { cx.stop_propagation(); }
                });
            },
        ).absolute().size_full()).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{STRIP, drop_zone, inherited_movable, top_strip};
    use crate::layout::{Axis, Content, Edge, Node};
    use gpui::{Bounds, point, px, size};
    #[test]
    fn zones_are_relative_to_the_pane_and_outside_cancels() {
        let b = Bounds::new(point(px(100.), px(200.)), size(px(800.), px(400.)));
        for (x, y, expected) in [(500., 400., Some(None)), (110., 400., Some(Some(Edge::Left))),
            (890., 400., Some(Some(Edge::Right))), (500., 210., Some(Some(Edge::Top))),
            (500., 590., Some(Some(Edge::Bottom))), (99., 400., None)] {
            assert_eq!(drop_zone(b, point(px(x), px(y))), expected);
        }
    }
    #[test]
    fn hover_strip_is_only_the_panes_top_edge() {
        let b = Bounds::new(point(px(100.), px(200.)), size(px(800.), px(400.)));
        assert!(top_strip(b, STRIP, point(px(110.), px(201.))));
        assert!(!top_strip(b, STRIP, point(px(110.), px(250.))));
        assert!(!top_strip(b, STRIP, point(px(99.), px(201.))));
        assert!(!top_strip(b, STRIP, point(px(500.), px(199.))));
        // A 40px top header is the whole row.
        assert!(top_strip(b, 40., point(px(110.), px(235.))));
        assert!(!top_strip(b, 40., point(px(110.), px(245.))));
    }
    #[test]
    fn movable_uses_the_nearest_explicit_ancestor() {
        let mut root = Node::container("root", Axis::Row, vec![Node::container("inner", Axis::Column,
            vec![Node::block("pane", Content::Terminal { terminal: "t1".into() })])]);
        assert_eq!(inherited_movable(&root, "pane"), None);
        root.movable = Some(false);
        assert_eq!(inherited_movable(&root, "pane"), Some(false));
        root.find_mut("inner").unwrap().movable = Some(true);
        assert_eq!(inherited_movable(&root, "pane"), Some(true));
    }
}
