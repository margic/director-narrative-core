# Narrative Engine — Rust Data Models

**Status:** Validated against real Nürburgring telemetry  
**Companion documents:** [architecture.md](architecture.md) · [narrative-engine-spec.md](narrative-engine-spec.md) · [test-harness.md](test-harness.md)

These are the complete Rust type definitions for `director-narrative-core`. Field names map directly to iRacing SDK variable names (in CamelCase SDK → snake_case Rust). Types marked `// live API only` are not present in `.ibt` recordings and must be synthesised in JSONL fixtures.

---

## 1. Input Contract — `TelemetryFrame`

The concrete struct passed to `NarrativeEngine::process_frame()` on each call. Deserialised from JSONL lines in the test harness and from the iRacing memory-mapped API in production.

```rust
use serde::Deserialize;

/// One telemetry snapshot from the iRacing session stream.
///
/// Fields marked `#[serde(default)]` are absent from JSONL test fixtures
/// and default to 0/empty. All `car_idx_*` arrays are live-API only;
/// they must be synthesised in JSONL fixtures.
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TelemetryFrame {
    // ── Player scalars ─────────────────────────────────────────────────────
    /// Lap — current lap number (1-based). 0 before first lap start.
    pub lap: u8,

    /// SessionTime — seconds since session start. Monotonic.
    pub session_time: f32,

    /// LapDistPct — fraction of lap distance completed (0.0–1.0).
    pub lap_dist_pct: f32,

    /// PlayerCarIdx — the focus car's slot index in car_idx_* arrays.
    pub player_car_idx: u8,

    /// PlayerCarPosition — race position of the focus car (1-based).
    /// 0 means unknown (car not yet classified). Use as active-car guard.
    pub player_car_position: u8,

    /// OnPitRoad — whether the focus car is currently on pit road.
    pub on_pit_road: bool,

    /// SessionFlags — bitmask. Relevant bits:
    ///   YELLOW_WAVE = 0x0100 (local yellow)
    ///   CAUTION     = 0x4000 (full-course caution)
    /// Always 0 in .ibt replay mode; must be synthesised for offline tests.
    pub session_flags: u32,

    // ── Full-field arrays — live API only ──────────────────────────────────
    // Length = session car count (up to 63). Inactive car slots carry
    // sentinel values documented per field.

    /// CarIdxLapDistPct — lap distance fraction for every car.
    /// Sentinel: values < -0.5 are iRacing inactive-slot sentinels.
    /// Active car guard: car_idx_position[i] > 0 AND car_idx_lap_dist_pct[i] > -0.5
    pub car_idx_lap_dist_pct: Vec<f32>,

    /// CarIdxPosition — race position of every car (1-based).
    /// 0 for cars not yet classified (pit lane, not on track).
    pub car_idx_position: Vec<u8>,

    /// CarIdxOnPitRoad — whether each car is currently on pit road.
    pub car_idx_on_pit_road: Vec<bool>,

    // ── Optional / live-only scalars (default to 0 in JSONL fixtures) ─────

    /// LapLastLapTime — last completed lap time in seconds.
    /// Always 0.0 in .ibt replay mode; used for anchor-count bootstrap only.
    #[serde(default)]
    pub lap_last_lap_time: f32,

    /// SessionInfoUpdate — iRacing monotonic counter; increments when the
    /// SessionInfo YAML blob changes. Used by RosterCache::needs_update().
    #[serde(default)]
    pub session_info_update: u32,

    /// SessionTick — sim step counter (~16 ms resolution).
    /// Used as the deduplication key on Race Control per (raceSessionId, session_tick, event_type).
    #[serde(default)]
    pub session_tick: i64,

    /// SessionState — iRacing session state enum.
    /// Values: Invalid=0, GetInCar=1, Warmup=2, ParadeLaps=3, Racing=4,
    ///         Checkered=5, CoolDown=6.
    #[serde(default)]
    pub session_state: i32,

    /// SessionNum — which sub-session is active (practice=0, qual=1, race=2).
    #[serde(default)]
    pub session_num: i32,

    /// CarIdxLapCompleted — laps completed per car.
    /// Used to provide `leaderLap` in the PublisherEvent context.
    #[serde(default)]
    pub car_idx_lap_completed: Vec<i32>,
}
```

---

## 2. Anchor Reading — Sampler Entry

The unit of storage in `AnchorSampler`. Collected readings are ingested into
`RegressionStore` at each lap crossing.

```rust
/// One recorded gap measurement at a fixed spatial anchor.
///
/// Stored in AnchorSampler::samples (a plain Vec, append-only).
/// RegressionStore rebuilds its per-(bucket, car_idx) series from
/// this Vec at each lap crossing via ingest().
#[derive(Debug, Clone)]
pub struct AnchorReading {
    /// Lap number when this reading was captured (1-based).
    pub lap: u8,

