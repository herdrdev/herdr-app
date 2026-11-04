//! Geometry for panes that aren't drawn: programs starting, and tabs that are hidden.
//! Layout metrics are shared with the renderer; normal resize still handles pixel rounding.

use super::*;
use crate::layout::Header;

pub(super) fn node_border(n: &Node, focused: bool) -> Option<&Line> {
    n.focus.as_ref().filter(|_| focused).and_then(|f| f.border.as_ref()).or(n.border.as_ref())
}

pub(super) fn padding(n: &Node) -> (f32, f32) {
    n.padding.map_or((0.0, 0.0), |p| p.axes())
}

pub(super) fn frame_border(frame: &PaneFrame, focused: bool) -> Option<&Line> {
    if focused { frame.focus.as_ref().or(frame.border.as_ref()) } else { frame.border.as_ref() }
}

pub(super) fn header_height(header: &Header) -> f32 {
    header.height.unwrap_or(HEADER_HEIGHT)
}

pub(super) fn gap(n: &Node, frame: Option<&PaneFrame>) -> f32 {
    n.gap.or(frame.and_then(|f| f.gap)).unwrap_or(0.0)
}

pub(super) fn separator_width(n: &Node, gap: f32) -> f32 {
    gap + n.divider.as_ref().map_or(0.0, |line| line.width)
}

pub(super) fn weight(n: &Node) -> f32 {
    match n.size {
        Some(Span::Px { .. }) => 0.0,
        Some(Span::Weight { weight }) => weight,
        None => 1.0,
    }
}

fn inset(size: Size<Pixels>, x: f32, y: f32) -> Size<Pixels> {
    gpui::size((size.width - px(x)).max(px(0.0)), (size.height - px(y)).max(px(0.0)))
}

fn content_size(n: &Node, size: Size<Pixels>, focused: bool, zoom: f32) -> Size<Pixels> {
    let border = node_border(n, focused).map_or(0.0, |line| line.width);
    let (y, x) = padding(n);
    inset(size, 2.0 * (x + border) * zoom, 2.0 * (y + border) * zoom)
}

/// Flex shares after fixed siblings, gaps and min/max constraints.
fn shares(children: &[&Node], available: f32, zoom: f32) -> Vec<f32> {
    let mut sizes = vec![0.0; children.len()];
    let mut pending = Vec::new();
    let mut left = available;
    for (i, n) in children.iter().enumerate() {
        match n.size {
            Some(Span::Px { px }) => {
                sizes[i] = (px * zoom).max(n.min.unwrap_or(0.0) * zoom).min(n.max.unwrap_or(f32::INFINITY) * zoom);
                left -= sizes[i];
            }
            _ => pending.push(i),
        }
    }
    let total_weight = pending.iter().map(|&i| weight(children[i])).sum::<f32>().max(f32::EPSILON);
    let initial_free = left.max(0.0);
    while !pending.is_empty() {
        let total = pending.iter().map(|&i| weight(children[i])).sum::<f32>().max(f32::EPSILON);
        // Grow factors stay normalized to the original siblings after an item freezes.
        let free = left.max(0.0).min(initial_free * (total / total_weight));
        let mut violations = Vec::new();
        for &i in &pending {
            let n = children[i];
            let share = free * weight(n) / total;
            sizes[i] = share.min(n.max.unwrap_or(f32::INFINITY) * zoom).max(n.min.unwrap_or(0.0) * zoom);
            violations.push((i, sizes[i] - share));
        }
        let total_violation = violations.iter().map(|(_, violation)| violation).sum::<f32>();
        if total_violation == 0.0 { break; }
        for (i, violation) in violations {
            if (total_violation > 0.0 && violation <= 0.0) || (total_violation < 0.0 && violation >= 0.0) { continue; }
            left -= sizes[i];
            pending.retain(|&j| j != i);
        }
    }
    sizes
}

