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
    // ── Battle / gap ─────────────────────────────────────────────────────────
    /// Gap to a nearby car is below the battle threshold.
    /// Fires from lap 1 — no OLS regression required.
    BattleEngaged {
        lap:               u8,
        session_time:      f32,
        /// Opponent car index (ahead or behind the player).
        car_idx:           u8,
        gap_s:             f32,
        car_race_position: u8,
    },
    /// Gap that previously triggered `BATTLE_ENGAGED` has widened beyond the threshold.
    BattleBroken {
        lap:          u8,
        session_time: f32,
        /// Opponent car index that was the subject of the prior `BATTLE_ENGAGED`.
        car_idx:      u8,
        gap_s:        f32,
    },
    /// OLS regression confirms a sustained closing rate to a nearby opponent.
    /// Covers both attacker (closing on car ahead) and defender (car behind closing)
    /// perspectives; the `car_idx` field identifies the opponent.
    BattleClosing {
        lap:                      u8,
        session_time:             f32,
        car_idx:                  u8,
        /// Closing rate in seconds per lap (positive = closing, from |median_slope|).
        closing_rate_sec_per_lap: f32,
        slope_info:               SlopeInfo,
    },
    // ── Session / flag ────────────────────────────────────────────────────────
    /// `SessionState` transitioned to Racing (4) and the green flag bit is set.
    RaceGreen {
        lap:          u8,
        session_time: f32,
    },
    /// Full-course yellow flag (Caution) became active.
    FlagYellowFullCourse {
        lap:          u8,
        session_time: f32,
    },
    /// Local (sector) yellow flag became active.
    FlagYellowLocal {
        lap:          u8,
        session_time: f32,
    },
    /// `SessionState` transitioned to Checkered (5).
    RaceCheckered {
        lap:          u8,
        session_time: f32,
    },
    // ── Position ──────────────────────────────────────────────────────────────
    /// Player gained at least one position at a lap crossing (non-pit lap).
    Overtake {
        lap:              u8,
        session_time:     f32,
        position_from:    u8,
        position_to:      u8,
        positions_gained: u8,
    },
    /// Player gained the lead at a lap crossing.
    OvertakeForLead {
        lap:              u8,
        session_time:     f32,
        position_from:    u8,
        positions_gained: u8,
    },
    // ── Lap / pit ─────────────────────────────────────────────────────────────
    LapCompleted {
        lap:           u8,
        session_time:  f32,
        lap_time_s:    Option<f32>,
        best_lap_time_s: Option<f32>,
        position:      u8,
        pit_frames:    u32,
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
