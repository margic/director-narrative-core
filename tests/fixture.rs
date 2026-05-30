use director_narrative_core::race_event::RaceEvent;
use director_narrative_core::replay::replay_frames;
use director_narrative_core::telemetry_frame::TelemetryFrame;

fn load_fixture() -> Vec<TelemetryFrame> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/test_fixture.jsonl");
    let content = std::fs::read_to_string(path)
        .expect("test fixture missing — run: python3 scripts/synthesize_test_fixture.py");
    content
        .lines()
        .filter(|l| !l.is_empty())
        .enumerate()
        .map(|(i, line)| {
            serde_json::from_str(line).unwrap_or_else(|e| panic!("line {}: {e}", i + 1))
        })
        .collect()
}

#[test]
fn lap3_push_car7() {
    let frames = load_fixture();
    let events = replay_frames(&frames);

    let battle_closing = events.iter().find(|e| {
        matches!(e, RaceEvent::BattleClosing { car_idx: 7, .. })
    });
    assert!(battle_closing.is_some(), "expected a BATTLE_CLOSING event for car_idx=7");

    if let Some(RaceEvent::BattleClosing { lap, slope_info, .. }) = battle_closing {
        assert_eq!(*lap, 3, "BATTLE_CLOSING should first be emitted at lap 3, got lap {lap}");
        assert!(
            (slope_info.median_slope - (-0.4998)).abs() < 0.01,
            "expected slope ≈ -0.500 s/lap, got {:.4}",
            slope_info.median_slope,
        );
    }
}

#[test]
fn lap4_attack_setup_car7() {
    let frames = load_fixture();
    let events = replay_frames(&frames);

    // BATTLE_CLOSING covers both Push and AttackSetup phases.
    // The second BATTLE_CLOSING for car 7 should be at lap 4 with a steeper slope.
    let attack = events
        .iter()
        .filter(|e| matches!(e, RaceEvent::BattleClosing { car_idx: 7, .. }))
        .nth(1);
    assert!(attack.is_some(), "expected a second BATTLE_CLOSING event for car_idx=7 (AttackSetup phase)");

    if let Some(RaceEvent::BattleClosing { lap, slope_info, .. }) = attack {
        assert_eq!(*lap, 4, "second BATTLE_CLOSING should be emitted at lap 4, got lap {lap}");
        assert!(
            (slope_info.median_slope - (-0.5999)).abs() < 0.01,
            "expected slope ≈ -0.600 s/lap, got {:.4}",
            slope_info.median_slope,
        );
    }
}

#[test]
fn close_approach_car7() {
    let frames = load_fixture();
    let events = replay_frames(&frames);

    let close = events.iter().find(|e| {
        matches!(e, RaceEvent::BattleEngaged { car_idx: 7, .. })
    });
    assert!(close.is_some(), "expected a BATTLE_ENGAGED event for car_idx=7");

    if let Some(RaceEvent::BattleEngaged { lap, gap_s, .. }) = close {
        assert!(
            *lap >= 5 && *lap <= 7,
            "BATTLE_ENGAGED should be around lap 6, got lap {lap}",
        );
        assert!(
            *gap_s < CLOSE_APPROACH_THRESH_S,
            "gap should be < {CLOSE_APPROACH_THRESH_S} s, got {gap_s}",
        );
    }
}

use director_narrative_core::battle_state::CLOSE_APPROACH_THRESH_S;
