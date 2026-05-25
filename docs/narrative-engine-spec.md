# Narrative Engine — Architecture Specification

**Status:** Phase 1 complete — validated against real Nürburgring telemetry  
**Implements:** Issues #2 (closed), informs #3, #5, #6, #7, #9, #10

---

## 1. Problem Statement

The goal of the narrative engine is to detect the transition from a **Strategic Holding Pattern** — a driver sitting in a draft train, saving tyres — to **Tactical Aggression** — a genuine push for an overtake. The output is a structured event stream that a downstream AI narrator can consume without interpreting raw telemetry.

The primary challenge is the **accordion effect**: in close racing (0.3–1.2s gaps), cars brake and accelerate at different points around a lap, causing the time gap to compress and expand heavily multiple times per lap. This cornering noise is orders of magnitude louder than the actual strategic closing rate. Any approach that measures point-in-time gap comparisons will generate false `ATTACKING` events in every single braking zone.

---

## 2. Core Heuristics

The following racing states are the target narrative outputs, mapped to their underlying data signatures:

| Narrative State | Data Signature |
|---|---|
| **Draft Train** | Gap is small but static — regression slope ≈ 0 across ≥ 2 clean anchor readings |
| **Push** | Gap shrinking consistently — sustained negative regression slope across ≥ 2 clean laps |
| **Attack Setup** | Accelerating negative slope — negative and steepening across ≥ 3 clean laps |
| **Overtake** | Position change confirmed at start/finish crossing |

---

## 3. Spatial Anchoring

### 3.1 Principle

Rather than measuring gap continuously over time, the engine records the gap at fixed **spatial checkpoints** (anchors) on the track, identified by `LapDistPct`. The accordion effect cancels itself out: both the current lap and the previous lap experience the same physical braking compression at the same spatial coordinate. The lap-over-lap delta at a fixed anchor is therefore a clean signal of strategic intent.

### 3.2 Dynamic Resolution Scaling

The number of anchors is **not fixed**. It is derived from the session's average lap time to maintain a target broadcast cadence. The named constant is `TARGET_CADENCE_SECONDS = 5.0`:

```
anchor_count = last_completed_lap_time_seconds / TARGET_CADENCE_SECONDS
```

The anchor count is computed from the **last completed lap time** — not `LapCurrentLapTime` (the elapsed time on the current lap), which is unreliable early in a lap. It is recalculated at each lap crossing and may update mid-session if lap times change substantially.

| Track profile | Lap time | Anchors | Update cadence |
|---|---|---|---|
| Short circuit | ~90s | ~18 | ~5.0s |
| Nürburgring Combined | ~540s | ~108 | ~5.0s |

A fixed 10-anchor model produces a ~53s silence on a 9-minute lap — too coarse to track a battle. The anchor count must be computed at session start from the first recorded lap time and may be updated if lap times change substantially.

### 3.3 Anchor Count Trade-offs

Increasing anchor count (decreasing `TARGET_CADENCE_SECONDS`) improves **spatial resolution** but does not improve **regression confidence**. The two axes are independent:

| Cadence | Nürburgring anchors | Short circuit anchors | Notes |
|---|---|---|---|
| 10s | 54 | 9 | Too coarse — 53s silence on long lap; misses corner detail |
| **5s (default)** | **108** | **18** | Validated against real Nürburgring data |
| 2.5s | 216 | 36 | Finer spatial detail; diminishing accordion cancellation benefit |
| 1s | 540 | 90 | Sub-metre buckets — position jitter within bucket reintroduces noise |

**Regression confidence is bounded by lap count, not anchor count.** Each anchor receives exactly one reading per lap regardless of how many anchors exist. At the Nürburgring (3–4 racing laps), every anchor has at most 4 readings whether there are 18 anchors or 1800.

Below ~2.5s cadence, diminishing returns on accordion cancellation set in: the bucket width becomes comparable to lap-to-lap position jitter at that spatial coordinate, and the cancellation guarantee degrades. The 5s default is the validated floor.

---

## 4. Identifier-Coupled State Machines

