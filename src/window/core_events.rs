//! Wake for core events; keep idle maintenance and machine checks on their own clocks.

use crate::hc::Stream;
use gpui::{BackgroundExecutor, FutureExt};
use std::time::{Duration, Instant};

pub const IDLE_TICK: Duration = Duration::from_millis(500);
const MACHINES_EVERY: Duration = Duration::from_millis(2500);

pub async fn next(events: &async_channel::Receiver<Stream>, executor: &BackgroundExecutor) -> Option<Stream> {
    match events.recv().with_timeout(IDLE_TICK, executor).await {
        Ok(Ok(event)) => Some(event),
        Ok(Err(_)) => Some(Stream::Lost),
        Err(_) => None,
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
            let Some(Stream::Data(bytes)) = next(&events, &executor).await else { panic!("missing status event") };
            assert_eq!(executor.now() - before, Duration::from_millis(100));
            assert!(lines.push(&bytes).terminals);
            assert_eq!(lines.live()["terminals"][0]["status"]["state"], state);
        }
        task.await;
    }

    #[gpui::test]
    async fn idle_maintenance_times_out_without_losing_the_next_event(cx: &mut gpui::TestAppContext) {
        let executor = cx.executor();
        let (tx, events) = async_channel::unbounded();
        let before = executor.now();
        assert!(next(&events, &executor).await.is_none());
        assert_eq!(executor.now() - before, IDLE_TICK);
        tx.try_send(Stream::Data(vec![1])).unwrap();
        let before = executor.now();
        assert!(matches!(next(&events, &executor).await, Some(Stream::Data(bytes)) if bytes == [1]));
        drop(tx);
        assert!(matches!(next(&events, &executor).await, Some(Stream::Lost)));
        assert_eq!(executor.now(), before, "queued data and a closed stream wake immediately");
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
