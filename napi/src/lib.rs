#![deny(clippy::all)]

mod irsdk;

use std::collections::HashMap;

use napi_derive::napi;

use director_narrative_core::{
    engine::NarrativeEngine as CoreEngine,
    telemetry_frame::TelemetryFrame as CoreFrame,
    race_event::RaceEvent as CoreEvent,
};

// ── Input type ───────────────────────────────────────────────────────────────

/// Telemetry frame received from the Node.js host. Field names are camelCase
/// on the JS side (napi-rs applies camelCase conversion automatically).
#[napi(object)]
pub struct TelemetryFrame {
    pub lap:                    u32,
    pub session_time:           f64,
    pub lap_dist_pct:           f64,
    pub player_car_idx:         u32,
    pub player_car_position:    u32,
    pub on_pit_road:            bool,
    pub session_flags:          u32,
    /// Per-car lap distance percentage, indexed by car_idx.
    pub car_idx_lap_dist_pct:   Vec<f64>,
    /// Per-car race position (0 = inactive), indexed by car_idx.
    pub car_idx_position:       Vec<u32>,
    /// Per-car pit-road flag, indexed by car_idx.
    pub car_idx_on_pit_road:    Vec<bool>,
}

// ── Output type ──────────────────────────────────────────────────────────────

/// A narrative event emitted by the engine.
///
/// `narrativeContext` is a JSON object whose keys vary by `eventType`:
/// - `PUSH` / `ATTACK_SETUP`: `carAheadIdx`, `slopeInfo`
/// - `DEFEND_PUSH` / `DEFEND_ATTACK`: `carBehindIdx`, `slopeInfo`
/// - `CLOSE_APPROACH`: `carAheadIdx`, `gapS`, `carRacePosition`
/// - `PRESSURE_BEHIND`: `carBehindIdx`, `gapS`, `carRacePosition`
/// - `LAP_COMPLETE`: `lapTimeS`, `position`, `pitFrames`
/// - `OVERTAKE`: `positionFrom`, `positionTo`, `positionsGained`
/// - `POSITION_LOST`: `positionFrom`, `positionTo`, `positionsLost`
/// - `PIT_ENTRY` / `PIT_EXIT`: `position`
#[napi(object)]
pub struct RaceEvent {
    pub event_type:       String,
    pub lap:              u32,
    pub session_time:     f64,
    pub narrative_context: serde_json::Value,
}

// ── NarrativeEngine class ────────────────────────────────────────────────────

#[napi]
pub struct NarrativeEngine {
    inner: CoreEngine,
    /// Active live session. Only used on Windows; always `None` on other platforms.
    live_session: Option<irsdk::thread::LiveSession>,
}

#[napi]
impl NarrativeEngine {
    /// Create a new engine.
    ///
    /// `anchorCount` is the number of spatial buckets, derived from the lap-1
    /// duration pre-pass: `Math.max(10, Math.floor(lap1DurationS / 5.0))`.
    /// Use 108 as the fallback for live Nürburgring sessions.
    #[napi(constructor)]
    pub fn new(anchor_count: u32) -> Self {
        NarrativeEngine {
            inner: CoreEngine::new(anchor_count as usize),
            live_session: None,
        }
    }

    /// Feed one telemetry frame and return any narrative events it triggers.
    ///
    /// Used by the JSONL / CI batch path. Not called in live mode.
    #[napi]
    pub fn process_frame(&mut self, frame: TelemetryFrame) -> Vec<RaceEvent> {
        let core_frame = into_core_frame(frame);
        self.inner
            .process_frame(&core_frame)
            .into_iter()
            .map(into_js_event)
            .collect()
    }

    /// Start live iRacing telemetry ingestion.
    ///
    /// On Windows: spawns a background thread that connects to the iRacing
    /// shared-memory file, waits on `IRSDKDataValidEvent` at 60 Hz, and calls
    /// `callback` with any narrative events produced each frame.
    ///
    /// On non-Windows platforms: returns an error immediately.
    ///
    /// `callback` signature: `(events: RaceEvent[]) => void`
    #[napi]
    pub fn start_live(
        &mut self,
        callback: napi::JsFunction,
    ) -> napi::Result<()> {
        #[cfg(target_os = "windows")]
        {
            use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction};
            use irsdk::thread::{LiveSession, DEFAULT_ANCHOR_COUNT};

            let tsfn: ThreadsafeFunction<Vec<RaceEvent>, ErrorStrategy::Fatal> =
                callback.create_threadsafe_function(0, |ctx| Ok(vec![ctx.value]))?;

            self.live_session = Some(LiveSession::spawn(DEFAULT_ANCHOR_COUNT, tsfn));
            Ok(())
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = callback;
            Err(napi::Error::from_reason(
                "startLive is only supported on Windows (iRacing platform)",
            ))
        }
    }

    /// Stop the live iRacing background thread.
    ///
    /// Sets the shutdown flag and blocks until the thread exits cleanly.
    /// No-op if `startLive` was not called or already stopped.
    #[napi]
    pub fn stop_live(&mut self) {
        if let Some(mut session) = self.live_session.take() {
            session.stop();
        }
    }
}

// ── Conversion helpers ───────────────────────────────────────────────────────

fn into_core_frame(js: TelemetryFrame) -> CoreFrame {
    CoreFrame {
        lap:                  js.lap as u8,
        session_time:         js.session_time as f32,
        lap_dist_pct:         js.lap_dist_pct as f32,
        player_car_idx:       js.player_car_idx as u8,
        player_car_position:  js.player_car_position as u8,
        on_pit_road:          js.on_pit_road,
        session_flags:        js.session_flags,
        car_idx_lap_dist_pct: js.car_idx_lap_dist_pct.iter().map(|&x| x as f32).collect(),
        car_idx_position:     js.car_idx_position.iter().map(|&x| x as u8).collect(),
        car_idx_on_pit_road:  js.car_idx_on_pit_road,
        lap_last_lap_time:    0.0,  // not available from the JS batch API
    }
}

pub(crate) fn into_js_event(event: CoreEvent) -> RaceEvent {
    // Serialise the enum to { event_type, lap, session_time, ...fields }.
    let mut obj = match serde_json::to_value(&event) {
        Ok(serde_json::Value::Object(m)) => m,
        _ => Default::default(),
    };

    let event_type = obj
        .remove("event_type")
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let lap = obj
        .remove("lap")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let session_time = obj
        .remove("session_time")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    // Remaining fields → camelCase keys in narrativeContext (recursive)
    let narrative_context: HashMap<String, serde_json::Value> = obj
        .into_iter()
        .map(|(k, v)| (snake_to_camel(&k), camel_keys(v)))
        .collect();

    RaceEvent {
        event_type,
        lap,
        session_time,
        narrative_context: serde_json::to_value(narrative_context)
            .unwrap_or(serde_json::Value::Object(Default::default())),
    }
}

/// Recursively convert all object keys in a JSON value to camelCase.
fn camel_keys(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let converted = map
                .into_iter()
                .map(|(k, val)| (snake_to_camel(&k), camel_keys(val)))
                .collect();
            serde_json::Value::Object(converted)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(camel_keys).collect())
        }
        other => other,
    }
}

fn snake_to_camel(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut up = false;
    for c in s.chars() {
        if c == '_' {
            up = true;
        } else if up {
            out.push(c.to_ascii_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}
