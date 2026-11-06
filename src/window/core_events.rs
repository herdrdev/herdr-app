//! Wake for core events; keep idle maintenance and machine checks on their own clocks.

use crate::hc::{EventLines, Pushed, Stream};
use gpui::{BackgroundExecutor, Task};
use std::future::{Future, poll_fn};
use std::pin::{Pin, pin};
use std::task::Poll;
use std::time::{Duration, Instant};

pub const IDLE_TICK: Duration = Duration::from_millis(500);
const MACHINES_EVERY: Duration = Duration::from_millis(2500);

async fn next(events: &async_channel::Receiver<Stream>, timeout: &mut Task<()>) -> Option<Stream> {
    let mut receive = pin!(events.recv());
    poll_fn(|cx| {
        // Give queued data priority when the event and the deadline are both ready.
        if let Poll::Ready(event) = receive.as_mut().poll(cx) {
            return Poll::Ready(Some(event.unwrap_or(Stream::Lost)));
        }
        if Pin::new(&mut *timeout).poll(cx).is_ready() { Poll::Ready(None) } else { Poll::Pending }
    }).await
}

/// State/attention and layout changes wake the UI immediately. Titles and progress
/// still batch at the old idle cadence, with credits returned as bytes are read.
pub async fn batch(
    events: &async_channel::Receiver<Stream>, lines: &mut EventLines, executor: &BackgroundExecutor,
    force: bool, mut credit: impl FnMut(usize),
) -> (Pushed, bool) {
    let deadline = executor.now() + IDLE_TICK;
    let mut timeout = None;
    let mut first = None;
    let (mut pushed, mut closed) = (Pushed::default(), false);
    loop {
        // A busy producer cannot prevent checking the deadline or a lifecycle change.
        for event in first.take().into_iter().chain(std::iter::from_fn(|| events.try_recv().ok())).take(256) {
            match event {
                Stream::Data(bytes) => {
                    let got = lines.push(&bytes);
                    pushed.docs |= got.docs;
                    pushed.terminals |= got.terminals;
                    pushed.urgent |= got.urgent;
                    credit(bytes.len());
                }
                _ => closed = true,
            }
        }
        if force || closed || pushed.docs || pushed.urgent || executor.now() >= deadline { return (pushed, closed); }
        // One timer for the batch, not a new task for every cosmetic event.
        let timeout = timeout.get_or_insert_with(|| executor.timer(deadline.saturating_duration_since(executor.now())));
        first = next(events, timeout).await;
        if first.is_none() { return (pushed, closed); }
    }
}