fn pane_size(n: &Node, size: Size<Pixels>, target: &str, frame: Option<&PaneFrame>, focused: &HashSet<String>, zoom: f32) -> Option<Size<Pixels>> {
    if n.hidden { return None; }
    let mut size = content_size(n, size, focused.contains(&n.id), zoom);
    match &n.kind {
        Kind::Block { .. } if n.id == target => {
            if let Some(frame) = frame.filter(|_| n.is_pane()) {
                let border = frame_border(frame, focused.contains(&n.id)).map_or(0.0, |line| line.width) * zoom;
                let header = frame.header.as_ref().filter(|h| !h.hover()).map_or(0.0, header_height) * zoom;
                size = inset(size, 2.0 * border, 2.0 * border + header);
            }
            Some(size)
        }
        Kind::Row { children } | Kind::Column { children } => {
            let row = n.axis() == Some(Axis::Row);
            let children: Vec<_> = children.iter().filter(|n| !n.hidden).collect();
            let separators = separator_width(n, gap(n, frame)) * zoom * children.len().saturating_sub(1) as f32;
            let available = f32::from(if row { size.width } else { size.height }) - separators;
            for (child, span) in children.iter().zip(shares(&children, available, zoom)) {
                let size = if row { gpui::size(px(span), size.height) } else { gpui::size(size.width, px(span)) };
                if let Some(size) = pane_size(child, size, target, frame, focused, zoom) { return Some(size); }
            }
            None
        }
        Kind::Stack { header, .. } => {
            let height = header.as_ref().map_or(0.0, header_height) * zoom;
            pane_size(n.active_child()?, inset(size, 0.0, height), target, frame, focused, zoom)
        }
        _ => None,
    }
}

/// The tab area and the cell size.
type Geometry = (Size<Pixels>, Size<Pixels>);

/// Whether hidden tabs fit the window: the geometry seen at the last tick, and
/// whether they were sized for it.
#[derive(Default)]
pub(super) enum HiddenFit {
    #[default]
    Unknown,
    Seen(Geometry),
    Done(Geometry),
}

impl AppWindow {
    pub(super) fn initial_size(&self, root: &Node, pane: &str, window: &Window) -> Option<(u16, u16)> {
        let focused = root.path_to(pane)?.into_iter().collect();
        self.displayed_size(root, root.panes().len(), pane, &focused, window)
    }

    /// The grid `pane` gets when `tab` is the one shown.
    pub(super) fn shown_size(&self, tab: &Tab, pane: &str, window: &Window) -> Option<(u16, u16)> {
        let focused = tab.focused_pane().and_then(|f| tab.root.path_to(&f)).unwrap_or_default().into_iter().collect();
        self.displayed_size(tab.shown(), tab.root.panes().len(), pane, &focused, window)
    }

    /// `shown`: the tree on screen; `panes`: how many the tab has (its frame needs more than one, or `lone`).
    fn displayed_size(&self, shown: &Node, panes: usize, pane: &str, focused: &HashSet<String>, window: &Window) -> Option<(u16, u16)> {
        let (context, area) = self.context_area()?;
        let frame = match context.content() {
            Some(Content::Context { pane: Some(frame), .. }) if frame.lone || panes > 1 => Some(frame),
            _ => None,
        };
        let zoom = settings::zoom();
        // The absolute bounds canvas already excludes the context border (padding box).
        let (y, x) = padding(context);
        let size = inset(area, 2.0 * x * zoom, 2.0 * y * zoom);
        let size = pane_size(shown, size, pane, frame, focused, zoom)?;
        Some(crate::terminal::grid_size(size, crate::terminal::cell_size(window)))
    }

    /// The block tabs show in, and the area it was last drawn with.
    fn context_area(&self) -> Option<(&Node, Size<Pixels>)> {
        let context = self.chrome.visible_blocks().into_iter().find(|n| matches!(n.content(), Some(Content::Context { .. })))?;
        let size = self.bounds.borrow().get(&context.id)?.size;
        (size.width > px(0.0) && size.height > px(0.0)).then_some((context, size))
    }

    /// Terminals in hidden tabs keep the size they last had on screen, so a tab shown
    /// after the window changed size draws its old screens, then jumps once the programs
    /// redraw. Once the window settles (the same at two ticks), they are sized as their
    /// panes would be, so the programs redraw out of sight.
    pub(super) fn fit_hidden_tabs(&mut self, window: &Window, cx: &App) {
        let Some((_, area)) = self.context_area() else { return };
        let cell = crate::terminal::cell_size(window);
        let now = (area, cell);
        match self.hidden_fit {
            HiddenFit::Done(g) if g == now => return,
            HiddenFit::Seen(g) if g == now => self.hidden_fit = HiddenFit::Done(now),
            _ => {
                self.hidden_fit = HiddenFit::Seen(now);
                return;
            }
        }
        let scale = window.scale_factor();
        let active = self.model.active_tab().map(|t| t.id.clone());
        for tab in self.model.tabs().filter(|t| Some(&t.id) != active.as_ref()) {
            for pane in tab.shown().panes() {
                if let Some(t) = self.terms.get(&pane.id)
                    && let Some((cols, rows)) = self.shown_size(tab, &pane.id, window)
                {
                    t.view.read(cx).fit(cols, rows, cell, scale);
                }
            }
        }
    }
}
