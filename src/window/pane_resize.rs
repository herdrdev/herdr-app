//! Keyboard resize geometry, independent of window bounds and document writes.

use crate::layout::{Axis, Kind, Node, Size};

impl super::AppWindow {
    pub(super) fn resize_pane(&mut self, args: &serde_json::Value) -> Result<serde_json::Value, String> {
        use serde_json::{Value, json};
        let (axis, sign) = match args.get("direction").and_then(Value::as_str) {
            Some("left") => (Axis::Row, -1.0),
            Some("right") => (Axis::Row, 1.0),
            Some("up") => (Axis::Column, -1.0),
            Some("down") => (Axis::Column, 1.0),
            _ => return Err("pane.resize direction is left, right, up or down".into()),
        };
        let amount = match args.get("amount") {
            None => 10.0,
            Some(v) => v.as_f64().ok_or("pane.resize amount must be a finite positive number")? as f32,
        };
        if !amount.is_finite() || amount <= 0.0 { return Err("pane.resize amount must be a finite positive number".into()); }
        let pane = match args.get("pane") {
            None | Some(Value::Null) => self.model.focused_pane().ok_or("no pane to resize")?,
            Some(Value::String(pane)) => pane.clone(),
            _ => return Err("pane.resize pane must be a pane id".into()),
        };
        let tab = self.model.tab_of(&pane).ok_or_else(|| format!("no pane {pane:?}"))?;
        if self.model.active_tab().map(|t| &t.id) != Some(&tab.id) { return Err("pane.resize requires a pane in the shown tab".into()); }
        if !tab.root.find(&pane).is_some_and(Node::is_pane) { return Err("pane.resize requires a pane".into()); }
        let Some((parent, a, _)) = pair(&tab.root, &pane, axis) else { return Ok(json!({"ok": true})); };
        let index = parent.children().iter().filter(|n| !n.hidden).position(|n| n.id == a.id).unwrap();
        let available = self.split_available(tab, parent).ok_or("pane.resize geometry is unavailable")?;
        let Some(updates) = sizes(parent, index, available, sign * amount) else { return Ok(json!({"ok": true})); };
        let (tab_id, parent_id) = (tab.id.clone(), parent.id.clone());
        let tab = self.model.tab_mut(&tab_id).unwrap();
        let children = tab.root.find_mut(&parent_id).unwrap().children_mut().unwrap();
        for (child, size) in children.iter_mut().filter(|n| !n.hidden).zip(updates) { child.size = size; }
        tab.zoomed = None;
        Ok(json!({"ok": true}))
    }
}

/// Nearest matching ancestor and its adjacent visible children. A disabled
/// split is a boundary; hidden nodes and inactive stack branches cannot resize.
pub(super) fn pair<'a>(root: &'a Node, pane: &str, axis: Axis) -> Option<(&'a Node, &'a Node, &'a Node)> {
    let path = root.path_to(pane)?;
    for ids in path.windows(2) {
        let parent = root.find(&ids[0])?;
        let child = root.find(&ids[1])?;
        if parent.hidden || child.hidden { return None; }
        if matches!(parent.kind, Kind::Stack { .. }) && parent.active_child()?.id != child.id {
            return None;
        }
    }
    for ids in path.windows(2).rev() {
        let parent = root.find(&ids[0])?;
        if parent.axis() != Some(axis) { continue; }
        if !parent.resizable() { return None; }
        let children: Vec<_> = parent.children().iter().filter(|n| !n.hidden).collect();
        if children.len() < 2 { continue; }
        let index = children.iter().position(|n| n.id == ids[1])?.min(children.len() - 2);
        return Some((parent, children[index], children[index + 1]));
    }
    None
}