### 4.1 The Core Constraint

Spatial anchoring is only valid when comparing the gap to the **same opponent car** across laps. In a real race, the car immediately ahead of the focus driver changes identity continuously through overtakes, pit stops, and incidents. Comparing Lap N gap (against Car A) to Lap N−1 gap (against Car B) produces meaningless noise.

The engine must track **specific driver pairs**, not a generic "gap to whoever is ahead."

### 4.2 State Machine Keying

Each battle is identified by a `(focus_car_idx, opponent_car_idx, anchor_bucket)` triplet. State is stored in a `HashMap`. When an overtake changes the opponent identity, the prior battle's state machine is destroyed and a new one is initialised.

### 4.3 Data Source Constraint

`CarIdx*` array variables (`CarIdxF2Time`, `CarIdxLapDistPct`, `CarIdxPosition`, etc.) are **only available via the live iRacing memory-mapped API**. They are never written to `.ibt` recording files. The live engine may use `CarIdxF2Time` directly. Offline analysis, replay harnesses, and CI validation must use synthetic JSONL fixtures that simulate the live API data format.

---

## 5. Ring Buffer and Regression

### 5.1 Why N vs N−1 is Insufficient

Comparing only the most recent two laps at an anchor has three failure modes:

1. **One corrupted reading destroys the signal.** A yellow flag on Lap 2 makes the Lap 3 vs Lap 2 delta meaningless, with no mechanism to detect or discard it.
2. **Cannot distinguish sustained intent from a single lucky lap.** A single faster-than-average lap (traffic clearing, opponent mistake) produces an identical delta to a genuine three-lap push.
3. **Catastrophic on long-track events.** A 30-minute race at the Nürburgring yields at most 3–4 racing laps — N vs N−1 produces 2–3 delta values total, too few to base a narrative decision on.

### 5.2 Rolling Linear Regression Slope

The engine stores the last **N clean readings** per `(opponent_car_idx, anchor_bucket)` pair in a `VecDeque` and computes the linear regression slope across all clean entries:

$$\text{closing\_rate}_i = \text{slope of } \{(l_1, g_1),\ (l_2, g_2),\ \ldots,\ (l_k, g_k)\}$$

where $l_j$ is **lap number** (x-axis) and $g_j$ is the **gap in seconds at anchor $i$** (y-axis). The regression is computed **per anchor independently** — each anchor has its own ring buffer and its own slope. Spatial anchoring defines *where* to sample; the regression tracks *how that gap evolves over laps* at that fixed location. A sustained negative slope is far more noise-resistant than a single delta; one outlier reading is diluted by the full history.

### 5.3 Ring Buffer Entry Schema

```rust
struct AnchorReading {
    lap: u32,
    gap_seconds: f32,
    is_clean: bool,   // false if YELLOW_WAVE or CAUTION was active at capture
}
```

Entries where `is_clean == false` remain in the buffer for capacity accounting but are excluded from the regression calculation. The effective sample size for regression is the count of clean entries only.

### 5.4 Confidence Thresholds

| Track profile | Minimum clean readings for Push | Minimum for Attack Setup |
|---|---|---|
| Short circuit (~90s laps) | 3 | 4 |
| Long track (~9 min laps) | 2 | 3 |

These thresholds are configurable and should scale with expected lap count for the session.

### 5.5 Two-Tier Classification: WHERE vs WHETHER

With many anchors (e.g. 108 at Nürburgring), each anchor independently computes a slope every lap crossing. Emitting a `BATTLE_PUSH` event from every anchor that crosses the threshold would flood the downstream pipeline. The engine applies a two-tier strategy:

**Tier 1 — Per-anchor slope (WHERE):** Each `(opponent_car_idx, anchor_bucket)` ring buffer produces an individual slope. These slopes form a spatial distribution — they answer *where on track* the gap is closing.

**Tier 2 — Aggregate classification (WHETHER):** The `BattleDetector` collects all per-anchor slopes for a given opponent at each lap crossing, computes the **median slope across anchors with sufficient clean data**, and uses that single value to drive the `BattleState` transition:

