# Narrative Engine — Rust Data Models

**Status:** Validated against real Nürburgring telemetry  
**Companion documents:** [architecture.md](architecture.md) · [narrative-engine-spec.md](narrative-engine-spec.md) · [test-harness.md](test-harness.md)

These are the complete Rust type definitions for `director-narrative-core`. Field names map directly to iRacing SDK variable names (in CamelCase SDK → snake_case Rust). Types marked `// live API only` are not present in `.ibt` recordings and must be synthesised in JSONL fixtures.

---

## 1. Input Contract — `TelemetryFrame`

The concrete struct passed to every `StoryDetector` on each tick. Deserialised from JSON on the napi boundary and from JSONL lines in the test harness.

```rust
use serde::{Deserialize, Serialize};

/// One telemetry snapshot at 5Hz.
///
/// Fields are split into two groups based on data source availability.
/// The `car_idx_*` arrays are only available from the live iRacing
/// memory-mapped API; they must be synthesised in JSONL test fixtures.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelemetryFrame {
    // ── Player scalars ─────────────────────────────────────────────────────
    /// SessionTime — seconds since session start. Monotonic.
    pub session_time: f64,

    /// SessionFlags — bitmask. Relevant bits:
    ///   YELLOW_WAVE = 0x0100 (local yellow)
    ///   CAUTION     = 0x4000 (full-course caution)
    /// Always 0 in .ibt replay mode; must be synthesised for offline tests.
    pub session_flags: u32,

    /// PlayerCarIdx — the focus car's slot index in car_idx_* arrays.
    pub player_car_idx: usize,

    /// Lap — current lap number (1-based). 0 before first lap start.
    pub lap: u32,

    /// LapDistPct — fraction of lap distance completed (0.0–1.0).
    pub lap_dist_pct: f32,

    /// LapCurrentLapTime — elapsed time on the current lap in seconds.
    /// Unreliable before the first full lap. Use LapTimer for duration.
    pub lap_current_lap_time: f32,

    /// LapLastLapTime — last completed lap time in seconds.
    /// Always 0.0 in .ibt replay mode; derived from session_time deltas
    /// by LapTimer.
    pub lap_last_lap_time: f32,

    /// PlayerCarPosition — race position of the focus car (1-based).
    /// 0 means unknown (car not yet classified). Use as active-car guard.
    pub player_car_position: u32,

    /// OnPitRoad — whether the focus car is currently on pit road.
    pub on_pit_road: bool,

    // ── Full-field arrays — live API only ──────────────────────────────────
    // Length = session car count (up to 63). Inactive car slots carry
    // sentinel values documented per field.

    /// CarIdxLapDistPct — lap distance fraction for every car.
    ///
    /// Sentinel: 0.0 for inactive/not-in-world cars. Active car guard:
    ///   car_idx_position[i] > 0 AND car_idx_lap_dist_pct[i] > -0.5
    ///
    /// Used for `find_car_ahead_ldp()` — the primary gap source.
    /// Updated every live frame. In replay mode, this updates every frame
    /// (unlike CarIdxF2Time which is stale between lap crossings).
    pub car_idx_lap_dist_pct: Vec<f32>,

    /// CarIdxF2Time — official time gap to the car one position ahead.
    ///
    /// Sentinel: -1.0 for inactive/not-in-world cars.
    ///
    /// In live sessions, this is the most accurate gap source (iRacing's
    /// own computation, not subject to speed-conversion error).
    /// In .ibt replay mode this is STALE — only 2 unique values per lap.
    /// Prefer car_idx_lap_dist_pct × lap_time_s in offline/replay contexts.
    pub car_idx_f2_time: Vec<f32>,

    /// CarIdxPosition — race position of every car (1-based).
    /// 0 for cars not yet classified (pit lane, not on track).
    pub car_idx_position: Vec<u32>,

    /// CarIdxOnPitRoad — whether each car is currently on pit road.
    pub car_idx_on_pit_road: Vec<bool>,
}
```

---

## 2. Anchor Reading — Ring Buffer Entry

The unit of storage in the per-`(anchor_bucket, opponent_car_idx)` ring buffer.

