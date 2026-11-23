//! Floating blocks: mods (or terminals) over a node, snapped to one of its
//! edges and a third along it. Drag one anywhere; let go and it springs to
//! the nearest edge. Left and right make it vertical, top and bottom
//! horizontal; its mod reads that with `placement()`.
//!
//! A block with `peek` on a side edge is a drawer: the mod draws one full
//! layout, Herdr shows only the `peek` px next to the edge and slides the rest
//! out on hover. Nothing inside moves; only the cut does.

use super::*;
use crate::layout::{Align, Edge, Float, snap};

/// Space between a floating block and its node's edge, in px at zoom 1.
const MARGIN: f32 = 12.0;
/// How far the pointer moves before a press becomes a drag.
const DRAG_START: f32 = 4.0;
const SNAP_MS: f32 = 360.0;

/// Space above and below the content inside a floating block's frame.
const FRAME_PAD: f32 = 5.0;
/// How long a drawer takes to slide open or shut.
const REVEAL_MS: f32 = 220.0;
/// How long the pointer may be away before a floating block counts as not hovered.
const LEAVE_MS: u64 = 280;

pub fn slot(id: &str) -> String {
    format!("float:{id}")
}

fn content_slot(id: &str) -> String {
    format!("float:{id}#content")
}

/// How open a drawer is (0 shut, 1 open), easing toward where hover wants it.
pub struct Reveal {
    from: f32,
    to: f32,
    start: Instant,
}

impl Reveal {
    fn now(&self) -> f32 {
        let t = (self.start.elapsed().as_secs_f32() * 1000.0 / REVEAL_MS).clamp(0.0, 1.0);
        let e = 1.0 - (1.0 - t).powi(3);
        self.from + (self.to - self.from) * e
    }

    fn running(&self) -> bool {
        (self.now() - self.to).abs() > 1e-3
    }
}

pub struct FloatDrag {
    id: String,
    owner: String,
    down: Point<Pixels>,
    pointer: Point<Pixels>,
    /// Where the pointer holds the block, as shares of its size.
    grab: (f32, f32),
    moving: bool,
    /// Where it lands if dropped now (it already takes that shape).
    target: (Edge, Align),
}

pub struct FloatAnim {
    /// Offset from its place when the spring started; it eases to zero.
    from: Point<Pixels>,
    start: Instant,
}

/// Ease out with a small overshoot, so the snap feels like a spring.
fn spring(t: f32) -> f32 {
    let (c1, t) = (1.25, t.clamp(0.0, 1.0) - 1.0);
    1.0 + (c1 + 1.0) * t * t * t + c1 * t * t
}

/// Where a block of `size` sits in `owner` at a placement (top-left corner).
fn home(owner: Bounds<Pixels>, size: Size<Pixels>, (edge, align): (Edge, Align)) -> Point<Pixels> {
    let m = settings::z(MARGIN);
    let along = |start: Pixels, room: Pixels, len: Pixels| match align {
        Align::Start => start + m,
        Align::Center => start + (room - len) / 2.0,
        Align::End => start + room - m - len,
    };
    let x = match edge {
        Edge::Left => owner.origin.x + m,
        Edge::Right => owner.right() - m - size.width,
        Edge::Top | Edge::Bottom => along(owner.origin.x, owner.size.width, size.width),
    };
    let y = match edge {
        Edge::Top => owner.origin.y + m,
        Edge::Bottom => owner.bottom() - m - size.height,
        Edge::Left | Edge::Right => along(owner.origin.y, owner.size.height, size.height),
    };
    point(x, y)
}

impl AppWindow {
    /// What `placement()` tells each floating mod (a dragged one: where it would land).
    pub(super) fn float_placements(&self) -> Value {
        let mut out = serde_json::Map::new();
        for n in self.chrome.walk() {
            for f in &n.floating {
                let drag = self.float_drag.as_ref().filter(|d| d.id == f.id && d.moving);
                let (edge, align) = drag.map_or((f.edge, f.align), |d| d.target);
                let hovered = drag.is_some() || self.float_hover.contains(&f.id);
                out.insert(f.id.clone(), json!({ "edge": edge, "align": align, "vertical": edge.vertical(), "dragging": drag.is_some(), "hovered": hovered }));
            }
        }
        Value::Object(out)
    }

