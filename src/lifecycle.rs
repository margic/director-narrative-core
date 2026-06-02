//! Publisher lifecycle events: PUBLISHER_HELLO / PUBLISHER_GOODBYE.
//!
//! [`LifecyclePublisher`] is called from the main publisher loop to generate
//! lifecycle [`RaceEvent`]s that are enqueued into the same [`PublisherTransport`]
//! as narrative events.

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
    fn is_fresh_is_true_before_activate() {
        let mut lc = LifecyclePublisher::new("0.1.0");
        assert!(lc.is_fresh());
        let _ = lc.on_activate(1, 0.0);
        assert!(!lc.is_fresh());
    }
}
