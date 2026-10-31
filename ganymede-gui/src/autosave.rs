//! Debounced autosave; a failed attempt pauses retries until explicitly resumed.
use std::time::{Duration, Instant};

pub const DELAY: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
pub struct AutosavePreference(pub bool);
impl Default for AutosavePreference {
    fn default() -> Self {
        Self(true)
    }
}
impl gpui::Global for AutosavePreference {}

#[derive(Default)]
pub struct AutosaveClock {
    revision: Option<u64>,
    changed_at: Option<Instant>,
    paused: bool,
}
impl AutosaveClock {
    pub fn resume(&mut self) {
        self.paused = false;
        self.changed_at = None;
    }
    pub fn failed(&mut self) {
        self.paused = true;
    }
    pub fn due(
        &mut self,
        revision: u64,
        dirty: bool,
        enabled: bool,
        saving: bool,
        now: Instant,
    ) -> bool {
        if self.revision != Some(revision) || self.changed_at.is_none() {
            self.revision = Some(revision);
            self.changed_at = Some(now);
        }
        enabled
            && dirty
            && !saving
            && !self.paused
            && now.duration_since(self.changed_at.unwrap()) >= DELAY
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_and_outputs_restart_the_quiet_period() {
        let mut clock = AutosaveClock::default();
        let now = Instant::now();
        assert!(!clock.due(1, true, true, false, now));
        assert!(!clock.due(2, true, true, false, now + DELAY));
        assert!(!clock.due(2, true, true, false, now + DELAY + Duration::from_secs(1)));
        assert!(clock.due(2, true, true, false, now + DELAY * 2));
        assert!(!clock.due(2, false, true, false, now + DELAY * 3));
    }
    #[test]
    fn disabled_inflight_and_failed_saves_do_not_retry() {
        let mut clock = AutosaveClock::default();
        let now = Instant::now();
        clock.due(1, true, true, false, now);
        assert!(!clock.due(1, true, false, false, now + DELAY));
        assert!(!clock.due(1, true, true, true, now + DELAY));
        clock.failed();
        assert!(!clock.due(2, true, true, false, now + DELAY));
        assert!(!clock.due(2, true, true, false, now + DELAY * 2));
        clock.resume();
        assert!(!clock.due(2, true, true, false, now + DELAY * 2));
        assert!(clock.due(2, true, true, false, now + DELAY * 3));
    }
}