$$\text{global\_slope} = \text{median}\{\text{slope}_i : \text{clean\_count}_i \geq \text{MIN\_THRESHOLD}\}$$

The median is used rather than the mean to suppress outlier anchors (e.g. a single anchor in a yellow-flag hotspot with marginal clean data).

The spatial distribution from Tier 1 is surfaced in the emitted `RaceEvent.narrative_context` as `hotspot_lap_dist_pct` — the anchor with the steepest negative slope. This lets the narrator say "closing hard into Turn 3" rather than just "closing".

```rust
// narrative_context fields populated by the two-tier strategy
hotspot_lap_dist_pct: f32,   // anchor with steepest per-lap slope (Tier 1)
closing_rate_per_lap: f32,   // median slope across qualifying anchors (Tier 2)
anchors_agreeing: usize,     // count of anchors with slope < 0 (confidence signal)
```

---

## 6. Yellow Flag Invalidation

Any anchor reading captured while `SessionFlags` contains `YELLOW_WAVE` (`0x100`) or `CAUTION` (`0x4000`) must be tagged `is_clean = false` and excluded from regression.

The state machine transitions to `ResetYellowContamination` when accumulated dirty readings leave insufficient clean history to compute a reliable slope. It re-enters `Tracking` once the next clean lap begins adding readings.

In the Nürburgring race data, `YELLOW_WAVE` flags hit the same sector (LapDistPct ~0.62) on both Lap 1 and Lap 2. Without this protection, two consecutive contaminated readings would corrupt the closing rate signal for the remainder of the race at that anchor.

---

## 7. Battle State Machine

```rust
enum BattleState {
    // No active battle tracking for this opponent
    Idle,

    // Accumulating anchor readings — insufficient clean data yet to classify
    Tracking {
        anchor_readings_count: usize,
    },

    // Sustained negative regression slope across ≥ 2 clean laps
    Push,

    // Accelerating negative slope across ≥ 3 clean laps — overtake attempt likely
    AttackSetup,

    // Car ahead changed identity (pit stop, overtake by third car, incident)
    // All accumulated anchor readings for the previous opponent are invalidated.
    // Transition back to Tracking on next lap crossing.
    ResetOpponentChanged,

    // One or more laps under yellow/caution — regression slope unreliable.
    // Wait for sufficient clean laps to re-accumulate before emitting signals.
    ResetYellowContamination,
}
```

---

## 8. Input Contract — `TelemetryFrame`

The concrete input type passed to every `StoryDetector` on each tick. Fields are split into two groups based on data source availability.

```rust
#[derive(Debug, Deserialize, Clone)]
pub struct TelemetryFrame {
    // ── Player scalars — available in .ibt AND live API ───────────────────────
    pub session_time: f64,          // SessionTime
    pub session_flags: u32,         // SessionFlags bitmask (YELLOW_WAVE=0x100, CAUTION=0x4000)
    pub player_car_idx: i32,        // PlayerCarIdx — identifies the focus car
    pub lap: u32,                   // Lap
    pub lap_dist_pct: f32,          // LapDistPct
    pub lap_current_lap_time: f32,  // LapCurrentLapTime (elapsed on current lap)
    pub lap_last_lap_time: f32,     // LapLastLapTime (used for dynamic anchor_count)
    pub speed: f32,                 // Speed (m/s)

    // ── Full-field arrays — live API only; must be present in JSONL fixtures ──
    // Length = session car count (up to 63). Inactive slots carry sentinel values.
    pub car_idx_lap_dist_pct: Vec<f32>,  // CarIdxLapDistPct  (0.0 for inactive cars)
    pub car_idx_f2_time: Vec<f32>,       // CarIdxF2Time — official time gap to car one position ahead
                                         // (-1.0 sentinel for inactive/not-in-world cars)
    pub car_idx_position: Vec<i32>,      // CarIdxPosition  (0 for inactive/unknown)
    pub car_idx_on_pit_road: Vec<bool>,  // CarIdxOnPitRoad
}
```

