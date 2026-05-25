# Narrative Engine — Test Harness

**Status:** Python prototype validated; Rust harness design specified  
**Companion documents:** [architecture.md](architecture.md) · [data-models.md](data-models.md)

---

## 1. Testing Philosophy

The engine must produce **deterministic, reproducible narrative events** from a given telemetry stream. The correctness criteria are race-domain facts — "does the engine fire `PUSH_DETECTED` on the right lap, for the right opponent, at the right slope?" — not just "does the Rust compile without warnings?"

The test strategy has three tiers:

| Tier | What is tested | Tool |
|---|---|---|
| Unit | OLS regression math, AnchorSampler bucket logic, LapTimer duration | `cargo test` |
| Scenario | Full pipeline: JSONL stream → expected events sequence | `cargo test` + JSONL fixtures |
| Regression | Real Nürburgring session produces identical event timeline to Python prototype | `cargo test` with `nurburgring_5lap.jsonl` fixture |

---

## 2. JSONL Fixture Format

A `.jsonl` fixture is a sequence of newline-delimited JSON objects. Each line is one `TelemetryFrame` serialised to the same schema the Python prototype reads from `data/session.jsonl`:

```json
{"session_time": 65.4, "lap": 1, "lap_dist_pct": 0.02341, "lap_last_lap_time": 0.0, "player_car_idx": 18, "player_car_position": 15, "on_pit_road": false, "session_flags": 0, "car_idx_lap_dist_pct": [0.241, 0.187, ...], "car_idx_position": [3, 7, ...], "car_idx_on_pit_road": [false, false, ...], "car_idx_f2_time": [0.0, 0.0, ...]}
```

