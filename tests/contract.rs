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
        player_car_idx: 0,
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
        player_car_idx: 0,
        opponent_car_idx: 1,
        gap_s: 0.4,
        car_race_position: 4,
        prior_skirmishes: 0,
        prior_attack_time_s: 0.0,
        engagement_started_at_session_time_s: 12.0,
    };

    let env = build_event(&event, &minimal_frame(), None, "session-abc", "rig-001");
    let json = normalized_event_json(&env);

    assert_envelope_contract(&json, "BATTLE_ENGAGED");
    assert_eq!(json["car"]["carIdx"], 1);
    
    // Legacy fields still present for transition
    assert_eq!(json["payload"]["leaderCarNumber"], "1");
    assert_eq!(json["payload"]["followerCarNumber"], "0");
    
    // New structured car references (primary source of truth)
    assert!(json["payload"]["leaderCar"].is_object());
    assert!(json["payload"]["followerCar"].is_object());
    assert_eq!(json["payload"]["leaderCar"]["carIdx"], 1);
    assert_eq!(json["payload"]["followerCar"]["carIdx"], 0);
}

#[test]
fn battle_closing_contract_uses_opponent_car_as_primary_identity() {
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

    let env = build_event(&event, &minimal_frame(), None, "session-abc", "rig-001");
    let json = normalized_event_json(&env);

    assert_envelope_contract(&json, "BATTLE_CLOSING");
    assert_eq!(json["car"]["carIdx"], 1);
    assert_eq!(json["payload"]["opponent_car_idx"], 1);
    assert_eq!(json["payload"]["player_car_idx"], 0);
    assert_eq!(json["context"]["leaderLap"], 3);
}

#[test]
fn overtake_includes_overtaking_and_overtaken_cars() {
    let event = RaceEvent::Overtake {
        lap: 3,
        session_time: 125.0,
        car_idx: 0,
        overtaken_car_idx: Some(2),
        position_from: 4,
        position_to: 3,
        positions_gained: 1,
    };

    let env = build_event(&event, &minimal_frame(), None, "session-abc", "rig-001");
    let json = normalized_event_json(&env);

    assert_envelope_contract(&json, "OVERTAKE");
    
    // Envelope should show player as primary car
    assert_eq!(json["car"]["carIdx"], 0);
    
    // Payload should include structured car references (primary source of truth)
    assert!(json["payload"]["overtakingCar"].is_object());
    assert!(json["payload"]["overtakenCar"].is_object());
    assert_eq!(json["payload"]["overtakingCar"]["carIdx"], 0);
    assert_eq!(json["payload"]["overtakenCar"]["carIdx"], 2);
    
    // Legacy position fields still present
    assert_eq!(json["payload"]["position_from"], 4);
    assert_eq!(json["payload"]["position_to"], 3);
}

#[test]
fn overtake_can_emit_without_overtaken_car_when_uncertain() {
    let event = RaceEvent::Overtake {
        lap: 3,
        session_time: 125.0,
        car_idx: 0,
        overtaken_car_idx: None,
        position_from: 4,
        position_to: 3,
        positions_gained: 1,
    };

    let env = build_event(&event, &minimal_frame(), None, "session-abc", "rig-001");
    let json = normalized_event_json(&env);

    assert_envelope_contract(&json, "OVERTAKE");
    
    // Legacy position fields are still present for narration
    assert_eq!(json["payload"]["position_from"], 4);
    assert_eq!(json["payload"]["position_to"], 3);
    
    // Overtaking car should still be identified
    assert!(json["payload"]["overtakingCar"].is_object());
    assert_eq!(json["payload"]["overtakingCar"]["carIdx"], 0);
    
    // Overtaken car should be null when not determined
    assert_eq!(json["payload"]["overtakenCar"], Value::Null);
}

#[test]
fn overtake_for_lead_includes_both_cars() {
    let event = RaceEvent::OvertakeForLead {
        lap: 5,
        session_time: 234.0,
        car_idx: 0,
        overtaken_car_idx: Some(1),
        position_from: 2,
        positions_gained: 1,
    };

    let env = build_event(&event, &minimal_frame(), None, "session-abc", "rig-001");
    let json = normalized_event_json(&env);

    assert_envelope_contract(&json, "OVERTAKE_FOR_LEAD");
    
    // Should identify both cars
    assert!(json["payload"]["overtakingCar"].is_object());
    assert!(json["payload"]["overtakenCar"].is_object());
    assert_eq!(json["payload"]["overtakingCar"]["carIdx"], 0);
    assert_eq!(json["payload"]["overtakenCar"]["carIdx"], 1);
}