```rust
/// One recorded gap measurement at a fixed spatial anchor.
///
/// Stored in a VecDeque<AnchorReading> per (bucket, car_idx) key.
/// The x-axis for the OLS regression is `lap` (integer lap number).
/// The y-axis is `gap_seconds` (f32 is sufficient; sub-millimetre
/// gap precision has no narrative significance).
#[derive(Debug, Clone)]
pub struct AnchorReading {
    /// Lap number when this reading was captured (1-based).
    pub lap: u32,

    /// Gap in seconds to car_ahead at this anchor position.
    /// Always positive (focus car is trailing). NaN is never stored —
    /// AnchorSampler silently drops frames where gap is NaN.
    pub gap_seconds: f32,

    /// True if SessionFlags had no YELLOW_WAVE or CAUTION bits set
    /// at capture time, AND the focus car was not on pit road.
    ///
    /// Dirty entries remain in the VecDeque for capacity accounting
    /// but are excluded from the OLS regression.
    pub is_clean: bool,
}
```

---

## 3. Battle State — FSM Enum

```rust
/// The narrative state of the relationship between the focus car and a
/// specific opponent, computed at each lap crossing.
///
/// State is stored in a HashMap<(opponent_car_idx, anchor_bucket), BattleState>
/// inside DynamicAnchorDetector. The state machine transitions are driven by
/// the two-tier regression (see architecture.md §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BattleState {
    /// No active battle tracking for this opponent pair.
    /// Initial state and the target after any battle resolution.
    Idle,

    /// Accumulating anchor readings. Insufficient clean data yet to classify
    /// a strategic intent. Emits no narrative events.
    Tracking,

    /// Sustained negative OLS slope across ≥ MIN_PUSH_READINGS clean laps.
    /// The focus driver is intentionally closing on the opponent.
    /// Condition: median_slope ≤ PUSH_SLOPE_THRESHOLD (-0.05 s/lap)
    Push,

    /// Accelerating negative slope across ≥ MIN_ATTACK_READINGS clean laps.
    /// Closing rate is increasing lap-over-lap — an overtake attempt is imminent.
    /// Condition: Push conditions met AND median_slope < prev_regression_slope
    AttackSetup,

    /// Opponent identity changed (pit stop, third-car overtake, incident).
    /// All accumulated AnchorReadings for the previous opponent are invalidated.
    /// Transitions to Tracking on the next lap crossing.
    ResetOpponentChanged { previous_opponent_car_idx: usize },

    /// One or more laps under yellow/caution exhausted the clean-reading count
    /// below MIN_PUSH_READINGS. Regression slope is unreliable.
    /// Transitions to Tracking once clean readings recover.
    ResetYellowContamination { contaminated_lap_count: u32 },
}
```

---

## 4. Narrative Events — Output Contract

All detectors produce `RaceEvent` structs. These are the values that cross the napi boundary to Node.js and are forwarded to the cloud director.

The `narrative_context` field implements the Decorator Pattern (spec §12): it extends the existing director event schema without breaking downstream consumers that do not read it.

```rust
use std::collections::HashMap;
use serde_json::Value;

/// A semantic narrative event emitted by the engine.
///
/// This struct is a superset of the director's existing RaceEvent schema.
/// The `narrative_context` field is an extension — legacy consumers that
/// do not read it continue to work without modification.
#[derive(Debug, Clone, Serialize)]
pub struct RaceEvent {
    /// Canonical event type string consumed by the director.
    /// Values: "BATTLE_OPENED", "PUSH_DETECTED", "ATTACK_SETUP",
    ///         "OVERTAKE_DETECTED", "CLOSE_APPROACH", "PIT_ENTRY",
    ///         "PIT_EXIT", "LAP_COMPLETE", "BATTLE_RESET_OPPONENT",
    ///         "BATTLE_RESET_YELLOW", "POSITION_LOST"
    pub event_type: &'static str,

    /// Focus car's slot index in CarIdx arrays.
    pub car_idx: usize,

    /// Session time in seconds at the moment of emission.
    pub session_time: f64,

    /// Lap number on which the event occurred.
    pub lap: u32,

    /// Narrative metadata. All fields are optional — their presence
    /// depends on event_type. The downstream AI narrator reads
    /// `ai_prompt_hint` directly; it does not interpret raw telemetry.
    pub narrative_context: NarrativeContext,
}

/// Narrative enrichment attached to every RaceEvent.
///
/// Fields are populated by the emitting detector. All fields are optional
/// (None serialises as null in JSON). The ai_prompt_hint is a pre-computed
/// English sentence the LLM can use directly in TTS output.
#[derive(Debug, Clone, Serialize, Default)]
pub struct NarrativeContext {
    /// Opponent car's slot index (present for battle events).
    pub opponent_car_idx: Option<usize>,

    /// Median OLS slope across qualifying anchors (seconds per lap).
    /// Negative = closing. Present for PUSH_DETECTED and ATTACK_SETUP.
    pub closing_rate_s_per_lap: Option<f32>,

    /// Rate of change of the regression slope (second derivative).
    /// Negative = accelerating close. Present for ATTACK_SETUP only.
    pub closing_rate_acceleration: Option<f32>,

    /// LapDistPct of the anchor with the steepest negative per-anchor slope.
    /// Lets the narrator say "closing hardest into Turn 3."
    pub hotspot_lap_dist_pct: Option<f32>,

    /// Number of anchors whose qualifying slope is < 0 (confidence signal).
    pub anchors_agreeing: Option<usize>,

    /// Count of clean laps included in the regression.
    pub clean_laps_in_regression: Option<u32>,

    /// Duration in seconds since BATTLE_OPENED for this opponent pair.
    pub battle_duration_seconds: Option<f64>,

    /// Gap in seconds at time of emission (for CLOSE_APPROACH events).
    pub gap_seconds: Option<f32>,

    /// Position at time of emission.
    pub race_position: Option<u32>,

    /// Net positions gained (+) or lost (-) relative to previous lap.
    pub position_delta: Option<i32>,

    /// Lap time in seconds (for LAP_COMPLETE events).
    pub lap_time_seconds: Option<f32>,

    /// Pre-computed English sentence for the AI narrator.
    /// Example: "Paul has been closing on Car 31 at 0.43 s/lap for three
    /// laps — hardest into the Karussell. An overtake attempt is likely."
    pub ai_prompt_hint: Option<String>,
}
```

