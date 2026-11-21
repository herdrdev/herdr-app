//! A mod block of the layout shown in its own OS window.
//!
//! Detaching hides the block in the main layout (the `hidden` flag, as
//! `layout.hide`) and opens a window that draws the same mod view, so the
//! mod keeps one instance, one state and one script runtime. Closing the
//! window, or `layout.attach`, shows the block in the layout again.
//!
//! Where each layout's detached windows are (`windows.json`, `detached`) is
//! kept as they move, so the next start, or switching back to the layout,
//! opens them again in the same place. Quitting never forgets one: only the
//! person closing the window or `layout.attach` does.

use super::*;
use gpui::{DisplayId, Pixels, Task};
use serde::{Deserialize, Serialize};

/// A window's spot is written once it has stayed still this long.
const SETTLE: Duration = Duration::from_millis(400);
/// The size of a window that has no saved spot, px.
const DEFAULT: (f32, f32) = (720.0, 520.0);

/// Open detached windows by node id, so a second request activates the
/// window that already shows the node.
#[derive(Default)]
pub(super) struct Detached(pub(super) HashMap<String, WindowHandle<DetachedNode>>);

impl Global for Detached {}

/// Where a detached window was: x, y, width and height, and the display it was on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Spot {
    pub(super) bounds: [f32; 4],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) display: Option<String>,
}

// ---- windows.json: the detached windows of each layout

/// The blocks a layout has detached, with where each window was (None: a spot
/// that cannot be read, so the window opens centered).
pub(super) fn saved(layout: &str) -> Vec<(String, Option<Spot>)> {
    movable::read()
        .get("detached")
        .and_then(|d| d.get(layout))
        .and_then(Value::as_object)
        .map(|l| l.iter().map(|(id, v)| (id.clone(), serde_json::from_value(v.clone()).ok())).collect())
        .unwrap_or_default()
}

pub(super) fn remember(layout: &str, id: &str, spot: &Spot) {
    if let Ok(v) = serde_json::to_value(spot) {
        movable::edit(|f| f["detached"][layout][id] = v);
    }
}

pub(super) fn forget(layout: &str, id: &str) {
    if movable::read().get("detached").and_then(|d| d.get(layout)).and_then(|l| l.get(id)).is_none() {
        return;
    }
    movable::edit(|f| {
        if let Some(l) = f["detached"].get_mut(layout).and_then(Value::as_object_mut) {
            l.remove(id);
        }
    });
}

/// Where `window` is now.
fn spot_of(window: &Window, cx: &App) -> Option<Spot> {
    let (WindowBounds::Windowed(b) | WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b)) = window.inner_window_bounds();
    if b.size.width <= px(0.0) {
        return None;
    }
    let bounds = [f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)];
    Some(Spot { bounds, display: window.display(cx).and_then(|d| crate::place::uuid(d.as_ref())) })
}

/// Where to open: the saved spot if its display is still there and the
/// window's corner is on it, else the saved size centered (on that display, or
/// the main one if it is gone).
fn placed(spot: Option<&Spot>, cx: &App) -> (Bounds<Pixels>, Option<DisplayId>) {
    let Some(s) = spot else {
        return (Bounds::centered(None, size(px(DEFAULT.0), px(DEFAULT.1)), cx), None);
    };
    let [x, y, w, h] = s.bounds;
    let (w, h) = (w.max(240.0), h.max(160.0));
    let display = s.display.as_deref().and_then(|want| cx.displays().into_iter().find(|d| crate::place::uuid(d.as_ref()).as_deref() == Some(want)));
    let at = point(px(x), px(y));
    let bounds = match &display {
        // Linux: Wayland doesn't let a window place itself and X11 window
        // managers shift it by their frame each time, so only its size comes
        // back (as for the main window, place.rs).
        Some(d) if !cfg!(target_os = "linux") && d.bounds().contains(&at) => Bounds::new(at, size(px(w), px(h))),
        _ => Bounds::centered(display.as_ref().map(|d| d.id()), size(px(w), px(h)), cx),
    };
    (bounds, display.map(|d| d.id()))
}

pub(super) struct DetachedNode {
    layout: String,
    id: String,
    app: Entity<AppWindow>,
    /// The mod's view, the main window's own. It is not read from the app on
    /// every draw: a window opens (and draws) inside the command that made it,
    /// while the app is being updated and cannot be read. None until the mod
    /// has loaded; the observer below fills it in.
    root: Option<Entity<ShellRoot>>,
    /// The spot's write, waiting for the window to settle (a new move replaces it).
    write: Option<Task<()>>,
    _watch: Subscription,
    _place: Option<Subscription>,
}

impl DetachedNode {
    /// The mod's view this window draws.
    pub(super) fn root(&self) -> Option<Entity<ShellRoot>> {
        self.root.clone()
    }

    /// The person closed the window: the block goes back into the layout and
    /// is not opened again.
    pub(super) fn release(&self, cx: &mut Context<Self>) {
        // The OS ending the session closes windows too: those come back.
        if QUITTING.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        if cx.has_global::<Detached>() {
            cx.global_mut::<Detached>().0.remove(&self.id);
        }
        forget(&self.layout, &self.id);
        let id = self.id.clone();
        self.app.update(cx, |app, cx| {
            app.detached.remove(&id);
            if let Some(n) = app.chrome.find_mut(&id) {
                n.hidden = false;
            }
            app.save();
            cx.notify();
        });
    }
}

