use std::collections::{HashMap, HashSet};

use crate::anchor_sampler::AnchorSampler;
use crate::battle_state::{
    classify, BattleState, CLOSE_APPROACH_MIN_FRAMES, CLOSE_APPROACH_THRESH_S,
    MIN_PUSH_READINGS, SCAN_FIELD_POSITIONS,
};
use crate::gap_finder::{find_cars_ahead, find_cars_behind};
use crate::lap_timer::LapTimer;
use crate::race_event::RaceEvent;
use crate::regression_store::RegressionStore;
use crate::telemetry_frame::TelemetryFrame;

// ── Constants ────────────────────────────────────────────────────────────────

/// A lap is flagged as a pit lap once this many frames were spent on pit road.
const PIT_LAP_FRAME_THRESH: u32 = 20;

const YELLOW_WAVE: u32 = 0x0100;
const CAUTION:     u32 = 0x4000;

/// Known Nürburgring yellow-flag zones: `(lap, ldp_start, ldp_end)`.
/// Samples inside these zones are marked dirty to avoid regression skew.
const YELLOW_ZONES: &[(u8, f32, f32)] = &[
    (1, 0.625, 0.646),
    (2, 0.616, 0.623),
];

// ── Engine ───────────────────────────────────────────────────────────────────

/// Pure state-machine narrative engine. A single instance owns all race state.
///
/// Ownership strategy (issue #5):
/// - All fields are owned values — no `Arc`, `Mutex`, or lifetime parameters.
/// - `TelemetryFrame` is accepted by shared reference per call and never retained.
/// - `AnchorSampler` and `RegressionStore` are exclusive to this struct; the
///   `_behind` pair handles the defensive (car-behind-player) direction.
/// - Frame-carry-over fields (`prev_lap`, `prev_on_pit`, `prev_position`) are
///   plain `Option`/`bool` — overwritten each call, never references.
pub struct NarrativeEngine {
    // Race-scoped — live for the entire session
    anchor_count:       usize,
    lap_timer:          LapTimer,
    sampler:            AnchorSampler,     // forward: cars ahead of player
    sampler_behind:     AnchorSampler,     // defensive: cars behind player
    regression:         RegressionStore,
    regression_behind:  RegressionStore,
    engine_state:       BattleState,
    defensive_state:    BattleState,
    prev_slope:         Option<f32>,
    prev_slope_beh:     Option<f32>,
    pit_laps:           HashSet<u8>,
    lap_end_positions:  HashMap<u8, u8>,
    lap_pit_frames:     HashMap<u8, u32>,
    // Close-approach tracking (forward + defensive)
    consecutive_close:      u32,
    last_close_t:           f32,
    tracking_car:           Option<u8>,
    consecutive_close_beh:  u32,
    last_close_beh_t:       f32,
    tracking_car_beh:       Option<u8>,
    // Frame-level carry-over (overwritten each call, plain fields not references)
    prev_lap:           Option<u8>,
    prev_on_pit:        bool,
    prev_position:      Option<u8>,
}

impl NarrativeEngine {
    /// Create a new engine. `anchor_count` is derived from lap 1 duration:
    /// `floor(lap1_duration_s / TARGET_CADENCE_S)`, minimum 10.
    pub fn new(anchor_count: usize) -> Self {
        NarrativeEngine {
            anchor_count,
            lap_timer:          LapTimer::new(),
            sampler:            AnchorSampler::new(anchor_count),
            sampler_behind:     AnchorSampler::new(anchor_count),
            regression:         RegressionStore::new(),
            regression_behind:  RegressionStore::new(),
            engine_state:       BattleState::Idle,
            defensive_state:    BattleState::Idle,
            prev_slope:         None,
            prev_slope_beh:     None,
            pit_laps:           HashSet::new(),
            lap_end_positions:  HashMap::new(),
            lap_pit_frames:     HashMap::new(),
            consecutive_close:      0,
            last_close_t:           f32::NEG_INFINITY,
            tracking_car:           None,
            consecutive_close_beh:  0,
            last_close_beh_t:       f32::NEG_INFINITY,
            tracking_car_beh:       None,
            prev_lap:           None,
            prev_on_pit:        false,
            prev_position:      None,
        }
    }