### 4.1 Serialised Example Payloads

**`PUSH_DETECTED`** — emitted at lap crossing when `BattleState` first transitions to `Push`:

```json
{
  "event_type": "PUSH_DETECTED",
  "car_idx": 18,
  "session_time": 480.2,
  "lap": 3,
  "narrative_context": {
    "opponent_car_idx": 31,
    "closing_rate_s_per_lap": -0.43,
    "hotspot_lap_dist_pct": 0.29,
    "anchors_agreeing": 28,
    "clean_laps_in_regression": 2,
    "battle_duration_seconds": 187.4,
    "ai_prompt_hint": "Paul has been closing on Car 31 at 0.43 s/lap for two clean laps — hardest at 29% of the lap (approaching the Karussell). An overtake attempt is developing."
  }
}
```

**`ATTACK_SETUP`** — emitted when slope accelerates past threshold:

```json
{
  "event_type": "ATTACK_SETUP",
  "car_idx": 18,
  "session_time": 620.4,
  "lap": 4,
  "narrative_context": {
    "opponent_car_idx": 31,
    "closing_rate_s_per_lap": -0.60,
    "closing_rate_acceleration": -0.10,
    "hotspot_lap_dist_pct": 0.29,
    "anchors_agreeing": 28,
    "clean_laps_in_regression": 3,
    "battle_duration_seconds": 327.8,
    "ai_prompt_hint": "The closing rate is accelerating — Paul is now hunting Car 31 harder each lap. This looks like a deliberate setup for an overtake attempt."
  }
}
```

**`CLOSE_APPROACH`** — frame-level event (fires within a lap, not at a crossing):

```json
{
  "event_type": "CLOSE_APPROACH",
  "car_idx": 18,
  "session_time": 892.3,
  "lap": 6,
  "narrative_context": {
    "opponent_car_idx": 7,
    "gap_seconds": 0.60,
    "race_position": 11,
    "ai_prompt_hint": "Paul is within 0.6 s of Car 7 — switch to battle cam."
  }
}
```

---

## 5. Core State Structs

### 5.1 `AnchorSampler`

Records the **first** gap sample per `(lap, bucket)` — one reading per spatial anchor per lap.

```rust
/// Records the first gap crossing of each spatial anchor bucket per lap.
///
/// The anchor bucket is computed as:
///   bucket = floor(lap_dist_pct × anchor_count) % anchor_count
///
/// Only the first crossing is recorded. Subsequent crossings of the
/// same bucket within the same lap are discarded (a car can cross the
/// same LapDistPct threshold twice in extreme cases — e.g. backing up
/// on a caution lap).
pub struct AnchorSampler {
    anchor_count: usize,
    seen: HashSet<(u32, usize)>,       // (lap, bucket) already recorded
    pub samples: Vec<AnchorSample>,    // append-only history
}

/// One entry in the AnchorSampler history.
#[derive(Debug, Clone)]
pub struct AnchorSample {
    pub lap: u32,
    pub bucket: usize,
    pub gap_seconds: f32,
    pub car_ahead_idx: usize,
    pub is_clean: bool,
}
```

### 5.2 `RegressionStore`

Per-`(bucket, car_idx)` OLS regression, rebuilt from the `AnchorSampler` history at each lap crossing.

