# Narrative Engine — Test Harness

**Status:** Implemented — 17 unit tests + 3 integration tests passing
**Companion documents:** [architecture.md](architecture.md) · [data-models.md](data-models.md)

---

## 1. Testing Philosophy

The engine must produce **deterministic, reproducible narrative events** from a given telemetry stream. The correctness criteria are race-domain facts — “does the engine fire `PUSH` on the right lap, for the right opponent, at the right slope?” — not just “does the Rust compile without warnings?”

The test strategy has three tiers:

| Tier | What is tested | Tool |
|---|---|---|
| Unit | OLS regression math, AnchorSampler bucket logic, LapTimer duration | `cargo test` |
| Scenario | Full pipeline: JSONL stream → expected events sequence | `cargo test` + JSONL fixtures |
| Regression | Real Nürburgring session produces identical event timeline to Python prototype | `cargo test` (requires `data/session.jsonl` — gitignored) |

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
| `data/test_fixture.jsonl` | Synthetic 10-lap race, accelerating close | BATTLE_CLOSING (lap 3), BATTLE_CLOSING (lap 4, steeper), BATTLE_ENGAGED (lap 6), OVERTAKE (lap 8), PIT (lap 9) |
| `data/session.jsonl` | Real Nürburgring race (47 cars, 5 laps) — **gitignored** | 8× BATTLE_ENGAGED, 4× LAP_COMPLETED, 2× OVERTAKE, 1× PIT_ENTRY, 1× PIT_EXIT, 0× BATTLE_CLOSING |

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
[08:00  L03]  LAP_COMPLETED  time=140.0s  P15
[08:00  L03]  OVERTAKE    P15→P14  (+1)
[08:00  L03]  BATTLE_CLOSING  slope=-0.4998s/lap  n=28  hotspot@29%
[10:20  L04]  BATTLE_CLOSING  slope=-0.5999s/lap  n=28  hotspot@29%
[12:40  L06]  BATTLE_ENGAGED  CarIdx 7 (P10) @ 0.60s
[19:40  L08]  OVERTAKE    P10→P9  (+1)
[19:42  L09]  PIT_ENTRY   P8
[19:47  L09]  PIT_EXIT    P8
[22:00  L09]  LAP_COMPLETED  time=140.0s  P9
```

The Rust implementation must reproduce this exact event sequence from the same JSONL input.

---

## 4. Rust Unit Tests

### 4.1 OLS Regression (`src/regression_store.rs`)

The `ols_slope` function is public and directly testable:

```rust
#[cfg(test)]
mod tests {
    use super::ols_slope;

    #[test]
    fn ols_slope_known_values() {
        // gap decreasing 0.5 s per lap
        let laps = vec![2.0_f32, 3.0, 4.0];
        let gaps = vec![4.5_f32, 4.0, 3.5];
        let slope = ols_slope(&laps, &gaps).unwrap();
        assert!((slope - (-0.5)).abs() < 1e-4, "slope={slope}");
    }

    #[test]
    fn ols_slope_returns_none_for_single_point() {
        assert!(ols_slope(&[1.0], &[2.5]).is_none());
        assert!(ols_slope(&[], &[]).is_none());
    }
}
```

### 4.2 AnchorSampler (`src/anchor_sampler.rs`)

```rust
#[test]
fn first_crossing_captured() {
    let mut sampler = AnchorSampler::new(10);  // 10 buckets

    // Lap 1, ldp=0.05 → bucket 0
    let recorded = sampler.update(1, 0.05, 2.0, 7, true);
    assert!(recorded);
    assert_eq!(sampler.samples.len(), 1);
}

#[test]
fn duplicate_crossing_ignored() {
    let mut sampler = AnchorSampler::new(10);
    sampler.update(1, 0.05, 2.0, 7, true);
    // Same lap, same bucket, same car_idx — should be dropped
    let recorded = sampler.update(1, 0.08, 1.8, 7, true);
    assert!(!recorded);
    assert_eq!(sampler.samples.len(), 1);
}

