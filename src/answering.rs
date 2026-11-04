//! Whether a terminal on another machine is answering. Its keys go over a
//! line that a weak network can stall for many seconds: the pane then says so,
//! instead of looking like typing went nowhere, and scrolling (which a
//! full-screen program gets as keys) stops piling up input to replay later.
//!
//! Silence alone isn't a stall: a password prompt, or a key a program
//! ignores, answers nothing too. After `QUIET` without output, one round trip
//! to the machine (a probe) tells the two apart: it comes back at once on a
//! working line and waits with everything else on a stalled one.

use std::time::{Duration, Instant};

/// Input this long without output gets a probe.
pub const QUIET: Duration = Duration::from_millis(1000);
/// A probe out this long means the line is stalled.
const PROBE_SLOW: Duration = Duration::from_millis(1000);
/// Normal latency: scrolls before this go through as they come.
const SCROLL_FREE: Duration = Duration::from_millis(500);
/// While unanswered, at most one scroll per this long goes out.
const SCROLL_EVERY: Duration = Duration::from_millis(1000);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Probe {
    None,
    Asked(Instant),
    /// The machine answered: the program is quiet, the line works.
    Answered,
    /// No reply before its timeout: ask again.
    Failed(Instant),
}

#[derive(Debug, PartialEq)]
pub struct Stall {
    /// Since the first input that hasn't been answered.
    pub waited: Duration,
    /// Inputs sent since the last output (keys, pastes, scrolls).
    pub sent: usize,
}

#[derive(Debug)]
pub struct Answering {
    /// The first input with no output since, and how many sends.
    unanswered: Option<(Instant, usize)>,
    probe: Probe,
    /// The last scroll that went out while unanswered.
    scrolled: Option<Instant>,
    /// Counts unanswered stretches: a probe's reply for an older one is old news.
    episode: u64,
}

impl Default for Answering {
    fn default() -> Self {
        Answering { unanswered: None, probe: Probe::None, scrolled: None, episode: 0 }
    }
}

impl Answering {
    /// Input went out. True when this starts a new unanswered stretch (the
    /// caller then watches it).
    pub fn input(&mut self, now: Instant) -> bool {
        // The machine answered a probe for the earlier input: this one is
        // measured on its own (the line may have stalled since).
        if self.probe == Probe::Answered {
            self.unanswered = None;
        }
        match &mut self.unanswered {
            Some((_, sent)) => {
                *sent += 1;
                false
            }
            None => {
                self.unanswered = Some((now, 1));
                self.probe = Probe::None;
                self.episode += 1;
                true
            }
        }
    }

    /// Output came back: whatever was waiting was answered.
    pub fn output(&mut self) {
        self.unanswered = None;
        self.probe = Probe::None;
        self.scrolled = None;
    }

    /// The line was replaced (reattached): nothing is waiting on the old one.
    pub fn reset(&mut self) {
        self.output();
        self.episode += 1;
    }

    pub fn episode(&self) -> u64 {
        self.episode
    }

    pub fn waiting(&self) -> bool {
        self.unanswered.is_some()
    }

    /// The machine answered: only the program is quiet, nothing to watch until more input.
    pub fn settled(&self) -> bool {
        self.probe == Probe::Answered
    }

    /// Whether a scroll may go out now (it is then counted as input).
    /// A program that hasn't answered gets one a second, not dozens to
    /// replay all at once when the line comes back.
    pub fn may_scroll(&mut self, now: Instant) -> bool {
        let free = match self.unanswered {
            None => true,
            Some((since, _)) => now.duration_since(since) < SCROLL_FREE || self.probe == Probe::Answered,
        };
        if !free && self.scrolled.is_some_and(|at| now.duration_since(at) < SCROLL_EVERY) {
            return false;
        }
        if !free {
            self.scrolled = Some(now);
        }
        true
    }

    /// The episode to probe now, if one is due (marked asked).
    pub fn probe_due(&mut self, now: Instant) -> Option<u64> {
        let (since, _) = self.unanswered?;
        let due = match self.probe {
            Probe::None => now.duration_since(since) >= QUIET,
            Probe::Failed(at) => now.duration_since(at) >= QUIET,
            Probe::Asked(_) | Probe::Answered => false,
        };
        due.then(|| {
            self.probe = Probe::Asked(now);
            self.episode
        })
    }

    /// A probe for `episode` came back (`answered`) or timed out.
    pub fn probed(&mut self, episode: u64, answered: bool, now: Instant) {
        if episode != self.episode || self.unanswered.is_none() {
            return;
        }
        self.probe = if answered { Probe::Answered } else { Probe::Failed(now) };
    }

    /// The line looks stalled: input waits and the machine hasn't answered a probe in time.
    pub fn stall(&self, now: Instant) -> Option<Stall> {
        let (since, sent) = self.unanswered?;
        let stalled = match self.probe {
            Probe::Asked(at) => now.duration_since(at) >= PROBE_SLOW,
            Probe::Failed(_) => true,
            Probe::None | Probe::Answered => false,
        };
        stalled.then(|| Stall { waited: now.duration_since(since), sent })
    }
}

