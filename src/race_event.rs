use serde::Serialize;

use crate::battle_state::SlopeInfo;

/// All narrative events emitted by the engine.
///
/// `lap` and `session_time` are included in every variant so the serialised
/// output is self-contained. The `event_type` discriminator is injected by
/// serde's internally-tagged format.
#[derive(Debug, Serialize)]
#[serde(tag = "event_type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RaceEvent {
    // ── Lap-level (regression-driven) ────────────────────────────────────────
    Push {
        lap:           u8,
        session_time:  f32,
        car_ahead_idx: u8,
        slope_info:    SlopeInfo,
    },
    AttackSetup {
        lap:           u8,
        session_time:  f32,
        car_ahead_idx: u8,
        slope_info:    SlopeInfo,
    },
    DefendPush {
        lap:            u8,
        session_time:   f32,
        car_behind_idx: u8,
        slope_info:     SlopeInfo,
    },
    DefendAttack {
        lap:            u8,
        session_time:   f32,
        car_behind_idx: u8,
        slope_info:     SlopeInfo,
    },
    // ── Frame-level (gap threshold) ───────────────────────────────────────────
    CloseApproach {
        lap:               u8,
        session_time:      f32,
        car_ahead_idx:     u8,
        gap_s:             f32,
        car_race_position: u8,
    },
    PressureBehind {
        lap:               u8,
        session_time:      f32,
        car_behind_idx:    u8,
        gap_s:             f32,
        car_race_position: u8,
    },
    // ── Position / pit ────────────────────────────────────────────────────────
    LapComplete {
        lap:          u8,
        session_time: f32,
        lap_time_s:   Option<f32>,
        position:     u8,
        pit_frames:   u32,
    },
    Overtake {
        lap:              u8,
        session_time:     f32,
        position_from:    u8,
        position_to:      u8,
        positions_gained: u8,
    },
    PositionLost {
        lap:             u8,
        session_time:    f32,
        position_from:   u8,
        position_to:     u8,
        positions_lost:  u8,
    },
    PitEntry {
        lap:          u8,
        session_time: f32,
        position:     u8,
    },
    PitExit {
        lap:          u8,
        session_time: f32,
        position:     u8,
    },
}