    /// Placements plus where each sits in the window (px), for `herdr-app status`.
    pub(super) fn float_status(&self) -> Value {
        let mut v = self.float_placements();
        let bounds = self.bounds.borrow();
        if let Some(map) = v.as_object_mut() {
            for (id, info) in map.iter_mut() {
                if let Some(b) = bounds.get(&slot(id)) {
                    info["bounds"] = json!([f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)]);
                }
            }
        }
        v
    }

    pub(super) fn render_floats(&self, n: &Node, cx: &mut Context<Self>) -> Vec<AnyElement> {
        n.floating.iter().filter(|f| !f.hidden).map(|f| self.render_float(n, f, cx)).collect()
    }

    fn render_float(&self, owner: &Node, f: &Float, cx: &mut Context<Self>) -> AnyElement {
        let slot = slot(&f.id);
        let content = match (&f.content, self.copies.get(&slot), self.terms.get(&slot)) {
            (Content::Terminal { .. }, _, Some(t)) => {
                div().w(settings::z(560.0)).h(settings::z(320.0)).rounded(settings::z(10.0)).overflow_hidden().child(super::shown_terminal(&t.view, cx)).into_any_element()
            }
            (_, Some(Copy { root: Some(root), inst, .. }), _) => {
                div().key_context(super::key_context("Mod", "mod", &inst.name)).child(root.clone()).into_any_element()
            }
            (_, Some(Copy { error: Some(e), inst, .. }), _) => div()
                .max_w(settings::z(360.0))
                .p_2()
                .rounded(settings::z(8.0))
                .bg(c("popover"))
                .text_xs()
                .text_color(c("blocked"))
                .child(format!("{} did not load: {e}", inst.name))
                .into_any_element(),
            _ => div().into_any_element(),
        };
        let measure = |key: String| {
            let bounds_map = self.bounds.clone();
            canvas(move |b, _, _| { bounds_map.borrow_mut().insert(key, b); }, |_, _, _, _| {}).absolute().size_full()
        };
        let drag = self.float_drag.as_ref().filter(|d| d.id == f.id && d.moving);
        let (edge, _) = drag.map_or((f.edge, f.align), |d| d.target);

        // A drawer: the content keeps its full size; the box shows `peek` px of
        // it next to the edge, more as it opens.
        let natural = self.bounds.borrow().get(&content_slot(&f.id)).map(|b| b.size);
        let peek = f.peek.filter(|_| edge.vertical()).map(|p| settings::z(p));
        let open = self.float_reveal.get(&f.id).map_or(0.0, Reveal::now);
        let cut = match (peek, natural) {
            (Some(p), Some(n)) if p < n.width => Some(size(p + (n.width - p) * open, n.height)),
            _ => None,
        };
        // Content size is only known after layout: when it changes, draw again
        // at once so the cut follows (else it waits for some other redraw).
        let content_measure = {
            let (bounds_map, key) = (self.bounds.clone(), content_slot(&f.id));
            canvas(
                move |b, window, _| {
                    if natural.is_none_or(|n| (n.width - b.size.width).abs() > px(0.5) || (n.height - b.size.height).abs() > px(0.5)) {
                        window.request_animation_frame();
                    }
                    bounds_map.borrow_mut().insert(key, b);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full()
        };
        // Herdr draws the frame (like a dialog's), so a closed drawer is a whole
        // slim panel; the mod draws only what's inside, edge to edge.
        let pad = settings::z(FRAME_PAD);
        let border = px(1.0);
        let empty = natural.is_some_and(|n| n.width < px(1.0) || n.height < px(1.0));
        let mut inner = div().child(content_measure).child(content);
        if let (Some(c), Some(n)) = (cut, natural) {
            // Held to the edge: the side away from it is what the cut hides.
            let x = if edge == Edge::Right { c.width - n.width } else { px(0.0) };
            inner = inner.absolute().left(x).top(pad);
        }
        let (id, owner_id, hover_id) = (f.id.clone(), owner.id.clone(), f.id.clone());
        let mut body = div()
            .id(SharedString::from(slot.clone()))
            .relative()
            .when(!empty, |b| {
                b.py(pad)
                    .bg(c("popover"))
                    .border_1()
                    .border_color(c(if drag.is_some() { "accent" } else { "border" }))
                    .rounded(settings::z(12.0))
                    .shadow_lg()
            })
            .when_some(cut.zip(natural), |b, (c, n)| {
                b.w(c.width + border * 2.0).h(n.height + pad * 2.0 + border * 2.0).overflow_hidden()
            })
            .occlude()
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| this.float_hovered(&hover_id, *hovered, window, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, _| this.float_press(&id, &owner_id, ev.position)))
            .child(inner);
        if let Some(a) = self.float_anims.get(&f.id) {
            let left = 1.0 - spring(a.start.elapsed().as_secs_f32() * 1000.0 / SNAP_MS);
            body = body.left(a.from.x * left).top(a.from.y * left);
        }
        // The outer box is what's measured and placed: the spring offset doesn't move it.
        let mut outer = div().relative().flex_none().child(measure(slot.clone())).child(body);
        if natural.is_none() {
            // Measured once before it shows, so it never flashes open.
            outer = outer.opacity(0.0);
        }

        let bounds = self.bounds.borrow();
        if let (Some(d), Some(ob)) = (drag, bounds.get(&owner.id)) {
            let size = bounds.get(&slot).map(|b| b.size).unwrap_or_default();
            let x = d.pointer.x - ob.origin.x - size.width * d.grab.0;
            let y = d.pointer.y - ob.origin.y - size.height * d.grab.1;
            return div().absolute().inset_0().child(outer.absolute().left(x).top(y).opacity(0.92)).into_any_element();
        }
        let mut layer = div().absolute().inset_0().p(settings::z(MARGIN)).flex();
        layer = if f.edge.vertical() { layer.flex_row() } else { layer.flex_col() };
        layer = match f.edge {
            Edge::Left | Edge::Top => layer.justify_start(),
            Edge::Right | Edge::Bottom => layer.justify_end(),
        };
        layer = match f.align {
            Align::Start => layer.items_start(),
            Align::Center => layer.items_center(),
            Align::End => layer.items_end(),
        };
        layer.child(outer).into_any_element()
    }

    fn float_press(&mut self, id: &str, owner: &str, at: Point<Pixels>) {
        let b = self.bounds.borrow().get(&slot(id)).copied();
        let share = |v: Pixels, start: Pixels, len: Pixels| if len > px(0.0) { f32::from(v - start) / f32::from(len) } else { 0.5 };
        let grab = b.map_or((0.5, 0.5), |b| (share(at.x, b.origin.x, b.size.width), share(at.y, b.origin.y, b.size.height)));
        let target = self.chrome.walk().iter().flat_map(|n| &n.floating).find(|f| f.id == id).map(|f| (f.edge, f.align)).unwrap_or_default();
        self.float_drag = Some(FloatDrag { id: id.to_owned(), owner: owner.to_owned(), down: at, pointer: at, grab, moving: false, target });
    }

    fn float_move(&mut self, at: Point<Pixels>, pressed: bool, cx: &mut Context<Self>) {
        let Some(d) = &mut self.float_drag else { return };
        if !pressed {
            self.float_drag = None;
            cx.notify();
            return;
        }
        d.pointer = at;
        let dist = f32::from((at.x - d.down.x).abs()).max(f32::from((at.y - d.down.y).abs()));
        if !d.moving && dist < DRAG_START {
            return;
        }
        d.moving = true;
        let bounds = self.bounds.borrow();
        if let (Some(ob), Some(b)) = (bounds.get(&d.owner), bounds.get(&slot(&d.id))) {
            let cx_ = at.x - b.size.width * d.grab.0 + b.size.width / 2.0 - ob.origin.x;
            let cy_ = at.y - b.size.height * d.grab.1 + b.size.height / 2.0 - ob.origin.y;
            d.target = snap(f32::from(cx_), f32::from(cy_), f32::from(ob.size.width), f32::from(ob.size.height));
        }
        drop(bounds);
        if mods::publish_floats(self.float_placements()) {
            self.refresh_mods(cx);
        }
        cx.notify();
    }

    /// Ends a press. A drag drops and springs into place (returns true so the
    /// release doesn't also click what's under it); a plain press was a click.
    fn float_release(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(d) = self.float_drag.take() else { return false };
        cx.defer_in(window, |this, window, cx| this.refocus(window, cx));
        if !d.moving {
            return false;
        }
        let bounds = self.bounds.borrow();
        let anim = match (bounds.get(&d.owner), bounds.get(&slot(&d.id))) {
            (Some(ob), Some(b)) => {
                let from = point(at.x - b.size.width * d.grab.0, at.y - b.size.height * d.grab.1);
                let to = home(*ob, b.size, d.target);
                Some(FloatAnim { from: point(from.x - to.x, from.y - to.y), start: Instant::now() })
            }
            _ => None,
        };
        drop(bounds);
        if let Some(f) = self.chrome.find_float_mut(&d.id) {
            (f.edge, f.align) = d.target;
        }
        if let Some(a) = anim {
            self.float_anims.insert(d.id, a);
        }
        self.save();
        if mods::publish_floats(self.float_placements()) {
            self.refresh_mods(cx);
        }
        cx.notify();
        true
    }

    /// Gives the keyboard back to what the layout shows (a press on a mod takes it).
    fn refocus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlays.iter().any(|o| o.kind.modal_keys()) {
            return;
        }
        let term = match self.agent_block() {
            Some(block) => self.shown_agent().and_then(|a| self.agent_view(&block, &a)),
            None => self.focused_pane.as_ref().and_then(|p| self.terms.get(p)),
        };
        if let Some(t) = term {
            t.focus.focus(window, cx);
        }
    }

    /// Starts easing any floating block whose content changed size, drops
    /// finished springs, and asks for frames while anything moves.
    pub(super) fn tick_floats(&mut self, window: &mut Window) {
        let ids: Vec<String> = self.chrome.walk().iter().flat_map(|n| &n.floating).map(|f| f.id.clone()).collect();
        self.float_reveal.retain(|id, _| ids.contains(id));
        for id in ids {
            // Open while hovered or carried.
            let dragged = self.float_drag.as_ref().is_some_and(|d| d.id == id && d.moving);
            let want = if dragged || self.float_hover.contains(&id) { 1.0 } else { 0.0 };
            let now = self.float_reveal.get(&id).map_or(0.0, Reveal::now);
            if self.float_reveal.get(&id).is_none_or(|r| r.to != want) {
                self.float_reveal.insert(id, Reveal { from: now, to: want, start: Instant::now() });
            }
        }
        self.float_anims.retain(|_, a| a.start.elapsed().as_secs_f32() * 1000.0 < SNAP_MS);
        if !self.float_anims.is_empty() || self.float_reveal.values().any(Reveal::running) {
            window.request_animation_frame();
        }
    }

    /// Hover tells the mod (`placement().hovered`) at once; leaving waits a
    /// moment, so crossing a gap or the edge doesn't make it flicker.
    fn float_hovered(&mut self, id: &str, hovered: bool, window: &mut Window, cx: &mut Context<Self>) {
        let seq = self.float_leave.entry(id.to_owned()).or_default();
        *seq += 1;
        if hovered {
            if self.float_hover.insert(id.to_owned()) && mods::publish_floats(self.float_placements()) {
                self.refresh_mods(cx);
            }
            cx.notify();
            return;
        }
        let (id, mine) = (id.to_owned(), *seq);
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(LEAVE_MS)).await;
            let _ = this.update(cx, |this, cx| {
                if this.float_leave.get(&id) == Some(&mine) && this.float_hover.remove(&id) {
                    if mods::publish_floats(this.float_placements()) {
                        this.refresh_mods(cx);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// While a floating block is pressed, follows the pointer anywhere in the window.
    pub(super) fn float_tracker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.float_drag.as_ref()?;
        let this = cx.entity().downgrade();
        Some(
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let t = this.clone();
                    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture {
                            let _ = t.update(cx, |this, cx| this.float_move(ev.position, ev.pressed_button == Some(MouseButton::Left), cx));
                        }
                    });
                    let t = this.clone();
                    window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture && ev.button == MouseButton::Left {
                            let dropped = t.update(cx, |this, cx| this.float_release(ev.position, window, cx)).unwrap_or(false);
                            if dropped {
                                cx.stop_propagation();
                            }
                        }
                    });
                },
            )
            .absolute()
            .size_full()
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Align, Edge, home, spring};
    use gpui::{Bounds, point, px, size};

    #[test]
    fn the_spring_overshoots_then_settles() {
        assert_eq!(spring(0.0), 0.0);
        assert!((spring(1.0) - 1.0).abs() < 1e-6);
        assert!((1..100).map(|i| spring(i as f32 / 100.0)).any(|v| v > 1.0));
    }

    #[test]
    fn home_respects_edges_and_thirds() {
        let owner = Bounds::new(point(px(0.0), px(0.0)), size(px(1000.0), px(600.0)));
        let s = size(px(40.0), px(200.0));
        assert_eq!(home(owner, s, (Edge::Left, Align::Center)), point(px(12.0), px(200.0)));
        assert_eq!(home(owner, s, (Edge::Right, Align::End)), point(px(948.0), px(388.0)));
        assert_eq!(home(owner, s, (Edge::Top, Align::Start)), point(px(12.0), px(12.0)));
    }
}
