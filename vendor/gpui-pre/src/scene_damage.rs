//! Diffs consecutive scenes to find the region of the window that changed, so a
//! renderer can redraw and present only that region.

use crate::{Bounds, PaintOperation, Primitive, ScaledPixels, Scene};
use std::ops::Range;

/// How far ahead the diff looks for a matching operation after a mismatch, so
/// that inserted or removed operations only damage themselves.
const RESYNC_WINDOW: usize = 32;

/// The region of the window that changed between two presented frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneDamage {
    /// Everything must be redrawn.
    Full,
    /// Only this region changed, in the scene's scaled-pixel coordinate space.
    Rect(Bounds<ScaledPixels>),
    /// Nothing changed, so presenting can be skipped.
    Unchanged,
}

impl SceneDamage {
    /// Computes the region where `next` renders differently from `prev`. May
    /// over-report, but never misses a change.
    pub fn between(prev: &Scene, next: &Scene) -> Self {
        let prev = &prev.paint_operations;
        let next = &next.paint_operations;

        let common = prev.len().min(next.len());
        let prefix = (0..common)
            .take_while(|&index| operation_eq(&prev[index], &next[index]))
            .count();
        let suffix = (0..common - prefix)
            .take_while(|&offset| {
                operation_eq(
                    &prev[prev.len() - 1 - offset],
                    &next[next.len() - 1 - offset],
                )
            })
            .count();

        let mut damage = Bounds::default();
        let mut prev_index = prefix;
        let mut next_index = prefix;
        let prev_end = prev.len() - suffix;
        let next_end = next.len() - suffix;
        while prev_index < prev_end && next_index < next_end {
            if operation_eq(&prev[prev_index], &next[next_index]) {
                prev_index += 1;
                next_index += 1;
                continue;
            }
            match resync(&prev[prev_index..prev_end], &next[next_index..next_end]) {
                Resync::Removed(count) => {
                    damage_operations(prev, prev_index..prev_index + count, &mut damage);
                    prev_index += count;
                }
                Resync::Inserted(count) => {
                    damage_operations(next, next_index..next_index + count, &mut damage);
                    next_index += count;
                }
                Resync::Replaced => {
                    damage_operations(prev, prev_index..prev_index + 1, &mut damage);
                    damage_operations(next, next_index..next_index + 1, &mut damage);
                    prev_index += 1;
                    next_index += 1;
                }
            }
        }
        damage_operations(prev, prev_index..prev_end, &mut damage);
        damage_operations(next, next_index..next_end, &mut damage);

        if damage.is_empty() {
            Self::Unchanged
        } else {
            Self::Rect(damage)
        }
    }

    /// Combines two damage regions into one that covers both.
    pub fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Full, _) | (_, Self::Full) => Self::Full,
            (Self::Unchanged, damage) | (damage, Self::Unchanged) => damage,
            (Self::Rect(a), Self::Rect(b)) => Self::Rect(join_bounds(&a, &b)),
        }
    }

    /// Adds `bounds` to the damage, ignoring empty bounds.
    pub fn with_bounds(self, bounds: Bounds<ScaledPixels>) -> Self {
        if bounds.is_empty() {
            self
        } else {
            self.union(Self::Rect(bounds))
        }
    }

    /// Whether the whole window must be redrawn.
    pub fn is_full(&self) -> bool {
        matches!(self, Self::Full)
    }

    /// Whether nothing needs to be redrawn.
    pub fn is_unchanged(&self) -> bool {
        matches!(self, Self::Unchanged)
    }

    /// The damaged rectangle, when the damage is not full or empty.
    pub fn bounds(&self) -> Option<Bounds<ScaledPixels>> {
        match self {
            Self::Full | Self::Unchanged => None,
            Self::Rect(bounds) => Some(*bounds),
        }
    }
}

enum Resync {
    /// This many operations at the start of `prev` have no counterpart in `next`.
    Removed(usize),
    /// This many operations at the start of `next` have no counterpart in `prev`.
    Inserted(usize),
    /// The first operations differ, and no nearby match was found.
    Replaced,
}

