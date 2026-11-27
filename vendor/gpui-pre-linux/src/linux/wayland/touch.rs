// HERDR: translate frame-batched Wayland contacts to GPUI's touch lifecycle.
use collections::HashMap;
use gpui::{Pixels, Point, TouchEvent, TouchId, TouchPhase};
use smallvec::SmallVec;
use std::{collections::hash_map::Entry, mem};

pub(super) enum TouchUpdate<Surface> {
    Down {
        id: i32,
        surface: Surface,
        position: Point<Pixels>,
    },
    Motion {
        id: i32,
        position: Point<Pixels>,
    },
    Up {
        id: i32,
    },
}

pub(super) struct TouchState<Surface> {
    pending: SmallVec<[TouchUpdate<Surface>; 8]>,
    active: HashMap<i32, (Surface, TouchEvent)>,
    next_id: u64,
}

impl<Surface: Clone + Eq> TouchState<Surface> {
    pub(super) fn new() -> Self {
        Self {
            pending: SmallVec::new(),
            active: HashMap::default(),
            next_id: 0,
        }
    }

    pub(super) fn push(&mut self, update: TouchUpdate<Surface>) {
        self.pending.push(update);
    }

    pub(super) fn take_frame(&mut self) -> SmallVec<[TouchUpdate<Surface>; 8]> {
        mem::take(&mut self.pending)
    }

    pub(super) fn take_unframed_releases(&mut self) -> SmallVec<[TouchUpdate<Surface>; 8]> {
        // A late or duplicate up must not flush updates from an unrelated
        // contact that is still waiting for its frame.
        let Some(release) = (0..self.pending.len()).rev().find(|&index| {
            let TouchUpdate::Up { id } = self.pending[index] else {
                return false;
            };
            self.pending[..index]
                .iter()
                .rev()
                .find_map(|update| match *update {
                    TouchUpdate::Down { id: earlier, .. } if earlier == id => Some(true),
                    TouchUpdate::Up { id: earlier } if earlier == id => Some(false),
                    _ => None,
                })
                .unwrap_or_else(|| self.active.contains_key(&id))
        }) else {
            return SmallVec::new();
        };
        self.pending.drain(..=release).collect()
    }

    pub(super) fn apply(&mut self, update: TouchUpdate<Surface>) -> Option<(Surface, TouchEvent)> {
        match update {
            TouchUpdate::Down {
                id,
                surface,
                position,
            } => {
                let Entry::Vacant(slot) = self.active.entry(id) else {
                    return None;
                };
                // A Wayland ID can be reused as soon as that contact ends. GPUI
                // IDs must distinguish each lifetime, including across seat changes.
                self.next_id += 1;
                let event = TouchEvent {
                    id: TouchId(self.next_id),
                    phase: TouchPhase::Started,
                    position,
                    ..TouchEvent::default()
                };
                slot.insert((surface.clone(), event.clone()));
                Some((surface, event))
            }
            TouchUpdate::Motion { id, position } => {
                self.active.get_mut(&id).map(|(surface, event)| {
                    event.phase = TouchPhase::Moved;
                    event.position = position;
                    (surface.clone(), event.clone())
                })
            }
            TouchUpdate::Up { id } => self.active.remove(&id).map(|(surface, mut event)| {
                // Up carries no position or surface.
                event.phase = TouchPhase::Ended;
                (surface, event)
            }),
        }
    }

    pub(super) fn cancel(&mut self) -> impl Iterator<Item = (Surface, TouchEvent)> + '_ {
        // Cancel has no frame terminator. Uncommitted updates, including an
        // undecided release, must not turn into recognized taps or pans.
        self.pending.clear();
        self.active.drain().map(|(_, (surface, mut event))| {
            event.phase = TouchPhase::Cancelled;
            (surface, event)
        })
    }

    pub(super) fn forget_surface(&mut self, surface: &Surface) {
        self.active.retain(|_, (target, _)| target != surface);
        self.pending.retain(|update| {
            !matches!(update, TouchUpdate::Down { surface: target, .. } if target == surface)
        });
    }
}

#[cfg(test)]
mod tests;