/// "6s", "1m 05s": how long, for a pane's notice.
pub fn short_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 { format!("{s}s") } else if s < 3600 { format!("{}m {:02}s", s / 60, s % 60) } else { format!("{}h {:02}m", s / 3600, (s / 60) % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn a_quiet_program_on_a_working_line_is_not_a_stall() {
        let t0 = Instant::now();
        let mut a = Answering::default();
        assert!(a.input(t0), "the first unanswered key starts a stretch");
        assert!(!a.input(at(t0, 100)));
        assert_eq!(a.probe_due(at(t0, 500)), None, "not before QUIET");
        let ep = a.probe_due(at(t0, 1000)).unwrap();
        assert_eq!(a.probe_due(at(t0, 1100)), None, "one probe at a time");
        a.probed(ep, true, at(t0, 1050));
        assert_eq!(a.stall(at(t0, 5000)), None, "the machine answered: only the program is quiet");
    }

    #[test]
    fn a_probe_that_waits_shows_a_stall_until_output_arrives() {
        let t0 = Instant::now();
        let mut a = Answering::default();
        a.input(t0);
        a.input(at(t0, 200));
        a.input(at(t0, 300));
        a.probe_due(at(t0, 1000)).unwrap();
        assert_eq!(a.stall(at(t0, 1500)), None, "a slow reply isn't a stall yet");
        assert_eq!(a.stall(at(t0, 2000)), Some(Stall { waited: Duration::from_secs(2), sent: 3 }));
        a.output();
        assert_eq!(a.stall(at(t0, 2500)), None);
        assert!(!a.waiting());
    }

    #[test]
    fn a_failed_probe_stays_a_stall_and_is_asked_again() {
        let t0 = Instant::now();
        let mut a = Answering::default();
        a.input(t0);
        let ep = a.probe_due(at(t0, 1000)).unwrap();
        a.probed(ep, false, at(t0, 11_000));
        assert!(a.stall(at(t0, 11_000)).is_some());
        assert_eq!(a.probe_due(at(t0, 11_500)), None);
        assert_eq!(a.probe_due(at(t0, 12_000)), Some(ep));
    }

    #[test]
    fn a_reply_to_an_older_stretch_changes_nothing() {
        let t0 = Instant::now();
        let mut a = Answering::default();
        a.input(t0);
        let old = a.probe_due(at(t0, 1000)).unwrap();
        a.output();
        a.input(at(t0, 2000));
        a.probed(old, true, at(t0, 2100));
        let new = a.probe_due(at(t0, 3000)).unwrap();
        assert_ne!(old, new);
        assert!(a.stall(at(t0, 4000)).is_some(), "the old reply must not vouch for the new stretch");
        a.reset();
        a.probed(new, true, at(t0, 4100));
        assert!(!a.waiting());
    }

    #[test]
    fn scrolls_into_an_unanswering_program_go_out_once_a_second() {
        let t0 = Instant::now();
        let mut a = Answering::default();
        assert!(a.may_scroll(t0));
        a.input(t0);
        assert!(a.may_scroll(at(t0, 100)), "normal latency: as they come");
        a.input(at(t0, 100));
        assert!(a.may_scroll(at(t0, 600)), "the first unanswered scroll goes");
        assert!(!a.may_scroll(at(t0, 700)));
        assert!(!a.may_scroll(at(t0, 1500)));
        assert!(a.may_scroll(at(t0, 1600)));
        a.output();
        assert!(a.may_scroll(at(t0, 1700)), "answered: free again");
    }

    #[test]
    fn scrolls_stay_free_when_the_machine_answers_but_the_program_is_quiet() {
        let t0 = Instant::now();
        let mut a = Answering::default();
        a.input(t0);
        let ep = a.probe_due(at(t0, 1000)).unwrap();
        a.probed(ep, true, at(t0, 1050));
        assert!(a.may_scroll(at(t0, 1100)));
        assert!(a.may_scroll(at(t0, 1150)));
    }

    #[test]
    fn input_after_a_quiet_answer_is_watched_afresh() {
        let t0 = Instant::now();
        let mut a = Answering::default();
        a.input(t0);
        let ep = a.probe_due(at(t0, 1000)).unwrap();
        a.probed(ep, true, at(t0, 1050));
        assert!(a.settled());
        assert!(a.input(at(t0, 5000)), "a new stretch, with its own probe");
        assert!(!a.settled());
        assert_eq!(a.probe_due(at(t0, 5500)), None);
        a.probe_due(at(t0, 6000)).unwrap();
        assert_eq!(a.stall(at(t0, 7000)), Some(Stall { waited: Duration::from_secs(2), sent: 1 }), "timed from the new input, not the old");
    }

    #[test]
    fn durations_read_short() {
        assert_eq!(short_duration(Duration::from_millis(6400)), "6s");
        assert_eq!(short_duration(Duration::from_secs(65)), "1m 05s");
        assert_eq!(short_duration(Duration::from_secs(3720)), "1h 02m");
    }
}
