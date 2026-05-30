//! Publisher lifecycle events: PUBLISHER_HELLO / PUBLISHER_HEARTBEAT / PUBLISHER_GOODBYE.
//!
//! [`LifecyclePublisher`] is called from the main publisher loop to generate
//! lifecycle [`RaceEvent`]s that are enqueued into the same [`PublisherTransport`]
//! as narrative events.

use std::time::{Duration, Instant};

use crate::race_event::RaceEvent;

const HEARTBEAT_INTERVAL_S: u64 = 30;

/// Manages publisher lifecycle events.
///
/// Create one instance at startup, call [`on_activate`] after successful
/// token acquisition, [`tick`] every engine loop iteration, and
/// [`on_deactivate`] on clean shutdown.
pub struct LifecyclePublisher {
    version:        String,
    last_heartbeat: Instant,
    interval:       Duration,
}

impl LifecyclePublisher {
    /// Create with the default 30-second heartbeat interval.
    pub fn new(version: impl Into<String>) -> Self {
        Self::with_interval(version, Duration::from_secs(HEARTBEAT_INTERVAL_S))
    }

    fn with_interval(version: impl Into<String>, interval: Duration) -> Self {
        Self {
            version:        version.into(),
            last_heartbeat: Instant::now(),
            interval,
        }
    }

    /// Emit `PUBLISHER_HELLO` — call once after successful registration.
    pub fn on_activate(&self, lap: u8, session_time: f32) -> RaceEvent {
        RaceEvent::PublisherHello {
            lap,
            session_time,
            version: self.version.clone(),
            scope:   "driver".to_owned(),
        }
    }

    /// Emit `PUBLISHER_GOODBYE` — call on clean shutdown.
    pub fn on_deactivate(&self, lap: u8, session_time: f32) -> RaceEvent {
        RaceEvent::PublisherGoodbye { lap, session_time }
    }

    /// Check elapsed time and return `Some(PUBLISHER_HEARTBEAT)` if the
    /// heartbeat interval has elapsed; otherwise `None`.
    ///
    /// Call once per engine loop iteration (every ~16ms at 60 Hz).
    pub fn tick(&mut self, lap: u8, session_time: f32) -> Option<RaceEvent> {
        if self.last_heartbeat.elapsed() >= self.interval {
            self.last_heartbeat = Instant::now();
            Some(RaceEvent::PublisherHeartbeat { lap, session_time })
        } else {
            None
        }
    }

    /// Back-date the last heartbeat timestamp for test purposes.
    #[cfg(test)]
    pub fn set_last_heartbeat_elapsed_for_test(&mut self, elapsed: Duration) {
        self.last_heartbeat = Instant::now()
            .checked_sub(elapsed)
            .expect("elapsed must not overflow Instant");
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_activate_emits_hello() {
        let lc = LifecyclePublisher::new("0.1.0");
        let event = lc.on_activate(1, 10.0);
        assert!(
            matches!(event, RaceEvent::PublisherHello { version, scope, .. }
                if version == "0.1.0" && scope == "driver")
        );
    }

    #[test]
    fn on_deactivate_emits_goodbye() {
        let lc = LifecyclePublisher::new("0.1.0");
        let event = lc.on_deactivate(5, 250.0);
        assert!(matches!(event, RaceEvent::PublisherGoodbye { lap: 5, .. }));
    }

    #[test]
    fn tick_returns_none_before_interval() {
        let mut lc = LifecyclePublisher::new("0.1.0");
        // No time has passed — heartbeat should not fire
        assert!(lc.tick(1, 0.0).is_none());
    }

    #[test]
    fn tick_emits_heartbeat_after_interval() {
        let mut lc = LifecyclePublisher::new("0.1.0");
        // Simulate 31 seconds elapsed
        lc.set_last_heartbeat_elapsed_for_test(Duration::from_secs(31));
        let event = lc.tick(3, 310.0);
        assert!(
            matches!(event, Some(RaceEvent::PublisherHeartbeat { lap: 3, .. })),
            "expected PUBLISHER_HEARTBEAT after 31s elapsed"
        );
    }

    #[test]
    fn tick_resets_timer_after_heartbeat() {
        let mut lc = LifecyclePublisher::new("0.1.0");
        lc.set_last_heartbeat_elapsed_for_test(Duration::from_secs(31));
        lc.tick(3, 310.0); // fires heartbeat and resets timer
        // Immediately after reset, should not fire again
        assert!(lc.tick(3, 310.1).is_none());
    }
}