```rust
/// Identity-aware OLS regression store.
///
/// Each (bucket, car_ahead_idx) pair is an independent time series.
/// A change of opponent does not corrupt existing series; the prior
/// opponent's series remains in _data until explicitly cleared.
///
/// Rebuilt entirely from AnchorSampler.samples at each lap crossing
/// (not incrementally updated) to simplify the max_lap boundary logic.
pub struct RegressionStore {
    _data: HashMap<(usize, usize), Vec<(u32, f32)>>,  // (bucket, car_idx) → [(lap, gap_s)]
}

impl RegressionStore {
    /// Rebuild from sampler history.
    ///
    /// max_lap: exclude samples from laps > max_lap. This prevents the
    /// first frame of the new lap (already in the sampler when the
    /// lap-crossing fires) from contaminating the previous lap's regression.
    pub fn ingest(&mut self, sampler: &AnchorSampler, max_lap: Option<u32>);

    /// Returns the most-negative qualifying slope per bucket.
    /// If multiple opponents compete at the same bucket, returns the
    /// one with the steepest closing slope (most negative).
    /// Buckets with fewer than `min_readings` clean points are excluded.
    pub fn per_bucket_slopes(&self, min_readings: usize) -> HashMap<usize, f32>;

    /// OLS slope for a single (bucket, car_idx) series.
    /// Returns None if < 2 clean readings or zero variance in lap number.
    pub fn slope_for(&self, bucket: usize, car_idx: usize) -> Option<f32>;
}
```

### 5.3 `LapTimer`

Computes lap durations from `session_time` deltas. Required because `LapLastLapTime` is always 0.0 in `.ibt` replay mode.

```rust
/// Derives lap durations from the first session_time seen per lap.
///
/// Implementation: stores the first session_time for each lap number.
/// When a new lap number is observed, the duration of the previous lap
/// is recorded as (new_first_time − prev_first_time).
pub struct LapTimer {
    starts: HashMap<u32, f64>,   // lap → first session_time
    times:  HashMap<u32, f64>,   // lap → computed duration
}

impl LapTimer {
    /// Call on every frame with current (lap, session_time).
    pub fn update(&mut self, lap: u32, session_time: f64);

    /// Best estimate of the current effective lap time.
    /// Returns the most recently completed lap duration, or the
    /// NURBURGRING_LAP_EST_S fallback if no lap has completed yet.
    pub fn best_estimate(&self) -> f64;

    /// Completed duration for a specific lap number, if available.
    pub fn completed(&self, lap: u32) -> Option<f64>;
}
```

### 5.4 `DynamicAnchorDetector`

The main state machine. Implements `StoryDetector`.

```rust
/// Spatial anchor battle detector.
///
/// Implements the full two-tier pipeline from architecture.md:
///   1. find_car_ahead_ldp() — identify opponent and compute gap
///   2. AnchorSampler — record first anchor crossing per lap
///   3. RegressionStore — per-(bucket, car_idx) OLS at lap crossing
///   4. Two-tier median slope → BattleState transition
///   5. Emit RaceEvent with narrative_context
///
/// One instance tracks one focus driver (player_car_idx).
pub struct DynamicAnchorDetector {
    config: AnchorDetectorConfig,
    lap_timer: LapTimer,
    sampler: AnchorSampler,     // rebuilt from anchor_count at first lap crossing
    regression: RegressionStore,
    state: BattleState,
    prev_regression_slope: Option<f32>,
    pit_frame_count: HashMap<u32, u32>,    // lap → pit frame count
    pit_laps: HashSet<u32>,
    prev_lap: Option<u32>,
    prev_position: Option<u32>,
    lap_end_positions: HashMap<u32, u32>,  // lap → position at crossing
    anchor_count: usize,                   // recomputed from first completed lap time
}

/// Configuration constants for DynamicAnchorDetector.
#[derive(Debug, Clone)]
pub struct AnchorDetectorConfig {
    pub target_cadence_s: f64,       // default: 5.0
    pub min_push_readings: usize,    // default: 2
    pub min_attack_readings: usize,  // default: 3
    pub push_slope_threshold: f32,   // default: -0.05 s/lap
    pub attack_slope_threshold: f32, // default: -0.10 s/lap
    pub max_battle_gap_s: f32,       // default: 5.0 s
    pub pit_lap_frame_thresh: u32,   // default: 20 frames
    pub close_approach_thresh_s: f32, // default: 1.5 s
    pub close_approach_min_frames: u32, // default: 5 frames
    pub fallback_lap_time_s: f64,    // default: 540.0 (Nürburgring Combined)
    pub known_yellow_zones: Vec<(u32, f32, f32)>, // (lap, ldp_start, ldp_end)
}
```

