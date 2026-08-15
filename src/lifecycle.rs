//! Publisher lifecycle events: PUBLISHER_HELLO / PUBLISHER_HEARTBEAT /
//! PUBLISHER_GOODBYE.
//!
//! [`LifecyclePublisher`] is called from the main publisher loop to generate
//! lifecycle [`RaceEvent`]s that are enqueued into the same [`PublisherTransport`]
//! as narrative events.

use std::time::{Duration, Instant};

use crate::race_event::RaceEvent;

/// Manages publisher lifecycle events.
///
/// Create one instance at startup, call [`on_activate`] after successful
/// token acquisition, and [`on_deactivate`] on clean shutdown.
pub struct LifecyclePublisher {
    version: String,
    /// `true` until `on_activate` is called for the first time.
    /// Used by the publisher loop to detect when a HELLO is still outstanding.
    fresh:          bool,
}

impl LifecyclePublisher {
    /// Create a lifecycle publisher.
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            fresh: true,
        }
    }

    /// `true` if `on_activate` has not yet been called on this instance.
    /// The publisher loop uses this to know when to emit `PUBLISHER_HELLO`.
    pub fn is_fresh(&self) -> bool {
        self.fresh
    }

    /// Emit `PUBLISHER_HELLO` — call once after successful registration.
    pub fn on_activate(&mut self, lap: u8, session_time: f32) -> RaceEvent {
        self.fresh = false;
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

    /// Emit `PUBLISHER_HEARTBEAT` — call on the heartbeat timer while connected.
    pub fn heartbeat(
        &self,
        lap: u8,
        session_time: f32,
        events_enqueued_total: u64,
    ) -> RaceEvent {
        RaceEvent::PublisherHeartbeat {
            lap,
            session_time,
            version: self.version.clone(),
            events_enqueued_total,
        }
    }
}

/// Wall-clock scheduler for `PUBLISHER_HEARTBEAT` emission.
///
/// An interval of `0` disables heartbeats entirely. The first heartbeat is
/// due one full interval after the first [`due`] call (HELLO covers startup).
pub struct HeartbeatScheduler {
    interval: Option<Duration>,
    last:     Option<Instant>,
}

impl HeartbeatScheduler {
    /// Create a scheduler firing every `interval_ms` milliseconds; `0` disables.
    pub fn new(interval_ms: u64) -> Self {
        Self {
            interval: (interval_ms > 0).then(|| Duration::from_millis(interval_ms)),
            last:     None,
        }
    }

    /// `true` when a heartbeat is due at `now`; advances the timer when it fires.
    pub fn due(&mut self, now: Instant) -> bool {
        let Some(interval) = self.interval else {
            return false;
        };
        match self.last {
            None => {
                self.last = Some(now);
                false
            }
            Some(last) if now.duration_since(last) >= interval => {
                self.last = Some(now);
                true
            }
            Some(_) => false,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_activate_emits_hello() {
        let mut lc = LifecyclePublisher::new("0.1.0");
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
    fn heartbeat_emits_heartbeat_event() {
        let lc = LifecyclePublisher::new("0.1.0");
        let event = lc.heartbeat(7, 321.5, 42);
        assert!(
            matches!(event, RaceEvent::PublisherHeartbeat { lap: 7, version, events_enqueued_total: 42, .. }
                if version == "0.1.0")
        );
    }

    #[test]
    fn heartbeat_scheduler_fires_after_interval() {
        let mut hb = HeartbeatScheduler::new(1000);
        let t0 = Instant::now();
        assert!(!hb.due(t0));
        assert!(!hb.due(t0 + Duration::from_millis(500)));
        assert!(hb.due(t0 + Duration::from_millis(1000)));
        assert!(!hb.due(t0 + Duration::from_millis(1500)));
        assert!(hb.due(t0 + Duration::from_millis(2000)));
    }

    #[test]
    fn heartbeat_scheduler_zero_interval_disables() {
        let mut hb = HeartbeatScheduler::new(0);
        let t0 = Instant::now();
        assert!(!hb.due(t0));
        assert!(!hb.due(t0 + Duration::from_secs(3600)));
    }

    #[test]
    fn is_fresh_is_true_before_activate() {
        let mut lc = LifecyclePublisher::new("0.1.0");
        assert!(lc.is_fresh());
        let _ = lc.on_activate(1, 0.0);
        assert!(!lc.is_fresh());
    }
}