#[test]
fn different_car_idx_tracked_independently() {
    let mut sampler = AnchorSampler::new(10);
    sampler.update(1, 0.05, 2.0, 7, true);
    // Same bucket, different lap — should record
    let recorded = sampler.update(2, 0.05, 1.7, 7, true);
    assert!(recorded);
    assert_eq!(sampler.samples.len(), 2);
}
```

### 4.3 RegressionStore max_lap guard (`src/regression_store.rs`)

```rust
#[test]
fn ingest_excludes_frames_beyond_max_lap() {
    // Simulate the first-frame lookahead bug: sampler has laps 1, 2, AND
    // the first frame of lap 3 when the "lap 2 complete" event fires.
    let mut sampler = AnchorSampler::new(28);
    sampler.update(1, 0.1, 3.0, 7, true);
    sampler.update(2, 0.1, 2.5, 7, true);
    sampler.update(3, 0.1, 2.1, 7, true);  // first frame of lap 3 — must be excluded

    let mut store = RegressionStore::new();
    store.ingest(&sampler, 2);  // max_lap=2 (u8)

    // Should only qualify slopes from laps 1 and 2
    let slopes = store.per_car_median_slopes(2);
    let info = slopes.get(&7).unwrap();
    assert!((info.median - (-0.5)).abs() < 0.01, "slope should be ≈0.5");
}
```

---

## 5. Rust Integration Tests (`tests/fixture.rs`)

All three tests use the single `data/test_fixture.jsonl` fixture (7000 frames, 10 laps, CarIdx 18 = player, 28 anchor buckets). The fixture is generated by `python3 scripts/synthesize_test_fixture.py` and is gitignored.

### 5.1 BATTLE_CLOSING fires at lap 3 for car 7

```rust
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
```

### 5.2 BATTLE_CLOSING (accelerating) fires at lap 4 for car 7

```rust
#[test]
fn lap4_attack_setup_car7() {
    let frames = load_fixture();
    let events = replay_frames(&frames);

    // The second BATTLE_CLOSING for car 7 should be at lap 4 with a steeper slope.
    let attack = events
        .iter()
        .filter(|e| matches!(e, RaceEvent::BattleClosing { car_idx: 7, .. }))
        .nth(1);
    assert!(attack.is_some(), "expected a second BATTLE_CLOSING event for car_idx=7 (AttackSetup phase)");

    if let Some(RaceEvent::BattleClosing { lap, slope_info, .. }) = attack {
        assert_eq!(*lap, 4, "second BATTLE_CLOSING should be at lap 4, got lap {lap}");
        assert!(
            (slope_info.median_slope - (-0.5999)).abs() < 0.01,
            "expected slope ≈ -0.600 s/lap, got {:.4}",
            slope_info.median_slope,
        );
    }
}
```

### 5.3 BATTLE_ENGAGED fires for car 7

```rust
#[test]
fn close_approach_car7() {
    let frames = load_fixture();
    let events = replay_frames(&frames);

    let ca = events.iter().find(|e| {
        matches!(e, RaceEvent::BattleEngaged { car_idx: 7, .. })
    });
    assert!(ca.is_some(), "expected a BATTLE_ENGAGED event for car_idx=7");

    if let Some(RaceEvent::BattleEngaged { lap, gap_s, .. }) = ca {
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
```

---

## 6. Running the Test Suite

```bash
# Generate the test fixture (required once)
python3 scripts/synthesize_test_fixture.py

# All tests (17 unit + 3 integration)
cargo test

# Unit tests only (fast, no fixture I/O)
cargo test --lib

# Integration tests only
cargo test --test fixture

# With output (useful when a test fails and you want the event list)
cargo test -- --nocapture
```

# Specific test
cargo test push_fires_on_sustained_closing -- --nocapture
```

The real Nürburgring fixture (`data/session.jsonl`) is 31 MB and is gitignored. In CI, download it from the designated fixture store or skip the regression test with:

```bash
cargo test --features skip_large_fixtures
```

---

## 7. Fixture Helper (`tests/fixture.rs`)

The `load_fixture()` function is defined inline at the top of `tests/fixture.rs`:

```rust
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
```

There is no `tests/common/mod.rs`. All test infrastructure lives directly in `tests/fixture.rs`.

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