**Key design note:** `car_ahead_idx` and `car_dist_ahead` are no longer top-level fields. The `DynamicAnchorDetector` derives `car_ahead_idx` internally on each tick by scanning `car_idx_lap_dist_pct` for the entry with the smallest positive delta relative to the focus car's `lap_dist_pct`. `CarIdxF2Time` is used in preference to `CarDistAhead / Speed` because it is the official iRacing time-gap computation and is not subject to speed-conversion error at low speeds.

`session_flags` is required for `is_clean` tagging (§5.3). Relevant bitmasks: `YELLOW_WAVE = 0x100`, `CAUTION = 0x4000`.

### 8.1 Data Source Availability

| Field group | .ibt file | Live API | JSONL fixture |
|---|---|---|---|
| Player scalars | ✅ | ✅ | ✅ (populated by generator) |
| `car_idx_*` arrays | ✗ absent | ✅ | ✅ (must be synthesised — see §9) |

---

## 9. Pluggable StoryDetector Architecture

### 9.1 Motivation

The engine must eventually compute diverse heuristic metrics beyond spatial closing rates: tyre thermal dynamics, fuel load delta, undercut window detection. Embedding all of this logic in a single struct creates an unmaintainable God Object. Instead, the core `TelemetryEngine` is a **dumb pipeline** — it owns no domain-specific math. All narrative intelligence lives in pluggable detectors registered at startup.

### 9.2 The Trait

The trait is defined as an **extension of the director's `RaceEvent`** — detectors produce `RaceEvent` structs (the existing downstream contract) with the `narrative_context` field populated. There is no separate internal event type: the output of every detector is already in the shape the `director` cloud ingestion expects.

```rust
trait StoryDetector {
    fn on_telemetry_tick(
        &mut self,
        tick: &TelemetryTick,
    ) -> Vec<RaceEvent>;  // director's existing type, decorated with narrative_context

    fn name(&self) -> &'static str;
}
```

Each detector is fully self-contained: it owns its own `HashMap` of battle state machines, its own `VecDeque` ring buffers, and all derivative math. The engine does not know or care what any detector does internally.

### 9.3 TelemetryEngine as Dispatcher

The engine holds a list of registered detectors via a public `register_detector` method and fans each tick out to all of them. It owns **no history** — all state lives inside the detectors.

For the v0.1.0 prototype, `process_tick` returns `Vec<RaceEvent>` directly (synchronous). This is the simplest shape across the napi boundary. A `Sender<RaceEvent>` channel is the appropriate production pattern for async streaming, but adds complexity that is not needed until the engine runs in a background thread.

```rust
struct TelemetryEngine {
    detectors: Vec<Box<dyn StoryDetector>>,
}

impl TelemetryEngine {
    pub fn new() -> Self {
        Self { detectors: Vec::new() }
    }

    pub fn register_detector(&mut self, detector: Box<dyn StoryDetector>) {
        self.detectors.push(detector);
    }

    pub fn process_tick(&mut self, frame: TelemetryFrame) -> Vec<RaceEvent> {
        self.detectors
            .iter_mut()
            .flat_map(|d| d.on_telemetry_tick(&frame))
            .collect()
    }
}
```

### 9.4 Known Detector Implementations

| Detector | Domain | Primary input |
|---|---|---|
| `DynamicAnchorDetector` | Spatial closing rate | `CarIdxF2Time`, `LapDistPct` |
| `TireDegradationDetector` | Tyre thermal trend | `TyreTemp*`, lap delta |
| `FuelWindowDetector` | Undercut opportunity | `FuelLevel`, pit delta estimate |

Each detector is independently unit-testable by constructing it in isolation and feeding synthetic `TelemetryTick` sequences without running the full engine.

---

## 10. napi-rs Public API — `NativeTelemetryPublisher`

The public contract that Node.js/Electron sees. JSON-in / JSON-out across the napi boundary avoids complex napi type mappings for nested Rust structs.