/// Return sizes for all visible siblings after a signed logical-pixel move.
/// `available` excludes separators and uses unzoomed pixels. Recompute geometry
/// from the current model so repeated commands do not depend on stale bounds.
/// Only the selected pair's stored sizes change. Reject a move if the flex
/// representation cannot preserve the other siblings and the pair's extent.
pub(super) fn sizes(parent: &Node, index: usize, available: f32, delta: f32) -> Option<Vec<Option<Size>>> {
    if !available.is_finite() || available <= 0.0 || !delta.is_finite() || delta == 0.0 { return None; }
    let mut children: Vec<Node> = parent.children().iter().filter(|n| !n.hidden).cloned().collect();
    let (a, b) = (children.get(index)?, children.get(index.checked_add(1)?)?);
    let measure = |nodes: &[Node]| super::pane_size::shares(&nodes.iter().collect::<Vec<_>>(), available, 1.0);
    let before = measure(&children);
    if before.iter().any(|v| !v.is_finite() || *v < 0.0) { return None; }
    let (la, lb) = (before[index], before[index + 1]);
    let total = la + lb;
    let low = a.min.unwrap_or(24.0).max(24.0).max(total - b.max.unwrap_or(f32::INFINITY));
    let high = a.max.unwrap_or(f32::INFINITY).min(total - b.min.unwrap_or(24.0).max(24.0));
    if !low.is_finite() || !high.is_finite() || low > high { return None; }
    let new_a = (la + delta).clamp(low, high);
    if new_a == la || (new_a - la).signum() != delta.signum() { return None; }
    let mut expected = before;
    expected[index] = new_a;
    expected[index + 1] = total - new_a;
    let fixed_a = matches!(a.size, Some(Size::Px { .. }));
    let fixed_b = matches!(b.size, Some(Size::Px { .. }));
    if fixed_a { children[index].size = Some(Size::Px { px: new_a }); }
    if fixed_b { children[index + 1].size = Some(Size::Px { px: total - new_a }); }
    if !fixed_a || !fixed_b {
        // Weights are grow factors normalized over ALL siblings, not just the
        // pair. Solve against the shared layout calculation, including clamps.
        let weight = |n: &Node| super::pane_size::weight(n);
        let sum: f32 = children.iter().map(weight).sum();
        if !sum.is_finite() || sum <= 0.0 { return None; }
        let variable = if fixed_a { index + 1 } else { index };
        let target = expected[variable];
        let mut low = 0.0;
        let mut high = if !fixed_a && !fixed_b {
            weight(&children[index]) + weight(&children[index + 1])
        } else {
            // A mixed pair can require changing its weighted child's share
            // relative to every other weighted sibling.
            (sum * available / 24.0).max(sum)
        };
        if !high.is_finite() { return None; }
        let pair_sum = high;
        for _ in 0..64 {
            let mid = low + (high - low) / 2.0;
            children[variable].size = Some(Size::Weight { weight: mid });
            if !fixed_a && !fixed_b {
                children[index + 1].size = Some(Size::Weight { weight: pair_sum - mid });
            }
            let actual = measure(&children);
            if (actual[variable] - target).abs() <= 0.0001 { break; }
            if actual[variable] < target { low = mid; } else { high = mid; }
        }
    }
    let actual = measure(&children);
    if actual.iter().zip(&expected).any(|(a, b)| !a.is_finite() || (a - b).abs() > 0.001) { return None; }
    Some(children.iter().map(|n| n.size).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::pane;

    fn row(children: Vec<Node>) -> Node { Node::new("row", Kind::Row { children }) }

    fn resized(root: &Node, index: usize, available: f32, delta: f32) -> Vec<f32> {
        let updates = sizes(root, index, available, delta).expect("representable resize");
        let mut children: Vec<_> = root.children().iter().filter(|n| !n.hidden).cloned().collect();
        for (child, size) in children.iter_mut().zip(updates) { child.size = size; }
        super::super::pane_size::shares(&children.iter().collect::<Vec<_>>(), available, 1.0)
    }

    fn close(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len());
        for (a, b) in actual.iter().zip(expected) { assert!((a - b).abs() <= 0.001, "{actual:?} != {expected:?}"); }
    }

    #[test]
    fn divider_direction_is_independent_of_focused_side() {
        let root = row(vec![pane("a", "a"), pane("b", "b")]);
        for target in ["a", "b"] {
            let (_, a, b) = pair(&root, target, Axis::Row).unwrap();
            assert_eq!((&*a.id, &*b.id), ("a", "b"));
            close(&resized(&root, 0, 200.0, 10.0), &[110.0, 90.0]);
            close(&resized(&root, 0, 200.0, -10.0), &[90.0, 110.0]);
        }
        assert!(pair(&root, "a", Axis::Column).is_none());
    }

    #[test]
    fn middle_and_last_children_share_the_last_divider() {
        let root = row(vec![pane("a", "a"), pane("b", "b"), pane("c", "c")]);
        for target in ["b", "c"] {
            let (_, a, b) = pair(&root, target, Axis::Row).unwrap();
            assert_eq!((&*a.id, &*b.id), ("b", "c"));
        }
    }

    #[test]
    fn matching_disabled_split_is_a_boundary() {
        let mut inner = row(vec![pane("a", "a"), pane("b", "b")]);
        inner.id = "inner".into();
        inner.resizable = Some(false);
        let root = row(vec![inner, pane("c", "c")]);
        assert!(pair(&root, "a", Axis::Row).is_none());
    }

    #[test]
    fn constraints_on_both_children_limit_movement() {
        let mut a = pane("a", "a");
        let mut b = pane("b", "b");
        a.min = Some(80.0);
        b.min = Some(90.0);
        a.size = Some(Size::Px { px: 100.0 });
        b.size = Some(Size::Px { px: 100.0 });
        let root = row(vec![a.clone(), b.clone()]);
        close(&resized(&root, 0, 200.0, 50.0), &[110.0, 90.0]);
        close(&resized(&root, 0, 200.0, -50.0), &[80.0, 120.0]);
        b.max = Some(105.0);
        close(&resized(&row(vec![a, b]), 0, 200.0, -50.0), &[95.0, 105.0]);
    }

    #[test]
    fn mixed_sizes_preserve_the_unrelated_weighted_sibling() {
        for fixed_first in [true, false] {
            let mut a = pane("a", "a");
            let mut b = pane("b", "b");
            if fixed_first { a.size = Some(Size::Px { px: 100.0 }); }
            else { b.size = Some(Size::Px { px: 100.0 }); }
            let root = row(vec![a, b, pane("c", "c")]);
            for delta in [-10.0, 10.0] {
                close(&resized(&root, 0, 300.0, delta), &[100.0 + delta, 100.0 - delta, 100.0]);
                assert_eq!(sizes(&root, 0, 300.0, delta).unwrap()[2], None);
            }
        }
    }

    #[test]
    fn constrained_weights_are_checked_against_layout() {
        let mut a = pane("a", "a");
        a.min = Some(150.0);
        let root = row(vec![a, pane("b", "b")]);
        close(&resized(&root, 0, 200.0, 10.0), &[160.0, 40.0]);

        // This max constraint leaves unused flex space. Shrinking the capped
        // pane cannot preserve that unused space AND the pair's weight sum.
        // Reject rather than returning weights that move the wrong divider.
        let mut a = pane("a", "a");
        a.max = Some(100.0);
        a.size = Some(Size::Weight { weight: 3.0 });
        let root = row(vec![a, pane("b", "b")]);
        assert!(sizes(&root, 0, 200.0, -10.0).is_none());
    }

    #[test]
    fn repeats_use_updated_sizes_and_preserve_hidden_siblings() {
        let mut a = pane("a", "a");
        a.size = Some(Size::Px { px: 100.0 });
        let mut hidden = pane("hidden", "hidden");
        hidden.hidden = true;
        hidden.size = Some(Size::Px { px: 999.0 });
        let mut root = row(vec![a, hidden.clone(), pane("b", "b"), pane("c", "c")]);
        for delta in [10.0, 10.0, -10.0, -10.0] {
            let updates = sizes(&root, 0, 300.0, delta).unwrap();
            for (child, size) in root.children_mut().unwrap().iter_mut().filter(|n| !n.hidden).zip(updates) {
                child.size = size;
            }
        }
        let children: Vec<_> = root.children().iter().filter(|n| !n.hidden).collect();
        close(&super::super::pane_size::shares(&children, 300.0, 1.0), &[100.0, 100.0, 100.0]);
        assert_eq!(root.find("hidden"), Some(&hidden));
    }

    #[test]
    fn invalid_or_immovable_geometry_is_a_noop() {
        let root = row(vec![pane("a", "a"), pane("b", "b")]);
        for (available, delta) in [(40.0, 10.0), (200.0, 0.0), (f32::NAN, 10.0), (200.0, f32::INFINITY)] {
            assert!(sizes(&root, 0, available, delta).is_none());
        }
        assert!(sizes(&root, 1, 200.0, 10.0).is_none());
        assert!(sizes(&root, usize::MAX, 200.0, 10.0).is_none());
    }
}