All `car_idx_*` arrays must be present and have the same length (one slot per session car). Inactive car slots use the sentinel values documented in [data-models.md](data-models.md#1-input-contract--telemetryframe).

### 2.1 Generating Synthetic Fixtures

`scripts/synthesize_test_fixture.py` generates `data/test_fixture.jsonl` with a controlled gap schedule:

```bash
python3 scripts/synthesize_test_fixture.py
```

To exercise a different scenario, modify `GAP_TO_TARGET_PER_LAP` in the script. The key constraint for `ATTACK_SETUP` is that the **gap must decrease by a larger amount each successive lap** (accelerating, not decelerating close):

```python
# Correct: closing rate accelerates → PUSH then ATTACK_SETUP
GAP_TO_TARGET_PER_LAP = {
    2: 4.5,    # enters battle window
    3: 4.0,    # -0.5 s/lap → PUSH fires after this lap
    4: 3.3,    # -0.7 s/lap → ATTACK_SETUP fires (slope more negative than prev)
    5: 2.2,    # -1.1 s/lap → accelerating
    6: 0.6,    # -1.6 s/lap → CLOSE_APPROACH fires
}

# Wrong: closing rate decelerates → PUSH only, ATTACK_SETUP never fires
GAP_TO_TARGET_PER_LAP = {
    2: 4.8,    # enters window
    3: 3.2,    # -1.6 s/lap → PUSH fires
    4: 2.1,    # -1.1 s/lap → slope LESS negative → no ATTACK_SETUP
    5: 1.3,    # -0.8 s/lap → decelerating
}
```

### 2.2 Fixture Inventory

| Fixture | Description | Expected events |
|---|---|---|
| `data/test_fixture.jsonl` | Synthetic 10-lap race, accelerating close | PUSH (lap 3), ATTACK_SETUP (lap 4), CLOSE_APPROACH (lap 6), OVERTAKE (lap 8), PIT (lap 9) |
| `data/session.jsonl` | Real Nürburgring race (47 cars, 5 laps) | 8× CLOSE_APPROACH, 4× LAP_COMPLETE, 2× OVERTAKE, 1× PIT_ENTRY, 1× PIT_EXIT, 0× PUSH/ATTACK |
| `tests/fixtures/synthetic_yellow.jsonl` | Yellow flag on lap 3 contaminates 2 consecutive anchor readings | PUSH delayed to lap 5 (not lap 3) due to dirty reading exclusion |
| `tests/fixtures/synthetic_opponent_change.jsonl` | Car ahead pits at lap 4, new opponent takes position | PUSH/ATTACK series resets at lap 4; new series starts from lap 5 |

---

## 3. Running the Python Prototype (Reference Implementation)

Before implementing the Rust engine, validate the logic against the Python prototype:

```bash
# Against the real Nürburgring data
python3 scripts/prototype_narrative.py

# Against the synthetic fixture
JSONL_PATH=data/test_fixture.jsonl python3 scripts/prototype_narrative.py
```

Expected output for the synthetic fixture:

```
[08:00  L03]  LAP_COMPLETE  time=140.0s  P15→P14
[08:00  L03]  OVERTAKE    P15→P14  (+1)
[08:00  L03]  PUSH  slope=-0.4998s/lap  n=28  hotspot@29%
[10:20  L04]  ATTACK_SETUP  slope=-0.5999s/lap  n=28  hotspot@29%
[12:40  L06]  CLOSE_APPROACH  CarIdx 7 (P10) @ 0.60s
[19:40  L08]  OVERTAKE    P10→P9  (+1)
[19:42  L09]  PIT_ENTRY   P8
[19:47  L09]  PIT_EXIT    P8
[22:00  L09]  LAP_COMPLETE  time=140.0s  P9→P8  [PIT]  → IDLE
```

The Rust implementation must reproduce this exact event sequence from the same JSONL input.

---

## 4. Rust Unit Tests

### 4.1 OLS Regression (`tests/ols_test.rs`)

```rust
#[cfg(test)]
mod ols_tests {
    use director_narrative_core::math::ols::ols_slope;

    #[test]
    fn slope_of_perfect_line() {
        // gap decreasing 0.5 s per lap
        let laps = vec![2, 3, 4];
        let gaps = vec![4.5, 4.0, 3.5];
        let slope = ols_slope(&laps, &gaps).unwrap();
        assert!((slope - (-0.5)).abs() < 1e-4, "slope={slope}");
    }

    #[test]
    fn slope_of_two_points_returns_exact_delta() {
        let laps = vec![1, 2];
        let gaps = vec![3.0, 2.0];
        let slope = ols_slope(&laps, &gaps).unwrap();
        assert!((slope - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn fewer_than_two_points_returns_none() {
        assert!(ols_slope(&[1], &[2.5]).is_none());
        assert!(ols_slope(&[], &[]).is_none());
    }

    #[test]
    fn zero_variance_in_lap_returns_none() {
        // All readings on the same lap — can't fit a line
        let laps = vec![3, 3, 3];
        let gaps = vec![1.0, 2.0, 3.0];
        assert!(ols_slope(&laps, &gaps).is_none());
    }
}
```

### 4.2 AnchorSampler (`tests/anchor_detector_test.rs`)

```rust
#[test]
fn first_crossing_only_per_lap_bucket() {
    let mut sampler = AnchorSampler::new(10);  // 10 buckets

    // Lap 1, ldp=0.05 → bucket 0
    let recorded = sampler.update(1, 0.05, 2.0, 7, true);
    assert!(recorded);

    // Same lap, same bucket again — should be dropped
    let recorded = sampler.update(1, 0.08, 1.8, 7, true);
    assert!(!recorded);

    // Same bucket, different lap — should record
    let recorded = sampler.update(2, 0.05, 1.7, 7, true);
    assert!(recorded);

    assert_eq!(sampler.samples.len(), 2);
}

#[test]
fn dirty_samples_stored_but_not_used_in_regression() {
    let mut sampler = AnchorSampler::new(10);
    sampler.update(1, 0.15, 3.0, 7, false);  // dirty
    sampler.update(2, 0.15, 2.5, 7, true);   // clean

    let mut store = RegressionStore::new();
    store.ingest(&sampler, Some(2));

    // (bucket=1, car=7) has only 1 clean reading → no qualifying slope
    let slopes = store.per_bucket_slopes(2);
    assert!(slopes.is_empty(), "Expected no qualifying slopes with 1 clean reading");
}
```

### 4.3 Lap Boundary `max_lap` Guard (`tests/regression_test.rs`)

```rust
#[test]
fn max_lap_excludes_first_frame_of_new_lap() {
    // Simulate the first-frame lookahead bug: sampler has laps 1, 2, AND
    // the first frame of lap 3 when the "lap 2 complete" event fires.
    let mut sampler = AnchorSampler::new(4);
    sampler.update(1, 0.1, 3.0, 7, true);
    sampler.update(2, 0.1, 2.5, 7, true);
    sampler.update(3, 0.1, 2.1, 7, true);  // first frame of lap 3 — must be excluded

    let mut store = RegressionStore::new();
    store.ingest(&sampler, Some(2));  // max_lap=2

    // Should only see laps 1 and 2 in the regression
    let points = store.points_for(1, 7).unwrap();  // bucket=1, car=7
    assert_eq!(points.len(), 2);
    assert_eq!(points[0].lap, 1);
    assert_eq!(points[1].lap, 2);
}
```

---

## 5. Rust Scenario Tests

### 5.1 Full Pipeline: PUSH Detection

```rust
#[test]
fn push_fires_on_sustained_closing() {
    let frames = load_fixture("tests/fixtures/synthetic_push.jsonl");
    let config = AnchorDetectorConfig::default();
    let mut engine = TelemetryEngine::new();
    engine.register_detector(Box::new(DynamicAnchorDetector::new(config)));

    let mut all_events: Vec<RaceEvent> = Vec::new();
    for frame in frames {
        all_events.extend(engine.process_tick(frame));
    }

    // Exactly one PUSH_DETECTED event
    let push_events: Vec<&RaceEvent> = all_events.iter()
        .filter(|e| e.event_type == "PUSH_DETECTED")
        .collect();
    assert_eq!(push_events.len(), 1, "Expected exactly 1 PUSH_DETECTED");

    let push = push_events[0];
    assert_eq!(push.lap, 3, "PUSH expected on lap 3");

    let slope = push.narrative_context.closing_rate_s_per_lap.unwrap();
    assert!(slope < -0.04, "Slope {slope} should be < -0.04 s/lap");
    assert!(slope > -1.0, "Slope {slope} implausibly steep — check fixture");
}
```

### 5.2 Full Pipeline: ATTACK_SETUP Requires Acceleration

```rust
#[test]
fn attack_setup_requires_accelerating_slope() {
    let frames = load_fixture("tests/fixtures/synthetic_attack.jsonl");
    // ... engine setup ...

    let push_events: Vec<&RaceEvent> = all_events.iter()
        .filter(|e| e.event_type == "PUSH_DETECTED").collect();
    let attack_events: Vec<&RaceEvent> = all_events.iter()
        .filter(|e| e.event_type == "ATTACK_SETUP").collect();

    // PUSH must precede ATTACK_SETUP
    assert!(!push_events.is_empty(), "Expected PUSH before ATTACK_SETUP");
    assert!(!attack_events.is_empty(), "Expected ATTACK_SETUP on accelerating close");
    assert!(push_events[0].session_time < attack_events[0].session_time);

    // Acceleration: attack slope must be more negative than push slope
    let push_slope  = push_events[0].narrative_context.closing_rate_s_per_lap.unwrap();
    let attack_slope = attack_events[0].narrative_context.closing_rate_s_per_lap.unwrap();
    assert!(attack_slope < push_slope,
        "Attack slope {attack_slope} must be more negative than push slope {push_slope}");
}
```

### 5.3 Yellow Flag Invalidation

```rust
#[test]
fn yellow_flag_lap_excluded_from_regression() {
    // synthetic_yellow.jsonl: lap 3 has session_flags=0x100 for 40% of the lap
    // PUSH should fire on lap 4 (laps 2 and 4 clean) not lap 3
    let frames = load_fixture("tests/fixtures/synthetic_yellow.jsonl");
    // ...

    let push_events: Vec<&RaceEvent> = all_events.iter()
        .filter(|e| e.event_type == "PUSH_DETECTED").collect();
    assert!(!push_events.is_empty());
    assert!(push_events[0].lap >= 4,
        "PUSH should not fire on contaminated lap 3; fired on lap {}", push_events[0].lap);
}
```

### 5.4 Opponent Identity Reset

```rust
#[test]
fn opponent_change_resets_regression_series() {
    // synthetic_opponent_change.jsonl:
    //   Laps 1-3: car_ahead = CarIdx 7 (gap closing)
    //   Lap 4: CarIdx 7 pits; new car_ahead = CarIdx 12 (gap opens)
    //   Laps 5-7: car_ahead = CarIdx 12 (gap closing again)
    // Expected: BATTLE_RESET_OPPONENT event at lap 4; PUSH fires at lap 6 for CarIdx 12
    let frames = load_fixture("tests/fixtures/synthetic_opponent_change.jsonl");
    // ...

    let reset_events: Vec<&RaceEvent> = all_events.iter()
        .filter(|e| e.event_type == "BATTLE_RESET_OPPONENT").collect();
    assert_eq!(reset_events.len(), 1, "Expected 1 opponent reset");
    assert_eq!(reset_events[0].lap, 4);

    let push_events: Vec<&RaceEvent> = all_events.iter()
        .filter(|e| e.event_type == "PUSH_DETECTED").collect();
    // PUSH for lap 6 CarIdx 12; NOT a spurious PUSH from the lap 1-3 CarIdx 7 series
    assert!(push_events.iter().all(|e| {
        e.narrative_context.opponent_car_idx == Some(12)
    }), "All PUSH events should be against CarIdx 12 after reset");
}
```

### 5.5 Regression Against Real Data

```rust
#[test]
fn nurburgring_real_session_matches_prototype_output() {
    // data/session.jsonl — 9985 frames, 5 laps, 47 cars
    // This is the canonical regression test. The Rust output must match
    // the Python prototype's event timeline exactly.
    let frames = load_fixture("data/session.jsonl");
    // ...

    // Event counts (from prototype run)
    let close_approach_count = all_events.iter().filter(|e| e.event_type == "CLOSE_APPROACH").count();
    let overtake_count       = all_events.iter().filter(|e| e.event_type == "OVERTAKE_DETECTED").count();
    let pit_entry_count      = all_events.iter().filter(|e| e.event_type == "PIT_ENTRY").count();
    let push_count           = all_events.iter().filter(|e| e.event_type == "PUSH_DETECTED").count();

    assert_eq!(close_approach_count, 8,  "8 CLOSE_APPROACH events (per prototype)");
    assert_eq!(overtake_count,       2,  "2 OVERTAKE events: +8 lap 2, +2 lap 3");
    assert_eq!(pit_entry_count,      1,  "1 PIT_ENTRY: lap 3");
    assert_eq!(push_count,           0,  "0 PUSH: opponent changed every lap");
}
```

---

## 6. Running the Test Suite

```bash
# All tests
cargo test

# Unit tests only (fast, no fixture I/O)
cargo test --lib

# Scenario tests only
cargo test --test anchor_detector_test
cargo test --test regression_test

# With output (useful when a test fails and you want the event list)
cargo test -- --nocapture

# Specific test
cargo test push_fires_on_sustained_closing -- --nocapture
```

The real Nürburgring fixture (`data/session.jsonl`) is 31 MB and is gitignored. In CI, download it from the designated fixture store or skip the regression test with:

```bash
cargo test --features skip_large_fixtures
```

---

## 7. Fixture Helper (`tests/common/mod.rs`)

```rust
pub mod common {
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    use director_narrative_core::models::TelemetryFrame;

    /// Load a JSONL fixture file into a Vec<TelemetryFrame>.
    /// Panics with a clear message if the file is missing or malformed.
    pub fn load_fixture(path: &str) -> Vec<TelemetryFrame> {
        let file = File::open(path)
            .unwrap_or_else(|_| panic!("Fixture not found: {path}\nRun scripts/synthesize_test_fixture.py first."));
        
        BufReader::new(file)
            .lines()
            .enumerate()
            .map(|(line_num, line)| {
                let line = line.expect("IO error reading fixture");
                serde_json::from_str(&line)
                    .unwrap_or_else(|e| panic!("Malformed JSON at {path}:{line_num}: {e}"))
            })
            .collect()
    }
}
```

---

## 8. CI Integration

The test suite is designed to run on any Linux CI agent without iRacing installed:

```yaml
# .github/workflows/test.yml
name: Test
on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      
      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
      
      - name: Generate synthetic fixtures
        run: python3 scripts/synthesize_test_fixture.py
      
      - name: Run tests
        run: cargo test

      # Large fixture (real race data) is optional — only runs if available
      - name: Run regression test (if fixture available)
        run: |
          if [ -f data/session.jsonl ]; then
            cargo test nurburgring_real_session -- --nocapture
          else
            echo "Skipping Nürburgring regression test (data/session.jsonl not present)"
          fi
```

---

## 9. Adding a New Scenario Fixture

To test a specific race situation not covered by the existing fixtures:

1. **Define the gap schedule** in `GAP_TO_TARGET_PER_LAP` in `scripts/synthesize_test_fixture.py`
2. **Run the generator** — inspect the Python prototype output to confirm the events fire as intended:
   ```bash
   python3 scripts/synthesize_test_fixture.py
   JSONL_PATH=data/test_fixture.jsonl python3 scripts/prototype_narrative.py
   ```
3. **Copy the generated `.jsonl`** to `tests/fixtures/` with a descriptive name
4. **Write the Rust test** that asserts the expected event sequence from that fixture
5. **Add the fixture to the CI workflow** so it is auto-generated if missing

The Python prototype is the ground truth. If the Rust output diverges from the Python output on the same fixture, the Rust implementation has a bug — not the spec.