impl Render for DetachedNode {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().id("detached").size_full().bg(c("background")).children(self.root())
    }
}

/// Opens node `id` of `layout` in its own OS window, with the system's
/// (resizable) title bar, where `spot` says. The caller hides the node.
pub(super) fn open(layout: &str, id: &str, root: Option<Entity<ShellRoot>>, spot: Option<&Spot>, app: &Entity<AppWindow>, cx: &mut App) -> Result<(), String> {
    if let Some(open) = cx.try_global::<Detached>().and_then(|d| d.0.get(id).cloned())
        && open.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return Ok(());
    }
    let (bounds, display_id) = placed(spot, cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id,
        titlebar: Some(TitlebarOptions { title: Some(format!("Herdr · {id}").into()), ..Default::default() }),
        // One app wherever the platform groups windows by app id.
        app_id: Some(std::env::var("HERDR_APP_ID").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "herdr-app".into())),
        ..Default::default()
    };
    let (layout_name, node, app) = (layout.to_owned(), id.to_owned(), app.clone());
    let handle = cx
        .open_window(options, move |_, cx| {
            cx.new(|cx: &mut Context<DetachedNode>| DetachedNode {
                layout: layout_name,
                id: node,
                root,
                write: None,
                // The main window notifies when a mod loads, reloads or
                // changes; this one follows it, once that update is over.
                _watch: cx.observe(&app, |this, app, cx| {
                    this.root = app.read(cx).copies.get(&this.id).and_then(|c| c.root.clone());
                    cx.notify();
                }),
                _place: None,
                app,
            })
        })
        .map_err(|e| format!("cannot open a window: {e}"))?;
    if !cx.has_global::<Detached>() {
        cx.set_global(Detached::default());
    }
    cx.global_mut::<Detached>().0.insert(id.to_owned(), handle.clone());
    let _ = handle.update(cx, |this, window, cx| {
        if let Some(s) = spot_of(window, cx) {
            remember(&this.layout, &this.id, &s);
        }
        // Moved or resized: written once it settles, which cancels the last write.
        this._place = Some(cx.observe_window_bounds(window, |this, window, cx| {
            let Some(spot) = spot_of(window, cx) else { return };
            let (layout, id) = (this.layout.clone(), this.id.clone());
            this.write = Some(cx.spawn(async move |_, cx| {
                cx.background_executor().timer(SETTLE).await;
                remember(&layout, &id, &spot);
            }));
        }));
        let closing = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = closing.update(cx, |this, cx| this.release(cx));
            true
        });
        window.activate_window();
    });
    Ok(())
}

/// Closes the window showing `id`, if one is open. It does not touch the
/// layout or the saved spot: the caller decides whether the block comes back.
pub(super) fn close(id: &str, cx: &mut App) {
    let Some(open) = cx.try_global::<Detached>().and_then(|d| d.0.get(id).cloned()) else { return };
    cx.global_mut::<Detached>().0.remove(id);
    let _ = open.update(cx, |_, window, _| window.remove_window());
}

/// Closes every detached window, keeping their saved spots (the app or the
/// layout is going away, and they come back with it).
pub(super) fn close_all(cx: &mut App) {
    let ids: Vec<String> = cx.try_global::<Detached>().map(|d| d.0.keys().cloned().collect()).unwrap_or_default();
    for id in ids {
        close(&id, cx);
    }
}

impl AppWindow {
    /// Whether `id` is a block that shows a mod: the only kind that detaches.
    pub(super) fn detachable(&self, id: &str) -> bool {
        matches!(self.chrome.find(id).map(|n| &n.kind), Some(Kind::Block { content: Content::Mod { .. } }))
    }

    /// Shows block `id` in its own window (at `spot`, else centered) and hides
    /// it here. Its mod is mounted if it never was (an inactive stack child).
    pub(super) fn detach_block(&mut self, id: &str, spot: Option<&Spot>, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        self.detached.insert(id.to_owned());
        if let Some(n) = self.chrome.find_mut(id) {
            n.hidden = true;
        }
        self.sync(window, cx);
        let root = self.copies.get(id).and_then(|c| c.root.clone());
        if let Err(e) = open(&self.layout_name, id, root, spot, &cx.entity(), cx) {
            self.detached.remove(id);
            if let Some(n) = self.chrome.find_mut(id) {
                n.hidden = false;
            }
            return Err(e);
        }
        self.save();
        cx.notify();
        Ok(())
    }

    /// The layout's detached windows, opened again where they were (at start,
    /// or on switching to the layout). A block the layout no longer has is
    /// left alone: the layout file may be mid-edit, and its spot is harmless.
    pub(super) fn restore_detached(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (id, spot) in saved(&self.layout_name) {
            if self.detached.contains(&id) || !self.detachable(&id) {
                continue;
            }
            if let Err(e) = self.detach_block(&id, spot.as_ref(), window, cx) {
                tracing::warn!("cannot reopen the detached window of {id:?}: {e}");
            }
        }
    }
}