    /// Process one telemetry frame and return any narrative events it triggers.
    pub fn process_frame(&mut self, frame: &TelemetryFrame) -> Vec<RaceEvent> {
        let mut events = Vec::new();

        let lap   = frame.lap;
        let t     = frame.session_time;
        let pos   = frame.player_car_position;
        let ldp   = frame.lap_dist_pct;
        let on_pit = frame.on_pit_road;

        // Skip frames before the race starts
        if pos == 0 || lap < 1 {
            self.prev_lap      = Some(lap);
            self.prev_on_pit   = on_pit;
            return events;
        }

        // ── Lap timer ──────────────────────────────────────────────────────
        self.lap_timer.update(lap, t);
        let lap_t = self.lap_timer.best_estimate();

        // ── Cleanness flag ─────────────────────────────────────────────────
        let synth_flags = synthesize_flags(lap, ldp);
        let is_clean    = (synth_flags & (YELLOW_WAVE | CAUTION)) == 0 && !on_pit;

        // ── Pit-frame counter ──────────────────────────────────────────────
        if on_pit {
            *self.lap_pit_frames.entry(lap).or_insert(0) += 1;
        }

        // ── Gap scanning ───────────────────────────────────────────────────
        let cars_ahead  = find_cars_ahead(frame, lap_t, SCAN_FIELD_POSITIONS);
        let cars_behind = find_cars_behind(frame, lap_t, SCAN_FIELD_POSITIONS);

        for &(car_idx, gap_s) in &cars_ahead {
            self.sampler.update(lap, ldp, gap_s, car_idx, is_clean);
        }
        for &(car_idx, gap_s) in &cars_behind {
            self.sampler_behind.update(lap, ldp, gap_s, car_idx, is_clean);
        }

        let nearest_ahead  = cars_ahead.first().copied();
        let nearest_behind = cars_behind.first().copied();

        // ── Pit entry / exit ───────────────────────────────────────────────
        if on_pit && !self.prev_on_pit {
            events.push(RaceEvent::PitEntry { lap, session_time: t, position: pos });
        } else if !on_pit && self.prev_on_pit {
            events.push(RaceEvent::PitExit { lap, session_time: t, position: pos });
        }

        // ── Close approach (cars ahead) ────────────────────────────────────
        match nearest_ahead {
            Some((car_idx, gap)) if gap < CLOSE_APPROACH_THRESH_S => {
                self.consecutive_close += 1;
                if self.consecutive_close >= CLOSE_APPROACH_MIN_FRAMES
                    && (t - self.last_close_t) > 30.0
                    && Some(car_idx) != self.tracking_car
                {
                    self.tracking_car  = Some(car_idx);
                    self.last_close_t  = t;
                    let car_race_position = frame.car_idx_position
                        .get(car_idx as usize).copied().unwrap_or(0);
                    events.push(RaceEvent::CloseApproach {
                        lap,
                        session_time:      t,
                        car_ahead_idx:     car_idx,
                        gap_s:             gap,
                        car_race_position,
                    });
                }
            }
            other => {
                self.consecutive_close = 0;
                let current_idx = other.map(|(c, _)| c);
                if current_idx != self.tracking_car {
                    self.tracking_car = None;
                }
            }
        }

        // ── Pressure behind ────────────────────────────────────────────────
        match nearest_behind {
            Some((car_idx, gap)) if gap < CLOSE_APPROACH_THRESH_S => {
                self.consecutive_close_beh += 1;
                if self.consecutive_close_beh >= CLOSE_APPROACH_MIN_FRAMES
                    && (t - self.last_close_beh_t) > 30.0
                    && Some(car_idx) != self.tracking_car_beh
                {
                    self.tracking_car_beh  = Some(car_idx);
                    self.last_close_beh_t  = t;
                    let car_race_position = frame.car_idx_position
                        .get(car_idx as usize).copied().unwrap_or(0);
                    events.push(RaceEvent::PressureBehind {
                        lap,
                        session_time:      t,
                        car_behind_idx:    car_idx,
                        gap_s:             gap,
                        car_race_position,
                    });
                }
            }
            other => {
                self.consecutive_close_beh = 0;
                let current_idx = other.map(|(c, _)| c);
                if current_idx != self.tracking_car_beh {
                    self.tracking_car_beh = None;
                }
            }
        }

        // ── Lap crossing ───────────────────────────────────────────────────
        if let Some(prev_lap) = self.prev_lap {
            if lap != prev_lap {
                let done_lap = prev_lap;

                // Mark pit laps
                let pit_frames = self.lap_pit_frames.get(&done_lap).copied().unwrap_or(0);
                if pit_frames >= PIT_LAP_FRAME_THRESH {
                    self.pit_laps.insert(done_lap);
                }

                let end_pos = self.prev_position.unwrap_or(pos);
                self.lap_end_positions.insert(done_lap, end_pos);

                events.push(RaceEvent::LapComplete {
                    lap:        done_lap,
                    session_time: t,
                    lap_time_s: self.lap_timer.completed(done_lap),
                    position:   end_pos,
                    pit_frames,
                });

                // Position change vs previous lap end
                if let Some(&prev_pos) = self.lap_end_positions.get(&done_lap.wrapping_sub(1)) {
                    if done_lap > 0 {
                        let delta = prev_pos as i16 - end_pos as i16;
                        if delta > 0 && !self.pit_laps.contains(&done_lap) {
                            events.push(RaceEvent::Overtake {
                                lap: done_lap, session_time: t,
                                position_from:    prev_pos,
                                position_to:      end_pos,
                                positions_gained: delta as u8,
                            });
                        } else if delta < 0 {
                            events.push(RaceEvent::PositionLost {
                                lap: done_lap, session_time: t,
                                position_from:   prev_pos,
                                position_to:     end_pos,
                                positions_lost:  (-delta) as u8,
                            });
                        }
                    }
                }

                // ── Forward regression ─────────────────────────────────────
                self.regression.ingest(&self.sampler, done_lap);
                let per_bucket  = self.regression.per_bucket_slopes(MIN_PUSH_READINGS);
                let car_medians = self.regression.per_car_median_slopes(MIN_PUSH_READINGS);
                let fwd = classify(
                    &car_medians, &per_bucket,
                    self.anchor_count,
                    self.prev_slope,
                    self.pit_laps.contains(&done_lap),
                );

                if fwd.state != self.engine_state {
                    match &fwd.state {
                        BattleState::Push => {
                            if let (Some(car_idx), Some(si)) = (fwd.threat_car, fwd.slope_info.clone()) {
                                events.push(RaceEvent::Push {
                                    lap: done_lap, session_time: t,
                                    car_ahead_idx: car_idx, slope_info: si,
                                });
                            }
                        }
                        BattleState::AttackSetup => {
                            if let (Some(car_idx), Some(si)) = (fwd.threat_car, fwd.slope_info.clone()) {
                                events.push(RaceEvent::AttackSetup {
                                    lap: done_lap, session_time: t,
                                    car_ahead_idx: car_idx, slope_info: si,
                                });
                            }
                        }
                        _ => {}
                    }
                    self.engine_state = fwd.state;
                }
                if let Some(si) = &fwd.slope_info {
                    self.prev_slope = Some(si.median_slope);
                }

                // ── Defensive regression ───────────────────────────────────
                self.regression_behind.ingest(&self.sampler_behind, done_lap);
                let per_bucket_beh  = self.regression_behind.per_bucket_slopes(MIN_PUSH_READINGS);
                let car_medians_beh = self.regression_behind.per_car_median_slopes(MIN_PUSH_READINGS);
                let def = classify(
                    &car_medians_beh, &per_bucket_beh,
                    self.anchor_count,
                    self.prev_slope_beh,
                    self.pit_laps.contains(&done_lap),
                );

                if def.state != self.defensive_state {
                    match &def.state {
                        BattleState::Push => {
                            if let (Some(car_idx), Some(si)) = (def.threat_car, def.slope_info.clone()) {
                                events.push(RaceEvent::DefendPush {
                                    lap: done_lap, session_time: t,
                                    car_behind_idx: car_idx, slope_info: si,
                                });
                            }
                        }
                        BattleState::AttackSetup => {
                            if let (Some(car_idx), Some(si)) = (def.threat_car, def.slope_info.clone()) {
                                events.push(RaceEvent::DefendAttack {
                                    lap: done_lap, session_time: t,
                                    car_behind_idx: car_idx, slope_info: si,
                                });
                            }
                        }
                        _ => {}
                    }
                    self.defensive_state = def.state;
                }
                if let Some(si) = &def.slope_info {
                    self.prev_slope_beh = Some(si.median_slope);
                }
            }
        }

        // ── Carry-over fields ──────────────────────────────────────────────
        self.prev_lap      = Some(lap);
        self.prev_on_pit   = on_pit;
        self.prev_position = Some(pos);

        events
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Synthesise yellow-flag bits from known track-coordinate zones.
/// Real iRacing replay exports have stale flags, so we ignore `session_flags`
/// and re-derive cleanness from physical location alone.
fn synthesize_flags(lap: u8, ldp: f32) -> u32 {
    for &(ylap, p0, p1) in YELLOW_ZONES {
        if lap == ylap && ldp >= p0 && ldp <= p1 {
            return YELLOW_WAVE;
        }
    }
    0
}

