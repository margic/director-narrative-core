use std::collections::HashMap;
use std::collections::HashSet;

use crate::car_registry::CarRegistry;
use crate::race_event::RaceEvent;

const SPEED_DROP_THRESHOLD_MPS: f32 = 8.0;
const SPEED_RATIO_THRESHOLD: f32 = 0.80;

#[derive(Clone, Copy)]
struct CarSnapshot {
    track_surface: i32,
    on_pit_road: bool,
    speed_ema_mps: f32,
}

pub struct BasicIncidentDetector {
    last_snapshot: HashMap<u8, CarSnapshot>,
    active_alerts: HashSet<u8>,
    last_player_incident_count: Option<i32>,
}

impl BasicIncidentDetector {
    pub fn new() -> Self {
        Self {
            last_snapshot: HashMap::new(),
            active_alerts: HashSet::new(),
            last_player_incident_count: None,
        }
    }

    pub fn update(
        &mut self,
        registry: &CarRegistry,
        lap: u8,
        session_time: f32,
        _session_tick: i64,
        player_car_idx: u8,
        player_incident_count: i32,
    ) -> Vec<RaceEvent> {
        let mut events = Vec::new();
        let mut player_alert_emitted = false;
        let player_prev_snapshot = self.last_snapshot.get(&player_car_idx).copied();

        for car in registry.active_cars() {
            let snapshot = CarSnapshot {
                track_surface: car.track_surface,
                on_pit_road: car.on_pit_road,
                speed_ema_mps: car.speed_ema_mps,
            };

            if let Some(prev) = self.last_snapshot.get(&car.car_idx).copied() {
                let surface_changed = snapshot.track_surface != prev.track_surface;
                let speed_drop_mps = (prev.speed_ema_mps - snapshot.speed_ema_mps).max(0.0);
                let speed_ratio = if prev.speed_ema_mps > 1.0 {
                    snapshot.speed_ema_mps / prev.speed_ema_mps
                } else {
                    1.0
                };
                let severe_drop = speed_drop_mps >= SPEED_DROP_THRESHOLD_MPS
                    && speed_ratio <= SPEED_RATIO_THRESHOLD;
                let not_pit_transition = !snapshot.on_pit_road && !prev.on_pit_road;

                // Emit all credible incident signatures:
                // - surface transitions (off-track/contact aftermath)
                // - severe speed collapses
                // Keep edge-triggering so a single sustained condition does not spam.
                let incident_condition = not_pit_transition && (surface_changed || severe_drop);
                if !incident_condition {
                    self.active_alerts.remove(&car.car_idx);
                }

                if incident_condition
                    && !self.active_alerts.contains(&car.car_idx)
                {
                    let severity = speed_drop_mps / prev.speed_ema_mps.max(1.0);
                    let reason = if surface_changed && severe_drop {
                        "surface_change_and_speed_drop"
                    } else if severe_drop {
                        "speed_drop"
                    } else if snapshot.track_surface < prev.track_surface {
                        "surface_drop"
                    } else {
                        "surface_change"
                    }
                    .to_owned();

                    events.push(RaceEvent::IncidentAlert {
                        lap,
                        session_time,
                        car_idx: car.car_idx,
                        driver_incident_count: (car.car_idx == player_car_idx)
                            .then_some(player_incident_count),
                        previous_track_surface: prev.track_surface,
                        current_track_surface: snapshot.track_surface,
                        previous_speed_mps: prev.speed_ema_mps,
                        current_speed_mps: snapshot.speed_ema_mps,
                        speed_drop_mps,
                        severity,
                        reason,
                    });
                    if car.car_idx == player_car_idx {
                        player_alert_emitted = true;
                    }
                    self.active_alerts.insert(car.car_idx);
                }
            }

            self.last_snapshot.insert(car.car_idx, snapshot);
        }

        let previous_count = self
            .last_player_incident_count
            .unwrap_or(player_incident_count);
        if player_incident_count > previous_count && !player_alert_emitted {
            if let Some(car) = registry.get(player_car_idx) {
                let prev = player_prev_snapshot.unwrap_or(CarSnapshot {
                    track_surface: car.track_surface,
                    on_pit_road: car.on_pit_road,
                    speed_ema_mps: car.speed_ema_mps,
                });
                let speed_drop_mps = (prev.speed_ema_mps - car.speed_ema_mps).max(0.0);
                let severity = (player_incident_count - previous_count) as f32;

                events.push(RaceEvent::IncidentAlert {
                    lap,
                    session_time,
                    car_idx: player_car_idx,
                    driver_incident_count: Some(player_incident_count),
                    previous_track_surface: prev.track_surface,
                    current_track_surface: car.track_surface,
                    previous_speed_mps: prev.speed_ema_mps,
                    current_speed_mps: car.speed_ema_mps,
                    speed_drop_mps,
                    severity,
                    reason: "incident_count_increase".to_owned(),
                });
            }
        }
        self.last_player_incident_count = Some(player_incident_count);

        events
    }
}

impl Default for BasicIncidentDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchor_sampler::AnchorSampler;
    use crate::car_registry::CarState;

    fn car(car_idx: u8, track_surface: i32, on_pit_road: bool, speed_ema_mps: f32) -> CarState {
        CarState {
            car_idx,
            car_number: car_idx.to_string(),
            driver_name: car_idx.to_string(),
            car_class_id: 1,
            current_position: car_idx + 1,
            current_lap: 4,
            lap_dist_pct: 0.25,
            on_pit_road,
            track_surface,
            last_lap_time_s: 0.0,
            best_lap_time_s: 0.0,
            speed_ema_mps,
            sampler: AnchorSampler::new(10),
            opponent_history: Vec::new(),
        }
    }

    #[test]
    fn emits_basic_incident_on_surface_change_with_speed_drop() {
        let mut registry = CarRegistry::new();
        registry.insert(car(7, 3, false, 72.0), 0);

        let mut detector = BasicIncidentDetector::new();
        let first = detector.update(&registry, 4, 120.0, 60, 7, 0);
        assert!(first.is_empty());

        registry.get_mut(7).unwrap().track_surface = 1;
        registry.get_mut(7).unwrap().speed_ema_mps = 52.0;
        let events = detector.update(&registry, 4, 121.0, 1_920, 7, 6);

        assert!(events.iter().any(|event| {
            matches!(
                event,
                RaceEvent::IncidentAlert {
                    car_idx: 7,
                    driver_incident_count: Some(6),
                    ..
                }
            )
        }));

        let repeated = detector.update(&registry, 4, 121.2, 1_950, 7, 6);
        assert!(repeated.is_empty(), "duplicate alert should be suppressed while condition remains active");
    }

    #[test]
    fn ignores_pit_transitions() {
        let mut registry = CarRegistry::new();
        registry.insert(car(7, 3, false, 72.0), 0);

        let mut detector = BasicIncidentDetector::new();
        let _ = detector.update(&registry, 4, 120.0, 60, 7, 0);

        registry.get_mut(7).unwrap().on_pit_road = true;
        registry.get_mut(7).unwrap().track_surface = 2;
        registry.get_mut(7).unwrap().speed_ema_mps = 20.0;
        let events = detector.update(&registry, 4, 121.0, 120, 7, 0);

        assert!(events.is_empty());
    }
}