```rust
#[napi]
pub struct NativeTelemetryPublisher {
    engine: TelemetryEngine,
}

#[napi]
impl NativeTelemetryPublisher {
    #[napi(constructor)]
    pub fn new() -> Self {
        let mut engine = TelemetryEngine::new();
        engine.register_detector(Box::new(DynamicAnchorDetector::new()));
        Self { engine }
    }

    #[napi]
    pub fn process_tick(&mut self, frame_json: String) -> Result<String> {
        let frame: TelemetryFrame = serde_json::from_str(&frame_json)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let events = self.engine.process_tick(frame);
        serde_json::to_string(&events)
            .map_err(|e| napi::Error::from_reason(e.to_string()))
    }
}
```

Errors from malformed JSON are surfaced as typed napi errors — not panics. The `.unwrap()` pattern must not be used on any input crossing the napi boundary.

---

## 11. Streaming Consumption

### 11.1 iRacing Data Model

The iRacing simulator writes telemetry to a shared memory-mapped file at its internal update rate (60 Hz). The SDK exposes this via a `wait_for_data(timeout)` blocking call that unblocks when a new frame is available. The engine does not poll at 60 Hz — narrative events are lap-scale signals, not frame-scale. A **5 Hz polling rate (200 ms interval)** is sufficient: it produces a sample every 200 ms, and even at maximum speed on the longest tracks, anchor crossings are separated by seconds not milliseconds.

### 11.2 Two Consumption Modes

