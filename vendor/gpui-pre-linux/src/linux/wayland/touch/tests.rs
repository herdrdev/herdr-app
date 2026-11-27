use super::*;
use gpui::{point, px};

fn position(x: f32, y: f32) -> Point<Pixels> {
    point(px(x), px(y))
}

fn down(id: i32, surface: u32) -> TouchUpdate<u32> {
    TouchUpdate::Down {
        id,
        surface,
        position: position(23.25, 110.5),
    }
}

fn dispatch_frame(state: &mut TouchState<u32>) -> Vec<(u32, TouchEvent)> {
    state
        .take_frame()
        .into_iter()
        .filter_map(|update| state.apply(update))
        .collect()
}

#[test]
fn frame_preserves_release_position_and_allocates_a_new_id_on_reuse() {
    let mut state = TouchState::new();
    let last_position = position(24.5, 120.25);
    state.push(down(0, 12));
    state.push(TouchUpdate::Motion {
        id: 0,
        position: last_position,
    });
    state.push(TouchUpdate::Up { id: 0 });
    state.push(down(0, 18));

    let frame = dispatch_frame(&mut state);
    assert_eq!(
        frame
            .iter()
            .map(|(surface, event)| (*surface, event.phase))
            .collect::<Vec<_>>(),
        [
            (12, TouchPhase::Started),
            (12, TouchPhase::Moved),
            (12, TouchPhase::Ended),
            (18, TouchPhase::Started),
        ],
    );
    assert_eq!(frame[0].1.position, position(23.25, 110.5));
    assert_eq!(frame[1].1.position, last_position);
    assert_eq!(frame[2].1.position, last_position);
    assert_eq!(frame[0].1.id, frame[1].1.id);
    assert_eq!(frame[0].1.id, frame[2].1.id);
    assert_ne!(frame[2].1.id, frame[3].1.id);
    assert!(dispatch_frame(&mut state).is_empty());
}

#[test]
fn a_release_without_a_frame_finishes_before_the_next_touch_frame() {
    let mut state = TouchState::new();
    state.push(down(1, 12));
    let started = dispatch_frame(&mut state);

    state.push(TouchUpdate::Up { id: 1 });
    state.push(down(1, 18));
    let fallback: Vec<_> = state
        .take_unframed_releases()
        .into_iter()
        .filter_map(|update| state.apply(update))
        .collect();

    assert_eq!(fallback.len(), 1);
    assert_eq!(fallback[0].0, 12);
    assert_eq!(fallback[0].1.id, started[0].1.id);
    assert_eq!(fallback[0].1.phase, TouchPhase::Ended);
    assert!(state.take_unframed_releases().is_empty());

    let next = dispatch_frame(&mut state);
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].0, 18);
    assert_eq!(next[0].1.phase, TouchPhase::Started);
}

#[test]
fn cancellation_discards_pending_updates_and_finalizes_delivered_contacts() {
    let mut state = TouchState::new();
    state.push(down(0, 12));
    state.push(down(1, 18));
    let started = dispatch_frame(&mut state);
    state.push(TouchUpdate::Up { id: 0 });
    state.push(TouchUpdate::Motion {
        id: 1,
        position: position(24.5, 120.25),
    });
    state.push(down(2, 18));

    let mut cancelled: Vec<_> = state.cancel().collect();
    cancelled.sort_by_key(|(_, event)| event.id);
    assert_eq!(cancelled.len(), 2);
    for ((surface, event), (original_surface, began)) in cancelled.iter().zip(&started) {
        assert_eq!(surface, original_surface);
        assert_eq!(event.id, began.id);
        assert_eq!(event.position, began.position);
        assert_eq!(event.phase, TouchPhase::Cancelled);
    }
    assert!(dispatch_frame(&mut state).is_empty());
    assert!(state.take_unframed_releases().is_empty());
    assert!(state.apply(TouchUpdate::Up { id: 1 }).is_none());
    let (_, restarted) = state.apply(down(0, 12)).unwrap();
    assert_ne!(restarted.id, started[0].1.id);
}

#[test]
fn cancellation_before_first_frame_delivers_no_contact() {
    let mut state = TouchState::new();
    state.push(down(0, 12));
    state.push(TouchUpdate::Up { id: 0 });

    assert_eq!(state.cancel().count(), 0);
    assert!(dispatch_frame(&mut state).is_empty());
}

#[test]
fn closing_one_surface_preserves_other_contacts_and_discards_its_queued_downs() {
    let mut state = TouchState::new();
    state.push(down(1, 12));
    state.push(down(-3, 18));
    let started = dispatch_frame(&mut state);
    state.push(TouchUpdate::Up { id: 1 });
    state.push(down(2, 12));
    state.push(TouchUpdate::Up { id: -3 });

    state.forget_surface(&12);

    let frame = dispatch_frame(&mut state);
    assert_eq!(frame.len(), 1);
    assert_eq!(frame[0].0, 18);
    assert_eq!(frame[0].1.id, started[1].1.id);
    assert_eq!(frame[0].1.phase, TouchPhase::Ended);
    assert_eq!(state.cancel().count(), 0);
}

#[test]
fn unknown_updates_and_duplicate_down_do_not_replace_the_contact() {
    let mut state = TouchState::new();
    assert!(
        state
            .apply(TouchUpdate::Motion {
                id: 99,
                position: position(50., 60.),
            })
            .is_none()
    );
    assert!(state.apply(TouchUpdate::Up { id: 99 }).is_none());
    let (_, start) = state.apply(down(99, 12)).unwrap();
    assert!(state.apply(down(99, 18)).is_none());
    let (surface, end) = state.apply(TouchUpdate::Up { id: 99 }).unwrap();
    assert_eq!(surface, 12);
    assert_eq!(end.id, start.id);
    assert_eq!(end.position, start.position);
}

#[test]
#[ignore = "run explicitly to measure Wayland touch adapter throughput"]
fn benchmark_five_contact_frames() {
    use std::{hint::black_box, time::Instant};

    const CONTACTS: i32 = 5;
    const FRAMES: usize = 200_000;

    let mut state = TouchState::new();
    for id in 0..CONTACTS {
        state.push(down(id, 12));
    }
    assert_eq!(dispatch_frame(&mut state).len(), CONTACTS as usize);

    let run = |state: &mut TouchState<u32>, frames| {
        let mut delivered = 0;
        for frame in 0..frames {
            for id in 0..CONTACTS {
                state.push(TouchUpdate::Motion {
                    id,
                    position: position(id as f32 * 12.5, (frame % 200) as f32),
                });
            }
            for update in state.take_frame() {
                delivered += usize::from(black_box(state.apply(update)).is_some());
            }
        }
        assert_eq!(delivered, frames * CONTACTS as usize);
    };

    run(&mut state, 2_000);
    let started = Instant::now();
    run(&mut state, FRAMES);
    let elapsed = started.elapsed();
    eprintln!(
        "{FRAMES} five-contact frames in {elapsed:?} ({:.0} ns/frame)",
        elapsed.as_nanos() as f64 / FRAMES as f64,
    );
}
