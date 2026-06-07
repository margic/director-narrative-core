//! `PublisherEvent` envelope — wire format for `/api/publisher/v2/ingest`.
//!
//! Every [`RaceEvent`] emitted by the engine is wrapped in a [`PublisherEvent`]
//! before being batched and POSTed to Race Control. The envelope carries
//! identity, timing, and session-context metadata that the engine itself does
//! not need.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::race_event::{EventScope, FlagScope, RaceEvent};
use crate::session_info::{CarRef, SessionMetadata, SessionRoster};
use crate::telemetry_frame::TelemetryFrame;

// ── Envelope types ────────────────────────────────────────────────────────────

/// Wire envelope — serialises to the Race Control API schema exactly.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublisherEvent {
    /// UUID v4 — idempotency key; unique per event emission.
    pub id: String,
    pub race_session_id: String,
    pub rig_id: String,
    /// `PublisherEventType` string value (e.g. `"BATTLE_CLOSING"`).
    #[serde(rename = "type")]
    pub event_type: String,
    /// Wall-clock milliseconds since Unix epoch at the moment of construction.
    pub timestamp: i64,
    /// iRacing `SessionTime` in seconds.
    pub session_time: f64,
    /// iRacing `SessionTick` counter.
    pub session_tick: i64,
    /// High-level ownership of the event.
    pub scope: EventScope,
    /// Car identity resolved from the session roster for car-scoped events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub car: Option<CarRef>,
    /// Event-specific fields (all fields of the `RaceEvent` variant except
    /// the `event_type` discriminator tag, which is hoisted to the envelope).
    pub payload: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<PublisherEventContext>,
}

/// Supplementary session context attached to every envelope.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublisherEventContext {
    /// Highest value in `CarIdxLapCompleted` — the leader's completed laps.
    pub leader_lap: Option<i32>,
    pub session_state: Option<i32>,
    pub session_flags: Option<u32>,
    pub session_num: Option<i32>,
    pub sub_session_id: Option<i64>,
    pub session_type: Option<String>,
    pub track_name: Option<String>,
}

// ── Builder ───────────────────────────────────────────────────────────────────

