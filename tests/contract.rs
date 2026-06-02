use director_narrative_core::battle_state::SlopeInfo;
use director_narrative_core::publisher_event::{build_event, PublisherEvent};
use director_narrative_core::race_event::RaceEvent;
use director_narrative_core::telemetry_frame::TelemetryFrame;
use serde_json::Value;

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

fn normalized_event_json(event: &PublisherEvent) -> Value {
    let mut json = serde_json::to_value(event).expect("publisher event should serialise");
    let Some(obj) = json.as_object_mut() else {
        panic!("publisher event should serialise to a JSON object");
    };
    obj.insert("id".to_owned(), Value::String("<redacted>".to_owned()));
    obj.insert("timestamp".to_owned(), Value::Number(0.into()));
    json
}

fn assert_envelope_contract(json: &Value, expected_type: &str) {
    assert_eq!(json["type"].as_str(), Some(expected_type));
    assert!(json["id"].as_str().is_some());
    assert!(json["raceSessionId"].as_str().is_some());
    assert!(json["rigId"].as_str().is_some());
    assert!(json["timestamp"].is_number());
    assert!(json["sessionTime"].is_number());
    assert!(json["sessionTick"].is_i64());
    assert!(json["car"].is_object());
    assert!(json["payload"].is_object());
    assert!(json["payload"].get("event_type").is_none());
    assert!(json["context"].is_object());
}

#[test]
fn lap_completed_contract_includes_aliases_and_snake_case_fields() {
    let event = RaceEvent::LapCompleted {
        lap: 2,
        session_time: 99.0,
        lap_time_s: Some(88.2),
        best_lap_time_s: Some(87.9),
        position: 5,
        pit_frames: 0,
    };

    let env = build_event(&event, &minimal_frame(), None, "session-abc", "rig-001");
    let json = normalized_event_json(&env);

    assert_envelope_contract(&json, "LAP_COMPLETED");
    let lap_time = json["payload"]["lapTime"].as_f64().unwrap();
    let best_lap_time = json["payload"]["bestLapTime"].as_f64().unwrap();
    let lap_time_snake = json["payload"]["lap_time_s"].as_f64().unwrap();
    let best_lap_time_snake = json["payload"]["best_lap_time_s"].as_f64().unwrap();

    assert!((lap_time - 88.2).abs() < 1e-4);
    assert!((best_lap_time - 87.9).abs() < 1e-4);
    assert!((lap_time_snake - 88.2).abs() < 1e-4);
    assert!((best_lap_time_snake - 87.9).abs() < 1e-4);
}

#[test]
fn battle_contract_includes_leader_and_follower_numbers() {
    let event = RaceEvent::BattleEngaged {
        lap: 2,
        session_time: 12.0,
        car_idx: 1,
        gap_s: 0.4,
        car_race_position: 4,
        prior_skirmishes: 0,
        prior_attack_time_s: 0.0,
    };

    let env = build_event(&event, &minimal_frame(), None, "session-abc", "rig-001");
    let json = normalized_event_json(&env);

    assert_envelope_contract(&json, "BATTLE_ENGAGED");
    assert_eq!(json["car"]["carIdx"], 1);
    assert_eq!(json["payload"]["leaderCarNumber"], "1");
    assert_eq!(json["payload"]["followerCarNumber"], "0");
}

#[test]
fn battle_closing_contract_uses_opponent_car_as_primary_identity() {
    let event = RaceEvent::BattleClosing {
        lap: 4,
        session_time: 1234.5,
        car_idx: 1,
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

    let env = build_event(&event, &minimal_frame(), None, "session-abc", "rig-001");
    let json = normalized_event_json(&env);

    assert_envelope_contract(&json, "BATTLE_CLOSING");
    assert_eq!(json["car"]["carIdx"], 1);
    assert_eq!(json["payload"]["car_idx"], 1);
    assert_eq!(json["context"]["leaderLap"], 3);
}