fn resync(prev: &[PaintOperation], next: &[PaintOperation]) -> Resync {
    for distance in 1..=RESYNC_WINDOW {
        if prev
            .get(distance)
            .is_some_and(|operation| operation_eq(operation, &next[0]))
        {
            return Resync::Removed(distance);
        }
        if next
            .get(distance)
            .is_some_and(|operation| operation_eq(&prev[0], operation))
        {
            return Resync::Inserted(distance);
        }
    }
    Resync::Replaced
}

/// Damages the area of the operations in `range`. A changed layer marker changes
/// the draw order within its layer, so it damages the whole layer.
fn damage_operations(
    operations: &[PaintOperation],
    range: Range<usize>,
    damage: &mut Bounds<ScaledPixels>,
) {
    for index in range {
        let layer = match &operations[index] {
            PaintOperation::Primitive(primitive) => {
                *damage = join_bounds(damage, &primitive.clipped_bounds());
                continue;
            }
            PaintOperation::StartLayer(bounds) => {
                *damage = join_bounds(damage, bounds);
                layer_after(operations, index)
            }
            PaintOperation::EndLayer => layer_before(operations, index),
        };
        for operation in &operations[layer] {
            if let PaintOperation::Primitive(primitive) = operation {
                *damage = join_bounds(damage, &primitive.clipped_bounds());
            }
        }
    }
}

/// The operations of the layer that starts at `start`.
fn layer_after(operations: &[PaintOperation], start: usize) -> Range<usize> {
    let mut depth = 0usize;
    for (index, operation) in operations.iter().enumerate().skip(start) {
        match operation {
            PaintOperation::StartLayer(_) => depth += 1,
            PaintOperation::EndLayer => {
                depth -= 1;
                if depth == 0 {
                    return start..index;
                }
            }
            PaintOperation::Primitive(_) => {}
        }
    }
    start..operations.len()
}

/// The operations of the layer that ends at `end`.
fn layer_before(operations: &[PaintOperation], end: usize) -> Range<usize> {
    let mut depth = 0usize;
    for index in (0..=end).rev() {
        match operations[index] {
            PaintOperation::EndLayer => depth += 1,
            PaintOperation::StartLayer(_) => {
                depth -= 1;
                if depth == 0 {
                    return index..end;
                }
            }
            PaintOperation::Primitive(_) => {}
        }
    }
    0..end
}

fn operation_eq(left: &PaintOperation, right: &PaintOperation) -> bool {
    match (left, right) {
        (PaintOperation::Primitive(left), PaintOperation::Primitive(right)) => left == right,
        (PaintOperation::StartLayer(left), PaintOperation::StartLayer(right)) => left == right,
        (PaintOperation::EndLayer, PaintOperation::EndLayer) => true,
        _ => false,
    }
}