    /// Spatial anchor bucket (0-based, 0..n_buckets).
    pub bucket: u8,

    /// Gap in seconds to the opponent at this anchor position.
    /// Always positive. NaN is never stored.
    pub gap_s: f32,

    /// Opponent car index.
    pub car_idx: u8,

    /// True if SessionFlags had no YELLOW_WAVE or CAUTION bits set
    /// at capture time, AND the focus car was not on pit road.
    ///
    /// Dirty entries remain in the Vec for capacity accounting
    /// but are excluded from the OLS regression.
    pub is_clean: bool,
}
```

---

## 3. Battle State — FSM Enum

```rust
/// The narrative state for one battle direction (forward or defensive).
///
/// NarrativeEngine holds two flat BattleState fields:
///   engine_state   — forward direction (player closing on car ahead)
///   defensive_state — defensive direction (car behind closing on player)
///
/// Opponent-identity changes and yellow-flag contamination are handled
/// inline by the engine; there are no dedicated reset states.
/// The state machine transitions are driven by the two-tier regression
/// (see architecture.md §4).
#[derive(Debug, Clone, PartialEq)]
pub enum BattleState {
    /// No active battle tracking. Initial state.
    Idle,

    /// Accumulating anchor readings. Insufficient clean data yet to
    /// classify a strategic intent. Emits no narrative events.
    Tracking,

    /// Sustained negative OLS slope across ≥ MIN_PUSH_READINGS clean laps.
    /// Condition: median_slope ≤ PUSH_SLOPE_THRESHOLD (-0.05 s/lap)
    ///            AND n_buckets ≥ MIN_PUSH_READINGS (2)
    Push,

    /// Accelerating negative slope across ≥ MIN_ATTACK_READINGS clean laps.
    /// Condition: Push conditions met AND median_slope < prev_slope
    ///            AND n_buckets ≥ MIN_ATTACK_READINGS (3)
    AttackSetup,
}
```

---

## 4. Narrative Events — Output Contract

The `RaceEvent` enum is the sole output type. Values serialise to JSON with the
`event_type` discriminator as a top-level key (`SCREAMING_SNAKE_CASE`).

```rust
use serde::Serialize;
use crate::battle_state::SlopeInfo;

#[derive(Debug, Serialize)]
#[serde(tag = "event_type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RaceEvent {
    // ── Battle / gap ──────────────────────────────────────────────────────
    /// Gap to a nearby car fell below the battle threshold.
    /// Fires from lap 1 — no OLS regression required.
    BattleEngaged { lap: u8, session_time: f32, car_idx: u8, gap_s: f32, car_race_position: u8 },
    /// Gap that triggered BattleEngaged has widened past the threshold.
    BattleBroken { lap: u8, session_time: f32, car_idx: u8, gap_s: f32 },
    /// OLS regression confirms a sustained closing rate (forward or defensive).
    /// car_idx is the opponent (ahead when attacker; behind when defender).
    BattleClosing { lap: u8, session_time: f32, car_idx: u8, closing_rate_sec_per_lap: f32, slope_info: SlopeInfo },
    // ── Session / flag ────────────────────────────────────────────────────
    RaceGreen { lap: u8, session_time: f32 },
    FlagYellowFullCourse { lap: u8, session_time: f32 },
    FlagYellowLocal { lap: u8, session_time: f32 },
    RaceCheckered { lap: u8, session_time: f32 },
    // ── Position ──────────────────────────────────────────────────────────
    Overtake { lap: u8, session_time: f32, position_from: u8, position_to: u8, positions_gained: u8 },
    OvertakeForLead { lap: u8, session_time: f32, position_from: u8, positions_gained: u8 },
    // ── Lap / pit ─────────────────────────────────────────────────────────
    LapCompleted { lap: u8, session_time: f32, lap_time_s: Option<f32>, best_lap_time_s: Option<f32>, position: u8, pit_frames: u32 },
    PitEntry { lap: u8, session_time: f32, position: u8 },
    PitExit { lap: u8, session_time: f32, position: u8 },
    // ── Lifecycle ─────────────────────────────────────────────────────────
    PublisherHello { lap: u8, session_time: f32, version: String, scope: String },
    PublisherGoodbye { lap: u8, session_time: f32 },
}

