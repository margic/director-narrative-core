use std::array;

use crate::anchor_sampler::AnchorSampler;
use crate::battle_state::BattleState;
use crate::session_info::SessionRoster;
use crate::telemetry_frame::TelemetryFrame;

pub struct CarRegistry {
    cars: [Option<CarState>; 64],
    last_seen_tick: [i64; 64],
}

pub struct CarState {
    pub car_idx:          u8,
    pub car_number:       String,
    pub driver_name:      String,
    pub car_class_id:     u32,
    pub current_position: u8,
    pub current_lap:      i32,
    pub lap_dist_pct:     f32,
    pub on_pit_road:      bool,
    pub track_surface:    i32,
    pub last_lap_time_s:  f32,
    pub best_lap_time_s:  f32,
    pub speed_ema_mps:    f32,
    pub sampler:          AnchorSampler,
    pub opponent_history: Vec<OpponentHistory>,
}

pub struct OpponentHistory {
    pub car_idx:              u8,
    pub first_seen_lap:       u8,
    pub last_engaged_lap:     Option<u8>,
    pub last_state_forward:   BattleState,
    pub last_state_defensive: BattleState,
    pub time_in_push_s:       f32,
    pub time_in_attack_s:     f32,
    pub skirmish_count:       u32,
}

impl CarRegistry {
    pub fn new() -> Self {
        Self {
            cars: array::from_fn(|_| None),
            last_seen_tick: [i64::MIN; 64],
        }
    }

    pub fn insert(&mut self, car: CarState, session_tick: i64) {
        let idx = car.car_idx as usize;
        if idx < self.cars.len() {
            self.cars[idx] = Some(car);
            self.last_seen_tick[idx] = session_tick;
        }
    }

    pub fn update_from_frame(
        &mut self,
        frame: &TelemetryFrame,
        roster: &SessionRoster,
        session_tick: i64,
        anchor_count: usize,
    ) {
        let mut in_frame = [false; 64];
        for (i, &position) in frame.car_idx_position.iter().enumerate().take(64) {
            if position == 0 {
                continue;
            }
            in_frame[i] = true;
            let car_idx = i as u8;
            let roster_ref = roster.lookup(car_idx);
            let lap = frame.car_idx_lap_completed.get(i).copied().unwrap_or(frame.lap as i32);
            let ldp = frame.car_idx_lap_dist_pct.get(i).copied().unwrap_or(-1.0).max(0.0);
            let on_pit = frame.car_idx_on_pit_road.get(i).copied().unwrap_or(false);
            let track_surface = frame.car_idx_track_surface.get(i).copied().unwrap_or_default();

            let state = self.cars[i].get_or_insert_with(|| CarState {
                car_idx,
                car_number: roster_ref.map(|r| r.car_number.clone()).unwrap_or_else(|| car_idx.to_string()),
                driver_name: roster_ref.map(|r| r.driver_name.clone()).unwrap_or_else(|| format!("Car {car_idx}")),
                car_class_id: roster_ref.and_then(|r| r.car_class_id).unwrap_or(0),
                current_position: position,
                current_lap: lap,
                lap_dist_pct: ldp,
                on_pit_road: on_pit,
                track_surface,
                last_lap_time_s: frame.lap_last_lap_time,
                best_lap_time_s: if frame.lap_last_lap_time > 0.0 { frame.lap_last_lap_time } else { 0.0 },
                speed_ema_mps: 0.0,
                sampler: AnchorSampler::new(anchor_count),
                opponent_history: Vec::new(),
            });

            if let Some(r) = roster_ref {
                state.car_number = r.car_number.clone();
                state.driver_name = r.driver_name.clone();
                state.car_class_id = r.car_class_id.unwrap_or(state.car_class_id);
            }
            if state.sampler.n_buckets() != anchor_count {
                state.sampler = AnchorSampler::new(anchor_count);
            }

            let prev_progress = state.current_lap as f32 + state.lap_dist_pct;
            let mut new_progress = lap as f32 + ldp;
            if new_progress + 0.5 < prev_progress {
                new_progress += 1.0;
            }
            let dt_ticks = session_tick.saturating_sub(self.last_seen_tick[i]);
            if dt_ticks > 0 {
                let dt_s = dt_ticks as f32 / 60.0;
                let raw_speed = ((new_progress - prev_progress).max(0.0) * 5000.0) / dt_s;
                state.speed_ema_mps = if state.speed_ema_mps <= 0.0 {
                    raw_speed
                } else {
                    0.1 * raw_speed + 0.9 * state.speed_ema_mps
                };
            }

            state.current_position = position;
            state.current_lap = lap;
            state.lap_dist_pct = ldp;
            state.on_pit_road = on_pit;
            state.track_surface = track_surface;
            state.last_lap_time_s = frame.lap_last_lap_time;
            if frame.lap_last_lap_time > 0.0
                && (state.best_lap_time_s == 0.0 || frame.lap_last_lap_time < state.best_lap_time_s)
            {
                state.best_lap_time_s = frame.lap_last_lap_time;
            }
            self.last_seen_tick[i] = session_tick;
        }

        for i in 0..self.cars.len() {
            if in_frame[i] {
                continue;
            }
            if self.cars[i].is_some() && session_tick.saturating_sub(self.last_seen_tick[i]) > 3600 {
                self.cars[i] = None;
                self.last_seen_tick[i] = i64::MIN;
            }
        }
    }

    pub fn active_cars(&self) -> impl Iterator<Item = &CarState> {
        self.cars.iter().filter_map(|car| car.as_ref())
    }