#[test]
fn battle_events_identify_both_sides_directly() {
    let event = RaceEvent::BattleEngaged {
        lap: 2,
        session_time: 50.0,
        player_car_idx: 0,
        opponent_car_idx: 3,
        gap_s: 0.35,
        car_race_position: 5,
        prior_skirmishes: 1,
        prior_attack_time_s: 12.5,
        engagement_started_at_session_time_s: 50.0,
    };

    let env = build_event(&event, &minimal_frame(), None, "session-abc", "rig-001");
    let json = normalized_event_json(&env);

    assert_envelope_contract(&json, "BATTLE_ENGAGED");
    
    // Payload should include structured car references for both battle participants
    assert!(json["payload"]["leaderCar"].is_object());
    assert!(json["payload"]["followerCar"].is_object());
    
    // Can determine who was where without heuristics
    let leader_idx = json["payload"]["leaderCar"]["carIdx"].as_i64().unwrap_or(0) as u8;
    let follower_idx = json["payload"]["followerCar"]["carIdx"].as_i64().unwrap_or(0) as u8;
    
    // Leader and follower should be different cars
    assert_ne!(leader_idx, follower_idx);
    assert!((leader_idx == 0 && follower_idx == 3) || (leader_idx == 3 && follower_idx == 0));
}

#[test]
fn battle_engaged_includes_engagement_start_time() {
    let event = RaceEvent::BattleEngaged {
        lap: 3,
        session_time: 200.0,
        player_car_idx: 0,
        opponent_car_idx: 1,
        gap_s: 0.45,
        car_race_position: 4,
        prior_skirmishes: 0,
        prior_attack_time_s: 0.0,
        engagement_started_at_session_time_s: 200.0,
    };

    let env = build_event(&event, &minimal_frame(), None, "session-abc", "rig-001");
    let json = normalized_event_json(&env);

    assert_envelope_contract(&json, "BATTLE_ENGAGED");

    // Snake-case field present in payload
    let start_snake = json["payload"]["engagement_started_at_session_time_s"].as_f64();
    assert!(start_snake.is_some(), "engagement_started_at_session_time_s should be in payload");
    assert!((start_snake.unwrap() - 200.0).abs() < 1e-4);

    // camelCase alias added by enrichment
    let start_camel = json["payload"]["engagementStartedAtSessionTime"].as_f64();
    assert!(start_camel.is_some(), "engagementStartedAtSessionTime should be in payload");
    assert!((start_camel.unwrap() - 200.0).abs() < 1e-4);

    // engagementGapSec camelCase alias
    let gap = json["payload"]["engagementGapSec"].as_f64();
    assert!(gap.is_some(), "engagementGapSec should be in payload");
    assert!((gap.unwrap() - 0.45).abs() < 1e-4);
}

#[test]
fn battle_broken_contract_uses_final_gap_sec_and_duration() {
    let event = RaceEvent::BattleBroken {
        lap: 5,
        session_time: 350.0,
        player_car_idx: 0,
        opponent_car_idx: 1,
        final_gap_sec: 1.8,
        car_race_position: 4,
        engagement_started_at_session_time_s: 320.0,
    };

    let env = build_event(&event, &minimal_frame(), None, "session-abc", "rig-001");
    let json = normalized_event_json(&env);

    assert_envelope_contract(&json, "BATTLE_BROKEN");

    // Snake-case field final_gap_sec present in payload
    let final_gap_snake = json["payload"]["final_gap_sec"].as_f64();
    assert!(final_gap_snake.is_some(), "final_gap_sec should be in payload");
    assert!((final_gap_snake.unwrap() - 1.8).abs() < 1e-4);

    // camelCase alias added by enrichment
    let final_gap_camel = json["payload"]["finalGapSec"].as_f64();
    assert!(final_gap_camel.is_some(), "finalGapSec should be in payload");
    assert!((final_gap_camel.unwrap() - 1.8).abs() < 1e-4);

    // finalGapSec should never be f32::MAX
    assert!(
        final_gap_camel.unwrap() < f32::MAX as f64,
        "finalGapSec should not be f32::MAX sentinel"
    );

    // engagementDurationSec computed from start time
    let duration = json["payload"]["engagementDurationSec"].as_f64();
    assert!(duration.is_some(), "engagementDurationSec should be in payload");
    assert!((duration.unwrap() - 30.0).abs() < 1e-4, "duration should be 350.0 - 320.0 = 30.0s");

    // leaderCar and followerCar still present
    assert!(json["payload"]["leaderCar"].is_object());
    assert!(json["payload"]["followerCar"].is_object());
}