/// Slope metadata embedded in BattleClosing events.
#[derive(Debug, Clone, Serialize)]
pub struct SlopeInfo {
    /// Median OLS slope across all qualifying anchors (s/lap). Negative = closing.
    pub median_slope: f32,
    /// Number of anchors with sufficient clean readings that contributed.
    pub anchors_qualifying: usize,
    /// Number of those anchors where slope < 0 (confidence signal).
    pub anchors_agreeing: usize,
    /// LapDistPct of the anchor with the steepest negative slope.
    pub hotspot_lap_dist_pct: f32,
}
```

**Lifecycle liveness contract (v2):**
- A rig-scoped `PUBLISHER_HEARTBEAT` event is emitted on a wall-clock timer (`[publisher] heartbeat_interval_ms`, default 15000 ms; `0` disables) whenever iRacing is connected and a valid `subSessionId` is resolved. Its payload carries `lap`, `session_time`, `version`, and `events_enqueued_total`.
- Liveness is refreshed by every authenticated `/api/publisher/v2/ingest` request, including `PUBLISHER_HELLO`, `PUBLISHER_HEARTBEAT`, `PUBLISHER_GOODBYE`, and normal event batches.
- `PUBLISHER_HELLO` is sent when the rig starts a session.
- `PUBLISHER_GOODBYE` is sent only on clean shutdown and is the only explicit check-in clear signal.
- Heartbeats keep a quiet-but-connected rig distinguishable from a dead stream; liveness remains valid for up to 30 minutes since the last received message.

### 4.1 Serialised Example Payloads

**`BATTLE_CLOSING`** — emitted at lap crossing when OLS regression confirms closing:

```json
{
  "event_type": "BATTLE_CLOSING",
  "lap": 3,
  "session_time": 480.2,
  "car_idx": 7,
  "closing_rate_sec_per_lap": 0.4998,
  "slope_info": {
    "median_slope": -0.4998,
    "anchors_qualifying": 28,
    "anchors_agreeing": 22,
    "hotspot_lap_dist_pct": 0.29
  }
}
```

**`BATTLE_ENGAGED`** — fires as soon as gap drops below the threshold (lap 1 capable):

```json
{
  "event_type": "BATTLE_ENGAGED",
  "lap": 6,
  "session_time": 892.3,
  "car_idx": 7,
  "gap_s": 0.60,
  "car_race_position": 11
}
```

**`BATTLE_BROKEN`** — fires when gap widens past the threshold after BATTLE_ENGAGED:

```json
{
  "event_type": "BATTLE_BROKEN",
  "lap": 8,
  "session_time": 1120.0,
  "car_idx": 7,
  "gap_s": 2.1
}
```

---

## 5. Core State Structs

### 5.1 `AnchorSampler`

Records the **first** gap sample per `(lap, bucket)` — one reading per spatial anchor per lap.

```rust
/// Records the first gap crossing of each spatial anchor bucket per lap,
/// per opponent car.
///
/// bucket = (lap_dist_pct × n_buckets) as usize % n_buckets
///
/// Only the first crossing per (lap, bucket, car_idx) is recorded.
pub struct AnchorSampler {
    n_buckets: usize,
    seen: HashSet<(u8, u8, u8)>,    // (lap, bucket, car_idx)
    pub samples: Vec<AnchorReading>, // append-only; see §2
}
```

### 5.2 `RegressionStore`

Per-`(bucket, car_idx)` OLS regression, rebuilt from the `AnchorSampler` history at each lap crossing.

```rust
/// Per-(bucket, car_idx) OLS regression store.
///
/// Rebuilt entirely from AnchorSampler.samples at each lap crossing via
/// ingest(). The full-rebuild approach is correct and fast (~2 650 ops
/// per lap at Nürburgring).
pub struct RegressionStore {
    data: HashMap<(u8, u8), Vec<(u8, f32)>>,  // (bucket, car_idx) → [(lap, gap_s)]
}

impl RegressionStore {
    /// Rebuild from sampler history.
    ///
    /// max_lap: exclude samples from laps > max_lap. Prevents the first
    /// frame of the new lap from contaminating the prior lap's regression
    /// (off-by-one correctness invariant — do not remove).
    pub fn ingest(&mut self, sampler: &AnchorSampler, max_lap: u8);

    /// Most-negative slope per bucket across all cars (used for heatmap).
    pub fn per_bucket_slopes(&self, min_readings: usize) -> HashMap<u8, f32>;

    /// Per-car two-tier analysis: median of each car's per-bucket slopes.
    /// The state machine uses this to select the most-threatening car.
    pub fn per_car_median_slopes(&self, min_readings: usize) -> HashMap<u8, CarSlopeInfo>;
}

/// Per-car slope summary returned by per_car_median_slopes.
pub struct CarSlopeInfo {
    pub median:    f32,
    pub n_buckets: usize,
    /// Buckets where slope < 0 (car is closing).
    pub n_agree:   usize,
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