Two modes exist depending on whether the engine is running against a live session or a replay harness (Issue #9).

**Mode 1 — Pull (v0.1.0, replay harness)**

Node.js drives the tick loop. It reads frames from a JSONL fixture file and calls `publisher.process_tick(frameJson)` synchronously. Events are returned immediately and forwarded to the `director`. This is the correct model for CI validation and development without a running sim.

```
┌────────────────────────────────────────────────────────────────┐
│ Node.js                                                         │
│                                                                 │
│  setInterval(200ms) ──► read next JSONL line                    │
│                    ──► publisher.process_tick(frameJson) ──► Rust│
│                    ◄── Vec<RaceEvent> (JSON string)      ◄── Rust│
│                    ──► emit to director                         │
└────────────────────────────────────────────────────────────────┘
```

**Mode 2 — Push (production, live iRacing session)**

Rust owns the polling loop. A background thread blocks on `ir.wait_for_data(200ms)`, builds a `TelemetryFrame`, calls `engine.process_tick`, and pushes any resulting `RaceEvent`s to the Node.js main thread via a napi `ThreadSafeFunction` callback. Node.js is purely a receiver — it never calls `process_tick` directly.

```
┌──────────────────────────────────────────────────────────────────────┐
│ Rust background thread                    │ Node.js main thread       │
│                                           │                           │
│  loop {                                   │  publisher.on_event =     │
│    ir.wait_for_data(200ms)                │    (eventJson) => {       │
│    frame = build_frame(&ir)               │      director.emit(event) │
│    events = engine.process_tick(frame)    │    }                      │
│    for e in events {                      │                           │
│      tsf.call(e.to_json())  ─────────────►│                           │
│    }                                      │                           │
│  }                                        │                           │
└──────────────────────────────────────────────────────────────────────┘
```

The `NativeTelemetryPublisher` in §10 implements Mode 1. Mode 2 requires extending it with a `start_live(callback: ThreadsafeFunction)` method and is a Phase 3+ concern.

### 11.3 Connection Lifecycle

iRacing can be launched, closed, and relaunched during an Electron session. The engine must handle all transitions:

| State | Condition | Engine behaviour |
|---|---|---|
| `Disconnected` | iRacing not running | Poll for connection every 1s; discard frames |
| `Connected` | iRacing running, not in a session | Frames arriving but `SessionState != Racing`; discard |
| `Racing` | Green flag | Process every tick normally |
| `Caution` | `SessionFlags & (YELLOW_WAVE \| CAUTION)` | Process ticks; mark anchor readings `is_clean = false` |
| `SessionEnded` | Chequered flag received | Flush final events; reset all detector state |
| `Reconnected` | iRacing restarted mid-Electron session | Reinitialise all detector state — prior lap history is invalid |

---

## 12. Event Schema — The Decorator Pattern

### 12.1 Design Rationale

The existing downstream `director` cloud ingestion already consumes `RaceEvent` structs with a defined base schema (event ID, timestamp, type, car IDs, track location). Rather than replacing this schema — which would be a breaking change to the downstream consumer — the engine **amends** it. A new `narrative_context` object is injected as an additional field. The base event routing fields remain identical, preserving full backward compatibility: a consumer that does not read `narrative_context` continues to work without modification.

This is the **Decorator Pattern** applied to an event schema: wrap the existing contract with additional capability rather than replacing it.

### 12.2 Example payload

```json
{
  "event_type": "PUSH_DETECTED",
  "car_idx": 5,
  "opponent_car_idx": 12,
  "session_time": 1482.3,
  "lap": 3,
  "anchor_bucket": 62,
  "narrative_context": {
    "battle_duration_seconds": 187,
    "closing_rate_dx_dt": -0.043,
    "clean_laps_in_regression": 3,
    "overtake_style": "sustained_pressure",
    "ai_prompt_hint": "Driver 5 has been systematically closing on car 12 for three laps. This is calculated, not opportunistic."
  }
}
```

### 12.3 Event types

| Event | Trigger | Key narrative_context fields |
|---|---|---|
| `BATTLE_OPENED` | First anchor reading for `(focus, opponent)` pair | `car_idx`, `opponent_car_idx`, `anchor_bucket` |
| `PUSH_DETECTED` | Negative regression slope, ≥ min_clean_push readings | `closing_rate_dx_dt`, `clean_laps_in_regression` |
| `ATTACK_SETUP` | Accelerating negative slope, ≥ min_clean_attack readings | `closing_rate_dx_dt`, `battle_duration_seconds` |
| `BATTLE_RESET_OPPONENT` | Car-ahead identity changed | `previous_opponent_car_idx` |
| `BATTLE_RESET_YELLOW` | Yellow contamination exhausted clean history | `contaminated_lap_count` |
| `OVERTAKE_DETECTED` | Position change confirmed at start/finish | `overtake_style`, `battle_duration_seconds` |

### 12.4 `ai_prompt_hint`

Computed entirely by the Rust engine from regression slope, battle duration, and clean-lap count. The downstream AI narrator reads this field directly — it does not interpret raw telemetry. This reduces LLM context window size and eliminates hallucination risk from telemetry misinterpretation.

---

## 13. Validation History

| Validation | Method | Outcome |
|---|---|---|
| Accordion noise cancellation | Synthetic 4-lap CSV, pandas spatial anchoring | Pass — delta signal clean at all 10 anchors |
| Dynamic resolution | Nürburgring lap time (~540s) applied to formula | 80–100 anchors required vs 10 in static model |
| Fixed-opponent assumption | 47-car Nürburgring race (Paul Crofts, Car #5) | Fail — opponent identity changes multiple times per lap |
| N vs N−1 viability | 4-lap Nürburgring race | Fail — only 3 delta values total across entire race |
| Yellow flag corruption | YELLOW_WAVE at LapDistPct ~0.62 on Laps 1 and 2 | 2 consecutive dirty readings at same anchor confirmed |
| `CarIdxF2Time` in `.ibt` | Full variable header dump of real `.ibt` file | Absent — not written to `.ibt` by iRacing |

---

## 14. Implementation Sequence

```
Issue #5  — Rust ownership/lifetime strategy for HashMap + VecDeque state
Issue #6  — VecDeque ring buffer with AnchorReading schema and regression_slope()
Issue #7  — BattleState enum with ResetOpponentChanged and ResetYellowContamination
Issue #3  — Second-derivative aggression metrics (slope-of-slope)
Issue #9  — JSONL replay harness with mandatory fixture scenarios
Issue #8  — napi-rs bindings
Issue #10 — Narrative event emission to Node.js listener
```

---

## 15. Deployment Topology

### 15.1 What the Live API Adds

The prototype (§prototype_narrative.py) proved that `CarDistAhead` alone is insufficient — the `is_clean` gap signal is corrupted by opponent identity changes every lap. The live iRacing memory API provides four array variables (one slot per car in the session, up to 63 cars) that unlock the full pipeline:

| Variable | What it provides | Why it matters |
|---|---|---|
| `CarIdxLapDistPct` | Track position of every car | Derive `car_ahead_idx` — who specifically is ahead |
| `CarIdxF2Time` | Official time gap to the car one position ahead | More precise than `CarDistAhead / Speed`; not affected by speed-conversion error |
| `CarIdxPosition` | Race position of every car | Detect position changes at any tick, not just lap crossings |
| `CarIdxOnPitRoad` | Whether each car is on pit road | Trigger `ResetOpponentChanged` when opponent pits |

None of these are available in `.ibt` recordings — they are only present in the running session's shared memory map. The engine's closing-rate signal is **only meaningful when connected to a live session**.

### 15.2 Three Deployment Modes

**Mode A — Driver-centric (Phase 1 target)**

The engine runs on a single driver's PC. `player_car_idx` is set to that driver's CarIdx. `DynamicAnchorDetector` tracks only battles involving the focus car.

```
Driver PC
  iRacing live API
    └─ Rust engine (focus = player_car_idx)
         └─ Events: (subject=player, object=near_opponent)
              └─ napi-rs → Node.js → broadcast director
```

- Event volume: low (1–3 active battles at a time)
- Coverage: the driver's own battles only
- Suitable for: single-car broadcast, driver coaching

**Mode B — Full-field broadcast (Phase 2)**

The engine still runs on a driver's PC (or an iRacing observer connection), but `DynamicAnchorDetector` iterates over **all active car pairs** — not just those involving the focus driver. Every `CarIdx` entry where `car_idx_position[i] > 0` is treated as a potential battle subject; for each such car, the car at `car_idx_position[i] - 1` is the potential opponent.

```
Driver PC (or observer connection)
  iRacing live API  →  full CarIdx arrays
    └─ Rust engine (all car pairs within BATTLE_THRESHOLD)
         └─ Events: (subject=any_car, object=any_car)
              └─ napi-rs → Node.js → broadcast director
```

- Event volume: moderate (5–15 active battles in a 30-car field)
- Coverage: the entire field
- Suitable for: broadcast sim covering the full race

**Mode C — Multi-driver federation (Phase 2+)**

Two or more drivers run Mode B engines simultaneously in the same session. Each engine independently produces full-field events. The broadcast director receives duplicate event streams and de-duplicates by a stable key:

```
event_key = hash(min(car_a_idx, car_b_idx), max(car_a_idx, car_b_idx), anchor_bucket, lap)
```

The first event to arrive for a given key is accepted; duplicates are discarded. If one driver's connection drops mid-race, the other(s) continue providing coverage without interruption.

```
Driver A PC  ─── Rust engine (full field) ─┐
Driver B PC  ─── Rust engine (full field) ─┤─► broadcast director
Driver C PC  ─── Rust engine (full field) ─┘     de-duplicates by event_key
```

### 15.3 What Changes Between Modes

| Concern | Mode A | Mode B | Mode C |
|---|---|---|---|
| `DynamicAnchorDetector` scope | player only | all active cars | all active cars |
| State machine count | 1–3 at a time | up to N_cars | up to N_cars |
| `RaceEvent.car_idx` | always `player_car_idx` | any car | any car |
| Director de-duplication needed | no | no | yes |
| Single point of failure | yes | yes | no |

### 15.4 When Two Drivers Fight Each Other

If Driver A and Driver B are in the same race and both run the engine, they will each emit a `PUSH_DETECTED` event for the same battle from opposite perspectives:

- Engine A: `{ subject: A, object: B, closing_rate: -0.12 }` — A closing on B
- Engine B: `{ subject: B, object: A, closing_rate: +0.12 }` — B sees A behind

These are **not** the same event. Both are correct. The director should surface both: A has a `PUSH` narrative, B has a `DEFEND` narrative for the same physical battle. The `event_key` de-duplication (§15.2) only applies to events with the same `subject` and `object` — not to perspective-swapped pairs.