/// Wrap a [`RaceEvent`] in a [`PublisherEvent`] envelope ready for serialisation.
///
/// * `roster` — optional current session roster. Car-scoped events resolve a
///   `car` envelope field from the roster, falling back to a stub containing
///   only `carIdx` and the stringified index as `carNumber` when the slot is
///   absent. Session- and rig-scoped events omit `car` entirely.
pub fn build_event(
    race_event: &RaceEvent,
    frame: &TelemetryFrame,
    roster: Option<&SessionRoster>,
    race_session_id: &str,
    rig_id: &str,
    session_meta: Option<&SessionMetadata>,
    sub_session_id: Option<i64>,
) -> PublisherEvent {
    let id = Uuid::new_v4().to_string();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    // Serialise the event, hoist the discriminator tag, use remainder as payload.
    let mut event_value =
        serde_json::to_value(race_event).expect("RaceEvent is always serialisable");
    let event_type = event_value
        .as_object_mut()
        .and_then(|m| m.remove("event_type"))
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let scope = race_event.event_scope();
    let mut payload = event_value;
    enrich_payload(&mut payload, race_event, frame, roster);

    let car = event_car(race_event, frame.player_car_idx, roster);

    let leader_lap = frame.car_idx_lap_completed.iter().copied().max();
    let context = Some(PublisherEventContext {
        leader_lap,
        session_state: Some(frame.session_state),
        session_flags: Some(frame.session_flags),
        session_num: Some(frame.session_num),
        sub_session_id,
        session_type: session_meta.and_then(|m| m.session_type.clone()),
        track_name: session_meta.and_then(|m| m.track_name.clone()),
    });

    PublisherEvent {
        id,
        race_session_id: race_session_id.to_owned(),
        rig_id: rig_id.to_owned(),
        event_type,
        timestamp,
        session_time: frame.session_time as f64,
        session_tick: frame.session_tick,
        scope,
        car,
        payload,
        context,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Return the primary `car_idx` for a given event.
///
/// Battle events are keyed on the *opponent* car; all other events
/// (session, flag, lap, position) are keyed on the player's own car.
fn primary_car_idx(event: &RaceEvent, player_car_idx: u8) -> u8 {
    match event {
        RaceEvent::BattleEngaged  { opponent_car_idx, .. }
        | RaceEvent::BattleBroken  { opponent_car_idx, .. }
        | RaceEvent::BattleClosing { opponent_car_idx, .. } => *opponent_car_idx,
        RaceEvent::IncidentAlert { car_idx, .. } => *car_idx,
        _ => player_car_idx,
    }
}

fn event_car(
    event: &RaceEvent,
    player_car_idx: u8,
    roster: Option<&SessionRoster>,
) -> Option<CarRef> {
    match event.event_scope() {
        EventScope::SessionScoped | EventScope::RigScoped => None,
        EventScope::CarScoped => Some(resolve_car(primary_car_idx(event, player_car_idx), roster)),
    }
}

fn event_scope_label(scope: EventScope) -> &'static str {
    match scope {
        EventScope::CarScoped => "CAR_SCOPED",
        EventScope::RigScoped => "RIG_SCOPED",
        EventScope::SessionScoped => "SESSION_SCOPED",
    }
}

/// Resolve a [`CarRef`] from the roster.
///
/// Falls back to a minimal stub when the roster is unavailable or the slot
/// is not yet populated.
fn resolve_car(car_idx: u8, roster: Option<&SessionRoster>) -> CarRef {
    roster
        .and_then(|r| r.lookup(car_idx))
        .cloned()
        .unwrap_or_else(|| CarRef {
            car_idx,
            car_number: car_idx.to_string(),
            driver_name: String::new(),
            team_name: None,
            car_class_short_name: None,
            car_class_id: None,
            user_id: None,
            irating: None,
            lic_string: None,
            flair_name: None,
        })
}

fn enrich_payload(
    payload: &mut Value,
    race_event: &RaceEvent,
    frame: &TelemetryFrame,
    roster: Option<&SessionRoster>,
) {
    let Some(obj) = payload.as_object_mut() else {
        return;
    };

    obj.insert(
        "eventScope".to_owned(),
        Value::String(event_scope_label(race_event.event_scope()).to_owned()),
    );

    match race_event {
        RaceEvent::LapCompleted { lap_time_s, best_lap_time_s, .. } => {
            obj.insert("lapTime".to_owned(), option_f32_json(*lap_time_s));
            obj.insert("bestLapTime".to_owned(), option_f32_json(*best_lap_time_s));
        }
        RaceEvent::BattleEngaged { player_car_idx, opponent_car_idx, gap_s, engagement_started_at_session_time_s, .. } => {
            let (leader_idx, follower_idx) =
                leader_follower_indices(frame, *player_car_idx, *opponent_car_idx);
            let leader = resolve_car(leader_idx, roster);
            let follower = resolve_car(follower_idx, roster);

            // Legacy fields for transition window
            obj.insert("leaderCarNumber".to_owned(), Value::String(leader.car_number.clone()));
            obj.insert("followerCarNumber".to_owned(), Value::String(follower.car_number.clone()));

            // New structured car references (primary source of truth)
            obj.insert("leaderCar".to_owned(), serde_json::to_value(&leader).unwrap_or(Value::Null));
            obj.insert("followerCar".to_owned(), serde_json::to_value(&follower).unwrap_or(Value::Null));

            // Gap at engagement (sanitized) and engagement start time (camelCase aliases)
            obj.insert("engagementGapSec".to_owned(), sanitize_sentinel_json(*gap_s));
            obj.insert("engagementStartedAtSessionTime".to_owned(), json!(engagement_started_at_session_time_s));
        }
        RaceEvent::BattleBroken { player_car_idx, opponent_car_idx, final_gap_sec, engagement_started_at_session_time_s, session_time, .. } => {
            let (leader_idx, follower_idx) =
                leader_follower_indices(frame, *player_car_idx, *opponent_car_idx);
            let leader = resolve_car(leader_idx, roster);
            let follower = resolve_car(follower_idx, roster);

            // Legacy fields for transition window
            obj.insert("leaderCarNumber".to_owned(), Value::String(leader.car_number.clone()));
            obj.insert("followerCarNumber".to_owned(), Value::String(follower.car_number.clone()));

            // New structured car references (primary source of truth)
            obj.insert("leaderCar".to_owned(), serde_json::to_value(&leader).unwrap_or(Value::Null));
            obj.insert("followerCar".to_owned(), serde_json::to_value(&follower).unwrap_or(Value::Null));

            // Final gap (None when the gap was a sentinel / car no longer visible)
            obj.insert("finalGapSec".to_owned(), option_f32_json(*final_gap_sec));
            let duration = (session_time - engagement_started_at_session_time_s).max(0.0);
            obj.insert("engagementDurationSec".to_owned(), json!(duration));
        }
        RaceEvent::BattleClosing { player_car_idx, opponent_car_idx, .. } => {
            let (leader_idx, follower_idx) =
                leader_follower_indices(frame, *player_car_idx, *opponent_car_idx);
            let leader = resolve_car(leader_idx, roster);
            let follower = resolve_car(follower_idx, roster);
            
            // Legacy fields for transition window
            obj.insert("leaderCarNumber".to_owned(), Value::String(leader.car_number.clone()));
            obj.insert("followerCarNumber".to_owned(), Value::String(follower.car_number.clone()));
            
            // New structured car references (primary source of truth)
            obj.insert("leaderCar".to_owned(), serde_json::to_value(&leader).unwrap_or(Value::Null));
            obj.insert("followerCar".to_owned(), serde_json::to_value(&follower).unwrap_or(Value::Null));
        }
        RaceEvent::Overtake { car_idx, overtaken_car_idx, .. } => {
            let overtaking_car = resolve_car(*car_idx, roster);
            
            // New structured car references (primary source of truth)
            obj.insert("overtakingCar".to_owned(), serde_json::to_value(&overtaking_car).unwrap_or(Value::Null));
            
            if let Some(overtaken_idx) = overtaken_car_idx {
                let overtaken_car = resolve_car(*overtaken_idx, roster);
                obj.insert("overtakenCar".to_owned(), serde_json::to_value(&overtaken_car).unwrap_or(Value::Null));
            }
        }
        RaceEvent::OvertakeForLead { car_idx, overtaken_car_idx, .. } => {
            let overtaking_car = resolve_car(*car_idx, roster);
            
            // New structured car references (primary source of truth)
            obj.insert("overtakingCar".to_owned(), serde_json::to_value(&overtaking_car).unwrap_or(Value::Null));
            
            if let Some(overtaken_idx) = overtaken_car_idx {
                let overtaken_car = resolve_car(*overtaken_idx, roster);
                obj.insert("overtakenCar".to_owned(), serde_json::to_value(&overtaken_car).unwrap_or(Value::Null));
            }
        }
        RaceEvent::TrafficIntercept { traffic_car_idx, .. } => {
            let traffic_car = resolve_car(*traffic_car_idx, roster);
            obj.insert("trafficCar".to_owned(), serde_json::to_value(&traffic_car).unwrap_or(Value::Null));
        }
        RaceEvent::HorizonClosing { attacker_car_idx, defender_car_idx, .. } => {
            let attacker_car = resolve_car(*attacker_car_idx, roster);
            let defender_car = resolve_car(*defender_car_idx, roster);
            obj.insert("attackerCar".to_owned(), serde_json::to_value(&attacker_car).unwrap_or(Value::Null));
            obj.insert("defenderCar".to_owned(), serde_json::to_value(&defender_car).unwrap_or(Value::Null));
        }
        RaceEvent::IncidentCluster { car_idxs, primary_car_idx, incident_type, lap_dist_pct_from, lap_dist_pct_to, .. } => {
            // Resolve all involved cars to a CarRef array
            let involved_cars: Vec<Value> = car_idxs
                .iter()
                .map(|&idx| serde_json::to_value(&resolve_car(idx, roster)).unwrap_or(Value::Null))
                .collect();
            obj.insert("involvedCars".to_owned(), Value::Array(involved_cars));

            // Resolve primary car (most-culpable)
            let primary = primary_car_idx.map(|idx| serde_json::to_value(&resolve_car(idx, roster)).unwrap_or(Value::Null)).unwrap_or(Value::Null);
            obj.insert("primaryCar".to_owned(), primary);

            // Incident classification
            let itype = incident_type.as_deref().map(Value::from).unwrap_or(Value::Null);
            obj.insert("incidentType".to_owned(), itype);

            // Cluster centroid lap distance percentage
            obj.insert("lapDistPct".to_owned(), json!((lap_dist_pct_from + lap_dist_pct_to) / 2.0));
        }
        RaceEvent::IncidentAlert { driver_incident_count, previous_track_surface, current_track_surface, previous_speed_mps, current_speed_mps, speed_drop_mps, severity, reason, .. } => {
            obj.insert(
                "driverIncidentCount".to_owned(),
                driver_incident_count.map(Value::from).unwrap_or(Value::Null),
            );
            obj.insert("previousTrackSurface".to_owned(), json!(previous_track_surface));
            obj.insert("currentTrackSurface".to_owned(), json!(current_track_surface));
            obj.insert("previousSpeedMps".to_owned(), json!(previous_speed_mps));
            obj.insert("currentSpeedMps".to_owned(), json!(current_speed_mps));
            obj.insert("speedDropMps".to_owned(), json!(speed_drop_mps));
            obj.insert("severityScore".to_owned(), json!(severity));
            obj.insert("reason".to_owned(), Value::String(reason.clone()));
        }
        RaceEvent::FlagYellowLocal { trigger_car_idx, lap_dist_pct, scope, .. } => {
            // Resolve trigger car to a structured CarRef when known.
            if let Some(idx) = trigger_car_idx {
                let trigger_car = resolve_car(*idx, roster);
                obj.insert("triggerCar".to_owned(), serde_json::to_value(&trigger_car).unwrap_or(Value::Null));
            }
            // Format track location as a human-readable percentage string, reused in the reason below.
            let location_pct_str = lap_dist_pct.map(|p| format!("{:.1}%", p * 100.0));
            if let Some(ref s) = location_pct_str {
                obj.insert("trackLocationPct".to_owned(), Value::String(s.clone()));
            }
            // Flag scope as a string matching the FlagScope enum variant name.
            let scope_str = match scope {
                FlagScope::SelfCaused  => "SelfCaused",
                FlagScope::Nearby      => "Nearby",
                FlagScope::SessionWide => "SessionWide",
                FlagScope::Unknown     => "Unknown",
            };
            obj.insert("flagScope".to_owned(), Value::String(scope_str.to_owned()));
            // Human-readable summary for narration.
            let reason = match scope {
                FlagScope::SelfCaused  => "Incident caused by player".to_owned(),
                FlagScope::Nearby      => {
                    let loc = location_pct_str.as_deref().unwrap_or("unknown location");
                    format!("Incident nearby at {loc}")
                }
                FlagScope::SessionWide => "Session-wide caution".to_owned(),
                FlagScope::Unknown     => "Yellow flag condition".to_owned(),
            };
            obj.insert("reason".to_owned(), Value::String(reason));
        }
        _ => {}
    }
}

/// Threshold above which a raw f32 telemetry value is treated as a
/// missing-data sentinel (e.g. iRacing's `f32::MAX` / `3.4e+38`).
const F32_SENTINEL_THRESHOLD: f32 = 1e30;

fn option_f32_json(v: Option<f32>) -> Value {
    match v {
        Some(n) => json!(n),
        None => Value::Null,
    }
}

/// Convert a raw f32 telemetry value to JSON, mapping sentinel values
/// (NaN, Infinite, or values >= `F32_SENTINEL_THRESHOLD` such as `f32::MAX`)
/// to `null` to prevent invalid data from reaching Cosmos.
fn sanitize_sentinel_json(v: f32) -> Value {
    if v.is_nan() || v.is_infinite() || v >= F32_SENTINEL_THRESHOLD {
        if cfg!(debug_assertions) {
            eprintln!("[publisher] sentinel value detected in telemetry: {v}");
        }
        Value::Null
    } else {
        json!(v)
    }
}

fn leader_follower_indices(frame: &TelemetryFrame, player_idx: u8, opponent_idx: u8) -> (u8, u8) {
    let player_pos = frame
        .car_idx_position
        .get(player_idx as usize)
        .copied()
        .filter(|p| *p > 0);
    let opponent_pos = frame
        .car_idx_position
        .get(opponent_idx as usize)
        .copied()
        .filter(|p| *p > 0);

    match (player_pos, opponent_pos) {
        (Some(pp), Some(op)) if op < pp => (opponent_idx, player_idx),
        (Some(pp), Some(op)) if pp < op => (player_idx, opponent_idx),
        _ => (player_idx, opponent_idx),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battle_state::SlopeInfo;
    use crate::race_event::RaceEvent;
    use crate::session_info::{CarRef, SessionMetadata, SessionRoster};
    use crate::telemetry_frame::TelemetryFrame;

    fn minimal_frame() -> TelemetryFrame {
        TelemetryFrame {
            lap: 4,
            session_time: 1234.5,
            lap_dist_pct: 0.5,
            player_car_idx: 0,
            player_car_position: 5,
            on_pit_road: false,
            session_flags: 0,
            car_idx_lap_dist_pct: vec![0.5, 0.51],
            car_idx_position: vec![5, 4],
            car_idx_on_pit_road: vec![false, false],
            car_idx_track_surface: vec![0, 0],
            lap_last_lap_time: 540.0,
            session_info_update: 1,
            session_tick: 9876,
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

    fn enriched_roster() -> SessionRoster {
        SessionRoster::from_cars(vec![
            CarRef {
                car_idx: 0,
                car_number: "42".to_owned(),
                driver_name: "Player".to_owned(),
                team_name: Some("Team A".to_owned()),
                car_class_short_name: Some("BMW M2".to_owned()),
                car_class_id: Some(4073),
                user_id: Some(123456),
                irating: Some(2680),
                lic_string: Some("D 2.79".to_owned()),
                flair_name: Some("United States".to_owned()),
            },
            CarRef {
                car_idx: 1,
                car_number: "7".to_owned(),
                driver_name: "Opponent".to_owned(),
                team_name: Some("Team B".to_owned()),
                car_class_short_name: Some("BMW M2".to_owned()),
                car_class_id: Some(4073),
                user_id: Some(999111),
                irating: Some(1400),
                lic_string: Some("R 2.50".to_owned()),
                flair_name: Some("Spain".to_owned()),
            },
        ])
    }

    #[test]
    fn battle_closing_json_shape() {
        let event = RaceEvent::BattleClosing {
            lap: 4,
            session_time: 1234.5,
            player_car_idx: 0,
            opponent_car_idx: 1,
            car_race_position: 3,
            closing_rate_sec_per_lap: 0.43,
            slope_info: SlopeInfo {
                median_slope: -0.43,
                anchors_qualifying: 5,
                anchors_agreeing: 4,
                hotspot_lap_dist_pct: 0.62,
            },
            prior_skirmishes: 0,
            prior_attack_time_s: 0.0,
        };

        let env = build_event(&event, &minimal_frame(), None, "session-abc", "rig-001", None, None);
        let json: Value = serde_json::to_value(&env).unwrap();

        // Envelope-level fields
        assert!(json["id"].as_str().map(|s| s.len() == 36).unwrap_or(false),
            "id should be a UUID string");
        assert_eq!(json["type"], "BATTLE_CLOSING");
        assert_eq!(json["raceSessionId"], "session-abc");
        assert_eq!(json["rigId"], "rig-001");
        assert_eq!(json["sessionTime"], 1234.5_f64);
        assert_eq!(json["sessionTick"], 9876_i64);

        // Car fallback (no roster)
        assert_eq!(json["car"]["carIdx"], 1);  // opponent car_idx, not player

        // Payload contains event-specific fields (field names are snake_case)
        let rate = json["payload"]["closing_rate_sec_per_lap"].as_f64().unwrap_or(0.0);
        assert!((rate - 0.43).abs() < 1e-4, "expected ~0.43, got {rate}");
        assert_eq!(json["payload"]["opponent_car_idx"], 1);
        assert_eq!(json["payload"]["player_car_idx"], 0);
        assert_eq!(json["payload"]["lap"], 4);
        assert!(json["payload"].get("event_type").is_none(),
            "event_type should be hoisted out of payload");

        // Context block
        assert_eq!(json["context"]["leaderLap"], 3);
        assert_eq!(json["context"]["sessionState"], 4);
        assert_eq!(json["context"]["sessionFlags"], 0);
        assert_eq!(json["context"]["sessionNum"], 0);
    }

    #[test]
    fn uuid_is_unique_across_calls() {
        let event = RaceEvent::RaceGreen { lap: 1, session_time: 0.0 };
        let frame = minimal_frame();
        let e1 = build_event(&event, &frame, None, "s", "r", None, None);
        let e2 = build_event(&event, &frame, None, "s", "r", None, None);
        assert_ne!(e1.id, e2.id);
    }

    #[test]
    fn session_events_omit_car_and_use_session_scope() {
        let event = RaceEvent::RaceGreen { lap: 1, session_time: 0.0 };
        let frame = minimal_frame(); // player_car_idx = 0
        let env = build_event(&event, &frame, None, "s", "r", None, None);
        let json: Value = serde_json::to_value(&env).unwrap();
        assert_eq!(env.scope, EventScope::SessionScoped);
        assert!(env.car.is_none());
        assert!(json.get("car").is_none());
        assert_eq!(json["scope"], "SESSION_SCOPED");
        assert_eq!(json["payload"]["eventScope"], "SESSION_SCOPED");
    }

    #[test]
    fn publisher_lifecycle_events_use_rig_scope() {
        let event = RaceEvent::PublisherHello {
            lap: 1,
            session_time: 0.0,
            version: "0.1.0".to_owned(),
            scope: "driver".to_owned(),
        };
        let env = build_event(&event, &minimal_frame(), None, "s", "r", None, None);
        let json: Value = serde_json::to_value(&env).unwrap();
        assert_eq!(env.scope, EventScope::RigScoped);
        assert!(env.car.is_none());
        assert_eq!(json["scope"], "RIG_SCOPED");
        assert_eq!(json["payload"]["eventScope"], "RIG_SCOPED");
    }

    #[test]
    fn battle_payload_includes_leader_and_follower_car_numbers() {
        let event = RaceEvent::BattleEngaged {
            lap: 2,
            session_time: 12.0,
            player_car_idx: 0,
            opponent_car_idx: 1,
            gap_s: 0.4,
            car_race_position: 4,
            prior_skirmishes: 0,
            prior_attack_time_s: 0.0,
            engagement_started_at_session_time_s: 12.0,
        };

        let env = build_event(&event, &minimal_frame(), None, "s", "r", None, None);
        let json: Value = serde_json::to_value(&env).unwrap();
        assert_eq!(json["payload"]["leaderCarNumber"], "1");
        assert_eq!(json["payload"]["followerCarNumber"], "0");
    }

    #[test]
    fn lap_completed_payload_includes_camel_case_aliases() {
        let event = RaceEvent::LapCompleted {
            lap: 2,
            session_time: 99.0,            player_car_idx: 0,            lap_time_s: Some(88.2),
            best_lap_time_s: Some(87.9),
            position: 5,
            pit_frames: 0,
        };

        let env = build_event(&event, &minimal_frame(), None, "s", "r", None, None);
        let json: Value = serde_json::to_value(&env).unwrap();
        let lap_time = json["payload"]["lapTime"].as_f64().unwrap_or_default();
        let best_lap = json["payload"]["bestLapTime"].as_f64().unwrap_or_default();
        let lap_time_snake = json["payload"]["lap_time_s"].as_f64().unwrap_or_default();
        let best_lap_snake = json["payload"]["best_lap_time_s"].as_f64().unwrap_or_default();
        assert!((lap_time - 88.2).abs() < 1e-3);
        assert!((best_lap - 87.9).abs() < 1e-3);
        assert!((lap_time_snake - 88.2).abs() < 1e-3);
        assert!((best_lap_snake - 87.9).abs() < 1e-3);
    }

    #[test]
    fn car_refs_include_enrichment_fields_when_roster_present() {
        let event = RaceEvent::Overtake {
            lap: 3,
            session_time: 42.0,
            car_idx: 0,
            overtaken_car_idx: Some(1),
            position_from: 6,
            position_to: 5,
            positions_gained: 1,
        };
        let frame = minimal_frame();
        let roster = enriched_roster();

        let env = build_event(&event, &frame, Some(&roster), "s", "r", None, None);
        let json: Value = serde_json::to_value(&env).unwrap();
        assert_eq!(json["payload"]["overtakingCar"]["irating"], 2680);
        assert_eq!(json["payload"]["overtakingCar"]["licString"], "D 2.79");
        assert_eq!(json["payload"]["overtakingCar"]["flairName"], "United States");
    }

    #[test]
    fn context_includes_session_metadata_when_provided() {
        let event = RaceEvent::RaceGreen { lap: 1, session_time: 1.0 };
        let frame = minimal_frame();
        let meta = SessionMetadata {
            track_name: Some("Winton National".to_owned()),
            session_type: Some("Practice".to_owned()),
            session_laps: Some("unlimited".to_owned()),
        };

        let env = build_event(&event, &frame, None, "s", "r", Some(&meta), Some(86268796));
        let json: Value = serde_json::to_value(&env).unwrap();
        assert_eq!(json["context"]["subSessionId"], 86268796);
        assert_eq!(json["context"]["sessionType"], "Practice");
        assert_eq!(json["context"]["trackName"], "Winton National");
    }
}
