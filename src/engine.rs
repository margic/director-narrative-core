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
    best_lap_time_s:    Option<f32>,
    // Battle-engaged tracking — cars currently within battle gap
    engaged_cars:       HashSet<u8>,
    engaged_cars_beh:   HashSet<u8>,
    // Close-approach tracking (forward + defensive)
    consecutive_close:      u32,
    last_close_t:           f32,
    tracking_car:           Option<u8>,
    consecutive_close_beh:  u32,
    last_close_beh_t:       f32,
    tracking_car_beh:       Option<u8>,
    // Session/flag carry-over
    prev_session_state: i32,
    prev_session_flags: u32,
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
            best_lap_time_s:    None,
            engaged_cars:       HashSet::new(),
            engaged_cars_beh:   HashSet::new(),
            consecutive_close:      0,
            last_close_t:           f32::NEG_INFINITY,
            tracking_car:           None,
            consecutive_close_beh:  0,
            last_close_beh_t:       f32::NEG_INFINITY,
            tracking_car_beh:       None,
            prev_session_state: 0,
            prev_session_flags: 0,
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

        let session_state = frame.session_state;
        let session_flags = frame.session_flags;

        // ── Session / flag transitions ─────────────────────────────────────
        if session_state == 4 && self.prev_session_state != 4 {
            events.push(RaceEvent::RaceGreen { lap, session_time: t });
        }
        if session_state == 5 && self.prev_session_state != 5 {
            events.push(RaceEvent::RaceCheckered { lap, session_time: t });
        }
        if session_flags & CAUTION != 0 && self.prev_session_flags & CAUTION == 0 {
            events.push(RaceEvent::FlagYellowFullCourse { lap, session_time: t });
        } else if session_flags & YELLOW_WAVE != 0 && self.prev_session_flags & YELLOW_WAVE == 0 {
            events.push(RaceEvent::FlagYellowLocal { lap, session_time: t });
        }
        self.prev_session_state = session_state;
        self.prev_session_flags = session_flags;

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

        // ── Battle engaged / broken (cars ahead) ──────────────────────────
        match nearest_ahead {
            Some((car_idx, gap)) if gap < CLOSE_APPROACH_THRESH_S => {
                self.consecutive_close += 1;
                if self.consecutive_close >= CLOSE_APPROACH_MIN_FRAMES
                    && (t - self.last_close_t) > 30.0
                    && Some(car_idx) != self.tracking_car
                {
                    self.tracking_car  = Some(car_idx);
                    self.last_close_t  = t;
                    self.engaged_cars.insert(car_idx);
                    let car_race_position = frame.car_idx_position
                        .get(car_idx as usize).copied().unwrap_or(0);
                    events.push(RaceEvent::BattleEngaged {
                        lap,
                        session_time:      t,
                        car_idx,
                        gap_s:             gap,
                        car_race_position,
                    });
                }
            }
            other => {
                self.consecutive_close = 0;
                let current_idx = other.map(|(c, _)| c);
                if current_idx != self.tracking_car {
                    // Emit BATTLE_BROKEN for any car that was engaged but is now gone
                    if let Some(prev_car) = self.tracking_car {
                        if self.engaged_cars.remove(&prev_car) {
                            let gap_s = other.map(|(_, g)| g).unwrap_or(f32::MAX);
                            events.push(RaceEvent::BattleBroken {
                                lap,
                                session_time: t,
                                car_idx: prev_car,
                                gap_s,
                            });
                        }
                    }
                    self.tracking_car = None;
                }
            }
        }

        // ── Battle engaged / broken (cars behind) ─────────────────────────
        match nearest_behind {
            Some((car_idx, gap)) if gap < CLOSE_APPROACH_THRESH_S => {
                self.consecutive_close_beh += 1;
                if self.consecutive_close_beh >= CLOSE_APPROACH_MIN_FRAMES
                    && (t - self.last_close_beh_t) > 30.0
                    && Some(car_idx) != self.tracking_car_beh
                {
                    self.tracking_car_beh  = Some(car_idx);
                    self.last_close_beh_t  = t;
                    self.engaged_cars_beh.insert(car_idx);
                    let car_race_position = frame.car_idx_position
                        .get(car_idx as usize).copied().unwrap_or(0);
                    events.push(RaceEvent::BattleEngaged {
                        lap,
                        session_time:      t,
                        car_idx,
                        gap_s:             gap,
                        car_race_position,
                    });
                }
            }
            other => {
                self.consecutive_close_beh = 0;
                let current_idx = other.map(|(c, _)| c);
                if current_idx != self.tracking_car_beh {
                    if let Some(prev_car) = self.tracking_car_beh {
                        if self.engaged_cars_beh.remove(&prev_car) {
                            let gap_s = other.map(|(_, g)| g).unwrap_or(f32::MAX);
                            events.push(RaceEvent::BattleBroken {
                                lap,
                                session_time: t,
                                car_idx: prev_car,
                                gap_s,
                            });
                        }
                    }
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

                let lap_time_s = self.lap_timer.completed(done_lap);
                if let Some(lt) = lap_time_s {
                    self.best_lap_time_s = Some(match self.best_lap_time_s {
                        Some(best) if best <= lt => best,
                        _ => lt,
                    });
                }
                events.push(RaceEvent::LapCompleted {
                    lap:            done_lap,
                    session_time:   t,
                    lap_time_s,
                    best_lap_time_s: self.best_lap_time_s,
                    position:       end_pos,
                    pit_frames,
                });

                // Position change vs previous lap end
                if let Some(&prev_pos) = self.lap_end_positions.get(&done_lap.wrapping_sub(1)) {
                    if done_lap > 0 {
                        let delta = prev_pos as i16 - end_pos as i16;
                        if delta > 0 && !self.pit_laps.contains(&done_lap) {
                            if end_pos == 1 {
                                events.push(RaceEvent::OvertakeForLead {
                                    lap: done_lap, session_time: t,
                                    position_from:    prev_pos,
                                    positions_gained: delta as u8,
                                });
                            } else {
                                events.push(RaceEvent::Overtake {
                                    lap: done_lap, session_time: t,
                                    position_from:    prev_pos,
                                    position_to:      end_pos,
                                    positions_gained: delta as u8,
                                });
                            }
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
                        BattleState::Push | BattleState::AttackSetup => {
                            if let (Some(car_idx), Some(si)) = (fwd.threat_car, fwd.slope_info.clone()) {
                                events.push(RaceEvent::BattleClosing {
                                    lap: done_lap, session_time: t,
                                    car_idx,
                                    closing_rate_sec_per_lap: si.median_slope.abs(),
                                    slope_info: si,
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
                        BattleState::Push | BattleState::AttackSetup => {
                            if let (Some(car_idx), Some(si)) = (def.threat_car, def.slope_info.clone()) {
                                events.push(RaceEvent::BattleClosing {
                                    lap: done_lap, session_time: t,
                                    car_idx,
                                    closing_rate_sec_per_lap: si.median_slope.abs(),
                                    slope_info: si,
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

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal TelemetryFrame. Player is car_idx=0 at position 5.
    /// `opponent_ldp` places car 1 just ahead with a ~0.54 s gap (540 s fallback lap).
    fn frame_with_gap(lap: u8, t: f32, session_state: i32, opponent_ldp: f32) -> TelemetryFrame {
        TelemetryFrame {
            lap,
            session_time:            t,
            lap_dist_pct:            0.500,
            player_car_idx:          0,
            player_car_position:     5,
            on_pit_road:             false,
            session_flags:           0,
            car_idx_lap_dist_pct:    vec![0.500, opponent_ldp],
            car_idx_position:        vec![5, 4], // player=5, car1=4 (ahead)
            car_idx_on_pit_road:     vec![false, false],
            lap_last_lap_time:       0.0,
            session_info_update:     0,
            session_tick:            0,
            session_state,
            session_num:             0,
            car_idx_lap_completed:   vec![],
        }
    }

    /// Frame with no car ahead — triggers BattleBroken when previously engaged.
    fn frame_no_opponent(lap: u8, t: f32) -> TelemetryFrame {
        TelemetryFrame {
            lap,
            session_time:            t,
            lap_dist_pct:            0.500,
            player_car_idx:          0,
            player_car_position:     5,
            on_pit_road:             false,
            session_flags:           0,
            car_idx_lap_dist_pct:    vec![0.500, -1.0], // car 1 inactive
            car_idx_position:        vec![5, 0],
            car_idx_on_pit_road:     vec![false, false],
            lap_last_lap_time:       0.0,
            session_info_update:     0,
            session_tick:            0,
            session_state:           4,
            session_num:             0,
            car_idx_lap_completed:   vec![],
        }
    }

    #[test]
    fn battle_engaged_fires_on_lap_1() {
        // Gap 0.001 * 540 (fallback) = 0.54 s < CLOSE_APPROACH_THRESH_S (1.5 s)
        let mut engine = NarrativeEngine::new(10);
        let mut all_events: Vec<RaceEvent> = Vec::new();
        for i in 0..6u8 {
            let evs = engine.process_frame(&frame_with_gap(1, i as f32, 4, 0.501));
            all_events.extend(evs);
        }
        let engaged = all_events.iter().find(|e| {
            matches!(e, RaceEvent::BattleEngaged { lap: 1, car_idx: 1, .. })
        });
        assert!(engaged.is_some(), "BATTLE_ENGAGED should fire on lap 1");
    }

    #[test]
    fn battle_broken_fires_after_engaged() {
        let mut engine = NarrativeEngine::new(10);
        let mut all_events: Vec<RaceEvent> = Vec::new();

        // Trigger BattleEngaged (5+ frames within gap)
        for i in 0..6u8 {
            let evs = engine.process_frame(&frame_with_gap(1, i as f32, 4, 0.501));
            all_events.extend(evs);
        }
        assert!(
            all_events.iter().any(|e| matches!(e, RaceEvent::BattleEngaged { car_idx: 1, .. })),
            "prerequisite: BattleEngaged should fire first"
        );

        // Now remove the opponent — BattleBroken should fire
        let evs = engine.process_frame(&frame_no_opponent(1, 6.0));
        let broken = evs.iter().any(|e| matches!(e, RaceEvent::BattleBroken { car_idx: 1, .. }));
        assert!(broken, "BATTLE_BROKEN should fire when the engaged opponent disappears");
    }

    #[test]
    fn race_green_fires_on_session_state_transition() {
        let mut engine = NarrativeEngine::new(10);

        // Frame in ParadeLaps (state=3) — no RACE_GREEN
        let evs1 = engine.process_frame(&frame_with_gap(0, 0.0, 3, 0.501));
        assert!(
            !evs1.iter().any(|e| matches!(e, RaceEvent::RaceGreen { .. })),
            "RACE_GREEN should NOT fire for state 3"
        );

        // Transition to Racing (state=4) — RACE_GREEN fires
        let evs2 = engine.process_frame(&frame_with_gap(1, 1.0, 4, 0.501));
        assert!(
            evs2.iter().any(|e| matches!(e, RaceEvent::RaceGreen { .. })),
            "RACE_GREEN should fire when SessionState transitions to 4"
        );

        // Second Racing frame — RACE_GREEN should NOT fire again
        let evs3 = engine.process_frame(&frame_with_gap(1, 2.0, 4, 0.501));
        assert!(
            !evs3.iter().any(|e| matches!(e, RaceEvent::RaceGreen { .. })),
            "RACE_GREEN should only fire once per transition"
        );
    }
}