/// Empty-safe union. `Bounds::union` treats empty default bounds as a rectangle
/// at the origin, which would incorrectly pull damage toward `(0, 0)`.
fn join_bounds(left: &Bounds<ScaledPixels>, right: &Bounds<ScaledPixels>) -> Bounds<ScaledPixels> {
    if left.is_empty() {
        *right
    } else if right.is_empty() {
        *left
    } else {
        left.union(right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentMask, Hsla, Point, Quad, Size, hsla, point, size};

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
        Bounds {
            origin: point(ScaledPixels(x), ScaledPixels(y)),
            size: size(ScaledPixels(width), ScaledPixels(height)),
        }
    }

    fn quad(bounds: Bounds<ScaledPixels>, lightness: f32) -> Quad {
        Quad {
            bounds,
            content_mask: ContentMask {
                bounds: rect(-1000., -1000., 2000., 2000.),
            },
            background: Hsla {
                h: 0.,
                s: 0.,
                l: lightness,
                a: 1.,
            }
            .into(),
            ..Default::default()
        }
    }

    enum Op {
        Quad(Quad),
        Layer(Bounds<ScaledPixels>, Vec<Quad>),
    }

    fn scene_of(operations: &[Op]) -> Scene {
        let mut scene = Scene::default();
        for operation in operations {
            match operation {
                Op::Quad(quad) => scene.insert_primitive(*quad),
                Op::Layer(bounds, quads) => {
                    scene.push_layer(*bounds);
                    for quad in quads {
                        scene.insert_primitive(*quad);
                    }
                    scene.pop_layer();
                }
            }
        }
        scene.finish();
        scene
    }

    fn quads(quads: &[Quad]) -> Scene {
        scene_of(&quads.iter().copied().map(Op::Quad).collect::<Vec<_>>())
    }

    #[test]
    fn identical_scenes_are_unchanged() {
        let a = quads(&[quad(rect(0., 0., 100., 100.), 0.5)]);
        let b = quads(&[quad(rect(0., 0., 100., 100.), 0.5)]);
        assert_eq!(SceneDamage::between(&a, &b), SceneDamage::Unchanged);
    }

    #[test]
    fn changed_quad_damages_its_bounds() {
        let unchanged = quad(rect(0., 0., 100., 100.), 0.1);
        let before = quads(&[unchanged, quad(rect(200., 200., 10., 20.), 0.5)]);
        let after = quads(&[unchanged, quad(rect(200., 200., 10., 20.), 0.9)]);
        assert_eq!(
            SceneDamage::between(&before, &after),
            SceneDamage::Rect(rect(200., 200., 10., 20.))
        );
    }

    #[test]
    fn inserted_and_removed_quads_damage_only_themselves() {
        let a = quad(rect(0., 0., 50., 50.), 0.1);
        let cursor = quad(rect(60., 0., 2., 20.), 0.5);
        let c = quad(rect(100., 0., 50., 50.), 0.9);
        let without = quads(&[a, c]);
        let with = quads(&[a, cursor, c]);
        let cursor_damage = SceneDamage::Rect(rect(60., 0., 2., 20.));
        assert_eq!(SceneDamage::between(&without, &with), cursor_damage);
        assert_eq!(SceneDamage::between(&with, &without), cursor_damage);
    }

    #[test]
    fn moved_highlight_damages_only_the_rows_between() {
        let rows: Vec<Quad> = (0..10)
            .map(|row| quad(rect(0., row as f32 * 20., 100., 20.), 0.2))
            .collect();
        let highlight = |row: usize| quad(rect(0., row as f32 * 20., 100., 20.), 0.8);
        let with_highlight_at = |row: usize| {
            let mut scene_quads = rows.clone();
            scene_quads.insert(row + 1, highlight(row));
            quads(&scene_quads)
        };
        assert_eq!(
            SceneDamage::between(&with_highlight_at(1), &with_highlight_at(8)),
            SceneDamage::Rect(rect(0., 20., 100., 160.))
        );
    }

    #[test]
    fn changed_layer_damages_the_whole_layer() {
        let first = quad(rect(0., 0., 10., 10.), 0.1);
        let second = quad(rect(50., 0., 10., 10.), 0.1);
        let bounds = rect(0., 0., 60., 10.);
        let before = quads(&[first, second]);
        let after = scene_of(&[Op::Layer(bounds, vec![first, second])]);
        assert_eq!(
            SceneDamage::between(&before, &after),
            SceneDamage::Rect(bounds)
        );
    }

    #[test]
    fn union_combines_damage() {
        assert_eq!(
            SceneDamage::Full.union(SceneDamage::Rect(rect(0., 0., 1., 1.))),
            SceneDamage::Full
        );
        assert_eq!(
            SceneDamage::Unchanged.union(SceneDamage::Unchanged),
            SceneDamage::Unchanged
        );
        assert_eq!(
            SceneDamage::Rect(rect(0., 0., 10., 10.))
                .union(SceneDamage::Rect(rect(20., 20., 10., 10.))),
            SceneDamage::Rect(rect(0., 0., 30., 30.))
        );
        assert_eq!(
            SceneDamage::Unchanged.with_bounds(Bounds::default()),
            SceneDamage::Unchanged
        );
    }
}