    pub fn get(&self, car_idx: u8) -> Option<&CarState> {
        self.cars.get(car_idx as usize).and_then(|car| car.as_ref())
    }

    pub fn get_mut(&mut self, car_idx: u8) -> Option<&mut CarState> {
        self.cars.get_mut(car_idx as usize).and_then(|car| car.as_mut())
    }

    pub fn find_opponent_history_mut(
        &mut self,
        car_idx: u8,
        opponent_idx: u8,
    ) -> Option<&mut OpponentHistory> {
        self.get_mut(car_idx)
            .and_then(|car| car.opponent_history.iter_mut().find(|h| h.car_idx == opponent_idx))
    }

    pub fn update_opponent_history(
        &mut self,
        player_idx: u8,
        opponent_idx: u8,
        state: &BattleState,
        is_pit_lap: bool,
        lap: u8,
    ) {
        let Some(car) = self.get_mut(player_idx) else { return; };
        let history = if let Some(existing) = car.opponent_history.iter_mut().find(|h| h.car_idx == opponent_idx) {
            existing
        } else {
            car.opponent_history.push(OpponentHistory {
                car_idx: opponent_idx,
                first_seen_lap: lap,
                last_engaged_lap: None,
                last_state_forward: BattleState::Idle,
                last_state_defensive: BattleState::Idle,
                time_in_push_s: 0.0,
                time_in_attack_s: 0.0,
                skirmish_count: 0,
            });
            car.opponent_history.last_mut().expect("opponent history entry should exist after insertion")
        };

        if !is_pit_lap {
            if matches!(history.last_state_forward, BattleState::Idle) && matches!(state, BattleState::Tracking) {
                history.skirmish_count += 1;
                history.last_engaged_lap = Some(lap);
            }
            if matches!(state, BattleState::Push) {
                history.time_in_push_s += 60.0;
            }
            if matches!(state, BattleState::AttackSetup) {
                history.time_in_attack_s += 60.0;
            }
        }
        history.last_state_forward = state.clone();
        history.last_engaged_lap = Some(lap);
    }

    pub fn class_count(&self) -> usize {
        let mut classes = std::collections::HashSet::new();
        for car in self.active_cars() {
            classes.insert(car.car_class_id);
        }
        classes.len()
    }
}

impl Default for CarRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::session_info::{CarRef, SessionRoster};

    use super::*;

    fn roster() -> SessionRoster {
        SessionRoster::from_cars(vec![
            CarRef {
                car_idx: 0,
                car_number: "01".into(),
                driver_name: "Player".into(),
                team_name: None,
                car_class_short_name: Some("LMP2".into()),
                car_class_id: Some(1),
                user_id: None,
                irating: None,
                lic_string: None,
                flair_name: None,
            },
            CarRef {
                car_idx: 1,
                car_number: "11".into(),
                driver_name: "GT3 A".into(),
                team_name: None,
                car_class_short_name: Some("GT3".into()),
                car_class_id: Some(2),
                user_id: None,
                irating: None,
                lic_string: None,
                flair_name: None,
            },
        ])
    }

    fn frame(on_pit: bool, pos1: u8, tick: i64) -> TelemetryFrame {
        TelemetryFrame {
            lap: 3,
            session_time: tick as f32 / 60.0,
            lap_dist_pct: 0.2,
            player_car_idx: 0,
            player_car_position: 2,
            on_pit_road: false,
            session_flags: 0,
            car_idx_lap_dist_pct: vec![0.2, 0.4],
            car_idx_position: vec![2, pos1],
            car_idx_on_pit_road: vec![false, on_pit],
            car_idx_track_surface: vec![0, 0],
            lap_last_lap_time: 90.0,
            session_info_update: 0,
            session_tick: tick,
            session_state: 4,
            session_num: 0,
            player_incident_count: 0,
            car_idx_lap_completed: vec![3, 3],
            lf_temp_m: 0.0,
            rf_temp_m: 0.0,
            lr_temp_m: 0.0,
            rr_temp_m: 0.0,
            fuel_level: 0.0,
            throttle: 0.0,
            brake: 0.0,
            speed: 0.0,
        }
    }

    #[test]
    fn slot_allocation() {
        let mut registry = CarRegistry::new();
        registry.update_from_frame(&frame(false, 1, 0), &roster(), 0, 12);
        assert!(registry.get(0).is_some());
        assert!(registry.get(1).is_some());
    }

    #[test]
    fn pit_cycle_continuity() {
        let mut registry = CarRegistry::new();
        registry.update_from_frame(&frame(false, 1, 0), &roster(), 0, 12);
        registry.update_from_frame(&frame(true, 1, 60), &roster(), 60, 12);
        let car = registry.get(1).expect("car should persist through pit");
        assert!(car.on_pit_road);
        assert_eq!(car.driver_name, "GT3 A");
    }

    #[test]
    fn disconnect_expiry() {
        let mut registry = CarRegistry::new();
        registry.update_from_frame(&frame(false, 1, 0), &roster(), 0, 12);
        registry.update_from_frame(&frame(false, 0, 3701), &roster(), 3701, 12);
        assert!(registry.get(1).is_none());
    }

    #[test]
    fn multi_class_car_classification() {
        let mut registry = CarRegistry::new();
        registry.update_from_frame(&frame(false, 1, 0), &roster(), 0, 12);
        assert_eq!(registry.get(0).unwrap().car_class_id, 1);
        assert_eq!(registry.get(1).unwrap().car_class_id, 2);
    }
}