pub fn machines_due(next: &mut Instant, now: Instant) -> bool {
    if now < *next { return false; }
    *next = now + MACHINES_EVERY;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[gpui::test]
    async fn a_short_working_turn_wakes_the_reader_before_the_idle_tick(cx: &mut gpui::TestAppContext) {
        let executor = cx.executor();
        let sender = executor.clone();
        let (tx, events) = async_channel::unbounded();
        let task = executor.spawn(async move {
            for (seq, state) in [(1, "working"), (2, "idle")] {
                sender.timer(Duration::from_millis(100)).await;
                let event = json!({"method": "event", "params": {"seq": seq, "type": "status", "terminal": "t1", "data": {"status": {"state": state}}}});
                tx.send(Stream::Data(format!("{event}\n").into_bytes())).await.unwrap();
            }
        });
        let mut lines = crate::hc::EventLines::new(&json!({"seq": 0, "terminals": [{"address": "t1", "state": "live", "status": {"state": "idle"}}]}));
        for state in ["working", "idle"] {
            let before = executor.now();
            let (pushed, closed) = batch(&events, &mut lines, &executor, false, |_| {}).await;
            assert_eq!(executor.now() - before, Duration::from_millis(100));
            assert!(pushed.terminals && pushed.urgent && !closed);
            assert_eq!(lines.live()["terminals"][0]["status"]["state"], state);
        }
        task.await;
    }

    #[gpui::test]
    async fn idle_maintenance_times_out_without_losing_the_next_event(cx: &mut gpui::TestAppContext) {
        let executor = cx.executor();
        let (tx, events) = async_channel::unbounded();
        let before = executor.now();
        let mut timeout = executor.timer(IDLE_TICK);
        assert!(next(&events, &mut timeout).await.is_none());
        assert_eq!(executor.now() - before, IDLE_TICK);
        tx.try_send(Stream::Data(vec![1])).unwrap();
        let before = executor.now();
        assert!(matches!(next(&events, &mut timeout).await, Some(Stream::Data(bytes)) if bytes == [1]));
        drop(tx);
        assert!(matches!(next(&events, &mut timeout).await, Some(Stream::Lost)));
        assert_eq!(executor.now(), before, "queued data and a closed stream wake immediately");
    }

    #[gpui::test]
    async fn cosmetic_updates_keep_their_batch_deadline_and_return_all_credits(cx: &mut gpui::TestAppContext) {
        let executor = cx.executor();
        let sender = executor.clone();
        let (tx, events) = async_channel::unbounded();
        let task = executor.spawn(async move {
            let mut total = 0;
            for seq in 1..=4 {
                sender.timer(Duration::from_millis(100)).await;
                let event = json!({"method": "event", "params": {"seq": seq, "type": "status", "terminal": "t1", "data": {"status": {"state": "working", "title": seq.to_string()}}}});
                let bytes = format!("{event}\n").into_bytes();
                total += bytes.len();
                tx.send(Stream::Data(bytes)).await.unwrap();
            }
            // Keep the stream live beyond the batching deadline.
            sender.timer(Duration::from_secs(1)).await;
            total
        });
        let mut lines = EventLines::new(&json!({"seq": 0, "terminals": [{"address": "t1", "state": "live", "status": {"state": "working"}}]}));
        let before = executor.now();
        let mut credited = 0;
        let (pushed, closed) = batch(&events, &mut lines, &executor, false, |n| credited += n).await;
        assert_eq!(executor.now() - before, IDLE_TICK, "cosmetic traffic cannot postpone maintenance");
        assert!(pushed.terminals && !pushed.urgent && !closed);
        assert_eq!(lines.live()["terminals"][0]["status"]["title"], "4");
        assert_eq!(credited, task.await);
    }

    #[gpui::test]
    async fn a_state_change_interrupts_a_shared_cosmetic_deadline(cx: &mut gpui::TestAppContext) {
        let executor = cx.executor();
        let sender = executor.clone();
        let (tx, events) = async_channel::unbounded();
        let task = executor.spawn(async move {
            for (seq, state) in [(1, "working"), (2, "working"), (3, "blocked")] {
                sender.timer(Duration::from_millis(50)).await;
                let event = json!({"method": "event", "params": {"seq": seq, "type": "status", "terminal": "t1", "data": {"status": {"state": state, "title": seq.to_string()}}}});
                tx.send(Stream::Data(format!("{event}\n").into_bytes())).await.unwrap();
            }
        });
        let mut lines = EventLines::new(&json!({"seq": 0, "terminals": [{"address": "t1", "state": "live", "status": {"state": "working"}}]}));
        let before = executor.now();
        let (pushed, closed) = batch(&events, &mut lines, &executor, false, |_| {}).await;
        assert_eq!(executor.now() - before, Duration::from_millis(150));
        assert!(pushed.urgent && !closed);
        assert_eq!(lines.live()["terminals"][0]["status"]["state"], "blocked");
        task.await;
    }

    #[test]
    fn frequent_events_do_not_speed_up_or_postpone_machine_checks() {
        let start = Instant::now();
        let mut next = start;
        assert!(machines_due(&mut next, start));
        for milliseconds in (100..2500).step_by(100) {
            assert!(!machines_due(&mut next, start + Duration::from_millis(milliseconds)));
        }
        assert!(machines_due(&mut next, start + MACHINES_EVERY));
        assert!(!machines_due(&mut next, start + MACHINES_EVERY));
        assert!(machines_due(&mut next, start + MACHINES_EVERY * 2));
    }
}
