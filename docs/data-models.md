# Narrative Engine — Rust Data Models

**Status:** Validated against real Nürburgring telemetry  
**Companion documents:** [architecture.md](architecture.md) · [narrative-engine-spec.md](narrative-engine-spec.md) · [test-harness.md](test-harness.md)

These are the complete Rust type definitions for `director-narrative-core`. Field names map directly to iRacing SDK variable names (in CamelCase SDK → snake_case Rust). Types marked `// live API only` are not present in `.ibt` recordings and must be synthesised in JSONL fixtures.

---

## 1. Input Contract — `TelemetryFrame`

The concrete struct passed to `NarrativeEngine::process_frame()` on each call. Deserialised from JSONL lines in the test harness and from the iRacing memory-mapped API in production.

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
/// inside NarrativeEngine. The state machine transitions are driven by
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

The `RaceEvent` enum is the sole output type. Values serialise to JSON with the `event_type` discriminator injected as a top-level key (`SCREAMING_SNAKE_CASE`). All fields cross the napi boundary as camelCase.

```rust
use serde::Serialize;
use crate::battle_state::SlopeInfo;

/// All narrative events emitted by the engine.
///
/// `event_type` is injected by serde's internally-tagged format using
/// `SCREAMING_SNAKE_CASE` renaming (`Push` → `"PUSH"`, `AttackSetup` → `"ATTACK_SETUP"`).
/// `lap` and `session_time` are present in every variant.
#[derive(Debug, Serialize)]
#[serde(tag = "event_type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RaceEvent {
    // ── Lap-level (regression-driven) ─────────────────────────────────────
    Push { lap: u8, session_time: f32, car_ahead_idx: u8, slope_info: SlopeInfo },
    AttackSetup { lap: u8, session_time: f32, car_ahead_idx: u8, slope_info: SlopeInfo },
    DefendPush { lap: u8, session_time: f32, car_behind_idx: u8, slope_info: SlopeInfo },
    DefendAttack { lap: u8, session_time: f32, car_behind_idx: u8, slope_info: SlopeInfo },
    // ── Frame-level (gap threshold) ────────────────────────────────────────
    CloseApproach { lap: u8, session_time: f32, car_ahead_idx: u8, gap_s: f32, car_race_position: u8 },
    PressureBehind { lap: u8, session_time: f32, car_behind_idx: u8, gap_s: f32, car_race_position: u8 },
    // ── Position / pit ─────────────────────────────────────────────────────
    LapComplete { lap: u8, session_time: f32, lap_time_s: Option<f32>, position: u8, pit_frames: u32 },
    Overtake { lap: u8, session_time: f32, position_from: u8, position_to: u8, positions_gained: u8 },
    PositionLost { lap: u8, session_time: f32, position_from: u8, position_to: u8, positions_lost: u8 },
    PitEntry { lap: u8, session_time: f32, position: u8 },
    PitExit { lap: u8, session_time: f32, position: u8 },
}

/// Slope metadata attached to regression-driven events (Push, AttackSetup, DefendPush, DefendAttack).
#[derive(Debug, Serialize)]
pub struct SlopeInfo {
    /// Median OLS slope across all qualifying anchors (s/lap). Negative = closing.
    pub median_slope: f32,
    /// Index of the anchor bucket with the steepest negative per-anchor slope.
    pub hotspot_bucket: usize,
    /// Number of anchors whose qualifying slope passed the threshold.
    pub qualifying_anchors: usize,
}
```

### 4.1 Serialised Example Payloads

**`PUSH`** — emitted at lap crossing when `BattleState` first transitions to `Push`:

```json
{
  "event_type": "PUSH",
  "lap": 3,
  "session_time": 480.2,
  "car_ahead_idx": 7,
  "slope_info": {
    "median_slope": -0.4998,
    "hotspot_bucket": 8,
    "qualifying_anchors": 28
  }
}
```

**`ATTACK_SETUP`** — emitted when slope accelerates past threshold:

```json
{
  "event_type": "ATTACK_SETUP",
  "lap": 4,
  "session_time": 620.4,
  "car_ahead_idx": 7,
  "slope_info": {
    "median_slope": -0.5999,
    "hotspot_bucket": 8,
    "qualifying_anchors": 28
  }
}
```

**`CLOSE_APPROACH`** — frame-level event (fires within a lap, not at a crossing):

