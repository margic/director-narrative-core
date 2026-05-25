use std::collections::{HashMap, HashSet};

use crate::anchor_sampler::AnchorSampler;
use crate::battle_state::BattleState;
use crate::lap_timer::LapTimer;
use crate::race_event::RaceEvent;
use crate::regression_store::RegressionStore;
use crate::telemetry_frame::TelemetryFrame;

/// Pure state-machine narrative engine. A single instance owns all race state.
///
/// Ownership strategy (issue #5):
/// - All fields are owned values — no `Arc`, `Mutex`, or lifetime parameters.
/// - `TelemetryFrame` is accepted by shared reference per call and never retained.
/// - `AnchorSampler` and `RegressionStore` are exclusive to this struct; the
///   `_behind` pair handles the defensive (car-behind-player) direction.
/// - Frame-carry-over fields (`prev_lap`, `prev_on_pit`, `prev_position`) are
///   plain `Option`/`bool` — overwritten each call, never references.
#[allow(dead_code)] // fields populated in new(); read access added in issues #6, #7, #9
pub struct NarrativeEngine {
    // Race-scoped — live for the entire session
    lap_timer:         LapTimer,
    sampler:           AnchorSampler,     // forward: cars ahead of player
    sampler_behind:    AnchorSampler,     // defensive: cars behind player
    regression:        RegressionStore,
    regression_behind: RegressionStore,
    engine_state:      BattleState,
    defensive_state:   BattleState,
    prev_slope:        Option<f32>,       // most recent forward threat median slope
    prev_slope_beh:    Option<f32>,       // most recent defensive threat median slope
    pit_laps:          HashSet<u8>,
    lap_end_positions: HashMap<u8, u8>,
    lap_pit_frames:    HashMap<u8, u32>,
    // Frame-level carry-over (overwritten each call, plain fields not references)
    prev_lap:          Option<u8>,
    prev_on_pit:       bool,
    prev_position:     Option<u8>,
}

impl NarrativeEngine {
    /// Create a new engine. `anchor_count` is derived from lap 1 duration:
    /// `floor(lap1_duration_s / TARGET_CADENCE_S)`, minimum 10.
    pub fn new(anchor_count: usize) -> Self {
        NarrativeEngine {
            lap_timer:         LapTimer,
            sampler:           AnchorSampler::new(anchor_count),
            sampler_behind:    AnchorSampler::new(anchor_count),
            regression:        RegressionStore::new(),
            regression_behind: RegressionStore::new(),
            engine_state:      BattleState::Idle,
            defensive_state:   BattleState::Idle,
            prev_slope:        None,
            prev_slope_beh:    None,
            pit_laps:          HashSet::new(),
            lap_end_positions: HashMap::new(),
            lap_pit_frames:    HashMap::new(),
            prev_lap:          None,
            prev_on_pit:       false,
            prev_position:     None,
        }
    }

    /// Process one telemetry frame and return any narrative events it triggers.
    /// Full implementation in issues #6, #7, and #9.
    pub fn process_frame(&mut self, _frame: &TelemetryFrame) -> Vec<RaceEvent> {
        Vec::new()
    }
}
