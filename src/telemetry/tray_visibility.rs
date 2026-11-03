//! Show requests are complete only after the current child confirms it is responsive.
use std::time::{Duration, Instant};

pub const SHOW_PROBE_KIND: u16 = 1;
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
pub struct Visibility {
    desired: bool,
    request: u64,
    probe: Option<(u64, u64)>,
    spawn_attempted: bool,
    stopped: bool,
    since: Option<Instant>,
}

impl Visibility {
    pub fn show(&mut self, now: Instant) {
        if !self.stopped {
            self.desired = true;
            self.request = self.request.wrapping_add(1);
            self.probe = None;
            self.spawn_attempted = false;
            self.since = Some(now);
        }
    }

    pub fn hide(&mut self) {
        self.desired = false;
        self.probe = None;
        self.since = None;
    }

    pub fn stop(&mut self) {
        self.hide();
        self.stopped = true;
    }

    pub fn wants_child(&self) -> bool {
        self.desired && !self.stopped
    }

    /// A request may start at most one replacement, avoiding a crash respawn loop.
    pub fn claim_spawn(&mut self) -> bool {
        if self.wants_child() && !self.spawn_attempted {
            self.spawn_attempted = true;
            true
        } else {
            false
        }
    }

    pub fn started(&mut self, now: Instant) {
        if self.wants_child() {
            self.since = Some(now);
        }
    }

    pub fn probe_needed(&self) -> Option<u64> {
        (self.wants_child() && self.probe.is_none()).then_some(self.request)
    }

    pub fn probe_sent(&mut self, generation: u64, request: u64) {
        if self.wants_child() && self.request == request {
            self.probe = Some((generation, request));
        }
    }

    pub fn confirmed(&mut self, generation: u64, request: u64) {
        if self
            .probe
            .is_some_and(|(g, r)| (g, r) == (generation, request))
        {
            self.desired = false;
            self.probe = None;
            self.since = None;
        }
    }

    pub fn disconnected(&mut self, generation: u64) {
        if self.probe.is_some_and(|(g, _)| g == generation) {
            self.probe = None;
        }
    }

    pub fn timed_out(&self, now: Instant) -> bool {
        self.since
            .is_some_and(|since| now.saturating_duration_since(since) >= PROBE_TIMEOUT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_during_close_survives_disconnect_and_stale_eof() {
        let now = Instant::now();
        let mut ui = Visibility::default();
        ui.show(now);
        let first = ui.probe_needed().unwrap();
        ui.probe_sent(4, first);
        ui.disconnected(3);
        assert_eq!(ui.probe_needed(), None);
        ui.disconnected(4);
        assert!(ui.wants_child());
        let next = ui.probe_needed().unwrap();
        ui.probe_sent(5, next);
        ui.confirmed(4, first);
        assert!(ui.wants_child());
        ui.confirmed(5, next);
        assert!(!ui.wants_child());
    }

    #[test]
    fn repeated_show_needs_latest_confirmation_and_hide_or_exit_cancels() {
        let now = Instant::now();
        let mut ui = Visibility::default();
        ui.show(now);
        let first = ui.probe_needed().unwrap();
        ui.probe_sent(1, first);
        ui.show(now);
        let latest = ui.probe_needed().unwrap();
        ui.probe_sent(1, latest);
        ui.confirmed(1, first);
        assert!(ui.wants_child());
        ui.hide();
        ui.disconnected(1);
        assert!(!ui.wants_child());
        ui.show(now);
        ui.stop();
        ui.show(now);
        assert!(!ui.wants_child());
    }

    #[test]
    fn unresponsive_child_gets_a_bounded_probe_deadline() {
        let now = Instant::now();
        let mut ui = Visibility::default();
        ui.show(now);
        assert!(ui.timed_out(now + PROBE_TIMEOUT)); // Even a full queue cannot stall forever.
        ui.probe_sent(9, ui.probe_needed().unwrap());
        assert!(!ui.timed_out(now + PROBE_TIMEOUT - Duration::from_millis(1)));
        assert!(ui.timed_out(now + PROBE_TIMEOUT));
        ui.disconnected(9);
        assert!(ui.wants_child());
        ui.started(now + PROBE_TIMEOUT);
        assert!(!ui.timed_out(now + PROBE_TIMEOUT));
    }

    #[test]
    fn each_show_allows_only_one_spawn_and_exit_disables_it() {
        let now = Instant::now();
        let mut ui = Visibility::default();
        ui.show(now);
        assert!(ui.claim_spawn());
        assert!(!ui.claim_spawn());
        ui.show(now);
        assert!(ui.claim_spawn());
        ui.stop();
        assert!(!ui.claim_spawn());
    }
}
