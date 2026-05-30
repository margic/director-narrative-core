//! Shared state between the publisher pipeline thread and the UI thread.

use std::collections::VecDeque;
use std::time::SystemTime;

/// Maximum number of entries retained in the event log.
pub const EVENT_LOG_CAPACITY: usize = 50;

/// Written by the publisher pipeline thread; read by the UI render loop.
///
/// Wrapped in `Arc<Mutex<PublisherStatus>>`. The pipeline holds the lock
/// for a few microseconds per frame update; the UI holds it for a single
/// paint pass (~16ms at 60fps). No long-held locks.
#[derive(Default)]
pub struct PublisherStatus {
    // ── iRacing connection ────────────────────────────────────────────────
    pub iracing_connected:  bool,
    pub sub_session_id:     Option<i64>,
    pub track_name:         Option<String>,
    pub session_type:       Option<String>,
    /// "unlimited" or a lap count string.
    pub session_laps:       Option<String>,
    pub current_lap:        u8,
    pub session_tick:       i64,
    pub session_time_secs:  f64,

    // ── Race Control / transport ──────────────────────────────────────────
    pub rc_last_http_status: Option<u16>,
    pub rc_connected:        bool,
    pub token_expires_at:    Option<SystemTime>,

    // ── Counters ──────────────────────────────────────────────────────────
    pub events_enqueued_total: u64,
    pub calls_total:           u64,
    pub calls_failed:          u64,

    // ── Event log ─────────────────────────────────────────────────────────
    pub event_log: VecDeque<EventLogEntry>,

    // ── Config ────────────────────────────────────────────────────────────
    pub config_path: Option<String>,
}

/// One row in the rolling event log panel.
#[derive(Clone)]
pub struct EventLogEntry {
    pub session_time:  f64,
    pub event_type:    String,
    pub car_number:    String,
    pub driver_name:   String,
}

impl PublisherStatus {
    /// Append an entry to the event log, evicting the oldest if at capacity.
    pub fn push_event_log(&mut self, entry: EventLogEntry) {
        if self.event_log.len() >= EVENT_LOG_CAPACITY {
            self.event_log.pop_back();
        }
        self.event_log.push_front(entry);
    }
}