```json
{
  "event_type": "CLOSE_APPROACH",
  "lap": 6,
  "session_time": 892.3,
  "car_ahead_idx": 7,
  "gap_s": 0.60,
  "car_race_position": 11
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

### 5.4 `NarrativeEngine` internals

All state is owned by `NarrativeEngine` (see §6). The internal layout mirrors the pipeline:

```
NarrativeEngine
  ├─ anchor_sampler:   AnchorSampler       — per-lap gap readings per bucket
  ├─ regression_store: RegressionStore     — per-(bucket, car_idx) OLS ring buffers
  ├─ lap_timer:        LapTimer            — lap-crossing detection
  ├─ battle_state:     BattleState         — IDLE / TRACKING / PUSH / ATTACK_SETUP FSM
  └─ (position/pit tracking)              — frame-level event detection
```

---

## 6. Engine API

The `NarrativeEngine` struct is the single entry point. It owns all state (samplers, regression stores, lap timer) and is constructed once per session.

```rust
pub struct NarrativeEngine { /* private */ }

impl NarrativeEngine {
    /// Create a new engine.
    ///
    /// `anchor_count`: number of equally-spaced spatial buckets around the
    /// track. Use `replay::compute_anchor_count(frames)` to derive this from
    /// the first completed lap time (target cadence = 5 s/anchor).
    pub fn new(anchor_count: usize) -> Self;

    /// Process one telemetry frame. Returns zero or more narrative events.
    /// Called at 5Hz in both live and JSONL-replay modes.
    pub fn process_frame(&mut self, frame: &TelemetryFrame) -> Vec<RaceEvent>;
}
```

For batch / replay use:

```rust
// replay.rs
pub fn compute_anchor_count(frames: &[TelemetryFrame]) -> usize;
pub fn replay_frames(frames: &[TelemetryFrame]) -> Vec<RaceEvent>;
```

---

## 7. napi-rs Public API

The Node.js-visible entry point. The napi crate (`napi/`) wraps `NarrativeEngine` behind a JavaScript class. All fields cross the napi boundary as camelCase.

```typescript
// JavaScript / TypeScript API (generated by napi-rs)

interface TelemetryFrame {
  lap: number;
  sessionTime: number;
  lapDistPct: number;
  playerCarIdx: number;
  carIdxLapDistPct: number[];
  carIdxF2Time: number[];
  carIdxPosition: number[];
  carIdxOnPitRoad: boolean[];
  sessionFlags: number;
}

interface RaceEvent {
  eventType: string;       // "PUSH", "ATTACK_SETUP", "CLOSE_APPROACH", etc.
  lap: number;
  sessionTime: number;
  narrativeContext: Record<string, unknown>;  // event-specific fields, camelCase
}

class NarrativeEngine {
  constructor(anchorCount: number);
  processFrame(frame: TelemetryFrame): RaceEvent[];
}
```

Usage in Node.js:

```javascript
const { NarrativeEngine } = require('./napi/index.node');
const engine = new NarrativeEngine(28);
const events = engine.processFrame(frame);  // returns RaceEvent[]
```

---

## 8. Project Structure

```
director-narrative-core/
├── Cargo.toml                  # Workspace root: members [".", "napi"]
├── src/
│   ├── lib.rs                  # Module declarations
│   ├── engine.rs               # NarrativeEngine — process_frame() entry point
│   ├── anchor_sampler.rs       # Per-lap gap sampling at fixed track positions
│   ├── regression_store.rs     # Per-anchor OLS ring buffers
│   ├── battle_state.rs         # BattleState FSM + classify()
│   ├── gap_finder.rs           # find_cars_ahead() / find_cars_behind()
│   ├── lap_timer.rs            # Lap crossing detection
│   ├── race_event.rs           # RaceEvent enum (all narrative output types)
│   ├── replay.rs               # replay_frames() + compute_anchor_count()
│   ├── telemetry_frame.rs      # TelemetryFrame input struct
│   └── bin/replay.rs           # CLI binary
├── napi/
│   ├── Cargo.toml              # cdylib with napi4 + serde-json features
│   └── src/lib.rs              # NarrativeEngine napi class + JS type definitions
├── tests/
│   └── fixture.rs              # Integration tests: PUSH @ lap 3, ATTACK_SETUP @ lap 4, CLOSE_APPROACH @ lap 6
├── listener/
│   └── index.js                # Node.js end-to-end demo
└── data/
    ├── test_fixture.jsonl      # Synthetic fixture (gitignored, 7000 frames)
    └── session.jsonl           # Real Nürburgring session (gitignored, 31 MB)
```