---

## 6. Engine and Detector Trait

```rust
/// Interface implemented by every narrative detector.
///
/// Detectors are fully self-contained: each owns its own ring buffers,
/// state machines, and derivative math. The TelemetryEngine does not
/// know what any detector does internally — it only calls on_tick().
pub trait StoryDetector: Send {
    /// Process one telemetry frame. Returns zero or more narrative events.
    /// Called at 5Hz (every 200 ms) in both live and replay modes.
    fn on_tick(&mut self, frame: &TelemetryFrame) -> Vec<RaceEvent>;

    /// Unique detector name for logging and metrics.
    fn name(&self) -> &'static str;
}

/// The engine. Owns a list of detectors and fans each tick out to all of them.
/// Holds no state of its own — all domain logic lives inside detectors.
pub struct TelemetryEngine {
    detectors: Vec<Box<dyn StoryDetector>>,
}

impl TelemetryEngine {
    pub fn new() -> Self;
    pub fn register_detector(&mut self, detector: Box<dyn StoryDetector>);
    
    /// Process one frame. Returns all events from all detectors.
    pub fn process_tick(&mut self, frame: TelemetryFrame) -> Vec<RaceEvent>;
}
```

---

## 7. napi-rs Public API

The Node.js-visible entry point. JSON-in / JSON-out across the napi boundary.

```rust
#[napi]
pub struct NativeTelemetryPublisher {
    engine: TelemetryEngine,
}

#[napi]
impl NativeTelemetryPublisher {
    /// Construct a publisher with the default detector set registered.
    /// config_json: JSON-serialised AnchorDetectorConfig.
    #[napi(constructor)]
    pub fn new(config_json: String) -> napi::Result<Self> {
        let config: AnchorDetectorConfig = serde_json::from_str(&config_json)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let mut engine = TelemetryEngine::new();
        engine.register_detector(Box::new(DynamicAnchorDetector::new(config)));
        Ok(Self { engine })
    }

    /// Process one telemetry frame.
    ///
    /// frame_json: JSON-serialised TelemetryFrame.
    /// Returns: JSON array of RaceEvent (may be empty).
    ///
    /// Errors: malformed JSON is surfaced as a typed napi error — not a panic.
    /// Never call .unwrap() on input crossing this boundary.
    #[napi]
    pub fn process_tick(&mut self, frame_json: String) -> napi::Result<String> {
        let frame: TelemetryFrame = serde_json::from_str(&frame_json)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let events = self.engine.process_tick(frame);
        serde_json::to_string(&events)
            .map_err(|e| napi::Error::from_reason(e.to_string()))
    }
}
```

---

## 8. Project Structure

```
director-narrative-core/
├── Cargo.toml
├── src/
│   ├── lib.rs                  # napi-rs public API (NativeTelemetryPublisher)
│   ├── engine.rs               # TelemetryEngine + StoryDetector trait
│   ├── models.rs               # TelemetryFrame, RaceEvent, NarrativeContext
│   ├── detectors/
│   │   ├── mod.rs
│   │   ├── anchor.rs           # DynamicAnchorDetector (primary)
│   │   ├── frame_events.rs     # CLOSE_APPROACH, PIT_ENTRY/EXIT, OVERTAKE
│   │   └── future/
│   │       ├── tyre_deg.rs     # TireDegradationDetector (Phase 2)
│   │       └── fuel_window.rs  # FuelWindowDetector (Phase 2)
│   ├── math/
│   │   ├── ols.rs              # ols_slope() — linear regression
│   │   ├── sampler.rs          # AnchorSampler + AnchorSample
│   │   └── regression.rs       # RegressionStore
│   └── lap_timer.rs            # LapTimer
├── tests/
│   ├── fixtures/
│   │   ├── nurburgring_5lap.jsonl       # Real-world: 5 laps, no PUSH/ATTACK
│   │   ├── synthetic_push.jsonl         # Slow approach → PUSH at lap 3
│   │   ├── synthetic_attack.jsonl       # Accelerating close → ATTACK_SETUP lap 4
│   │   ├── synthetic_yellow.jsonl       # Yellow flag contamination scenario
│   │   └── synthetic_opponent_change.jsonl  # Opponent identity change mid-battle
│   ├── anchor_detector_test.rs
│   ├── regression_test.rs
│   └── ols_test.rs
└── scripts/
    ├── prototype_narrative.py       # Python validation prototype (reference)
    ├── synthesize_test_fixture.py   # Generates synthetic JSONL fixtures
    └── export_replay.py             # Windows: exports iRacing replay to JSONL
```
