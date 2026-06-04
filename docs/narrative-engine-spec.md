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
    lap:      u8,
    bucket:   u8,
    gap_s:    f32,
    car_idx:  u8,
    is_clean: bool,  // false if YELLOW_WAVE or CAUTION was active at capture
}
```

Entries where `is_clean == false` are simply not added to `RegressionStore.data`
during `ingest()`. The store is rebuilt from scratch at each lap crossing from
`AnchorSampler.samples`, so dirty entries are filtered at rebuild time.

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

The spatial distribution from Tier 1 is surfaced in the emitted `BattleClosing` event
via the embedded `slope_info: SlopeInfo` struct. This lets the narrator say
"closing hard into Turn 3" rather than just "closing".

```rust
/// Embedded in BattleClosing events.
pub struct SlopeInfo {
    pub median_slope:         f32,    // median slope across qualifying anchors (Tier 2)
    pub anchors_qualifying:   usize,  // anchors with sufficient clean data
    pub anchors_agreeing:     usize,  // anchors where slope < 0
    pub hotspot_lap_dist_pct: f32,    // LapDistPct of steepest-closing anchor (Tier 1)
}
```

---

## 6. Yellow Flag Invalidation

Any anchor reading captured while `SessionFlags` contains `YELLOW_WAVE` (`0x100`) or `CAUTION` (`0x4000`) must be tagged `is_clean = false` and excluded from regression.

When a dirty lap results in too few clean readings to compute a reliable slope, the
engine resets `BattleState` to `Tracking` at the next lap crossing. The reset is
handled inline; there is no dedicated `ResetYellowContamination` state.

In the Nürburgring race data, `YELLOW_WAVE` flags hit the same sector (LapDistPct ~0.62) on both Lap 1 and Lap 2. Without this protection, two consecutive contaminated readings would corrupt the closing rate signal for the remainder of the race at that anchor.

---

## 7. Battle State Machine

```rust
// Defined in src/battle_state.rs
enum BattleState {
    // No active battle tracking. Initial state.
    Idle,

    // Accumulating anchor readings — insufficient clean data yet to classify.
    Tracking,

    // Sustained negative OLS slope across ≥ MIN_PUSH_READINGS (2) clean laps.
    // Condition: median_slope ≤ PUSH_SLOPE_THRESHOLD (-0.05 s/lap)
    Push,

    // Accelerating negative slope across ≥ MIN_ATTACK_READINGS (3) clean laps.
    // Condition: Push conditions met AND median_slope < prev_slope
    AttackSetup,
}
```

Opponent-identity changes (pit stop, third-car overtake) and yellow-flag
contamination are handled inline by the engine. On either condition the engine
resets the relevant `BattleState` to `Tracking` at the next lap crossing and
discards stale regression data for the previous opponent.

---

## 8. Input Contract — `TelemetryFrame`

The concrete input type passed to `NarrativeEngine::process_frame()` on each call. Fields are split into two groups based on data source availability.

```rust
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TelemetryFrame {
    // ── Player scalars ─────────────────────────────────────────────────────────
    pub lap:                  u8,    // Lap
    pub session_time:         f32,   // SessionTime
    pub lap_dist_pct:         f32,   // LapDistPct
    pub player_car_idx:       u8,    // PlayerCarIdx
    pub player_car_position:  u8,    // PlayerCarPosition (1-based; 0 = not yet classified)
    pub on_pit_road:          bool,  // OnPitRoad
    pub session_flags:        u32,   // SessionFlags (YELLOW_WAVE=0x100, CAUTION=0x4000)

    // ── Full-field arrays — live API; synthesised in JSONL fixtures ────────────
    pub car_idx_lap_dist_pct: Vec<f32>,  // CarIdxLapDistPct (< -0.5 = inactive slot)
    pub car_idx_position:     Vec<u8>,   // CarIdxPosition (0 = inactive)
    pub car_idx_on_pit_road:  Vec<bool>, // CarIdxOnPitRoad

    // ── Optional — absent from JSONL fixtures (default 0) ─────────────────────
    #[serde(default)] pub lap_last_lap_time:       f32,   // used for anchor-count bootstrap
    #[serde(default)] pub session_info_update:     u32,   // YAML roster change counter
    #[serde(default)] pub session_tick:            i64,   // dedup key for Race Control
    #[serde(default)] pub session_state:           i32,   // 4=Racing, 5=Checkered
    #[serde(default)] pub session_num:             i32,   // 0=practice,1=qual,2=race
    #[serde(default)] pub car_idx_lap_completed:   Vec<i32>, // for leaderLap context
}
```

The engine derives `car_ahead_idx` internally on each call by scanning
`car_idx_lap_dist_pct` for the car with the smallest positive delta relative to
`player_car_idx`'s position. `CarIdxF2Time` is no longer in the struct; gap is
always computed from `car_idx_lap_dist_pct × lap_time_s` which is correct for
both live and replay modes.

### 8.1 Data Source Availability

| Field group | .ibt file | Live API | JSONL fixture |
|---|---|---|---|
| Player scalars | ✅ | ✅ | ✅ (populated by generator) |
| `car_idx_*` arrays | ✗ absent | ✅ | ✅ (must be synthesised — see §9) |

---

## 9. NarrativeEngine Architecture

### 9.1 Design

The `NarrativeEngine` struct is the single entry point. It owns all state (anchor samplers, regression ring buffers, lap timer, battle state machines) and is constructed once per session with a computed `anchor_count`.

In Phase 1 all detection logic lives directly inside `NarrativeEngine`. The pluggable-detector architecture (Phase 2) is the correct pattern for adding `TireDegradationDetector`, `FuelWindowDetector`, and similar future detectors without creating a God Object.

### 9.2 Public API

```rust
pub struct NarrativeEngine { /* private */ }

impl NarrativeEngine {
    /// Construct a new engine.
    ///
    /// `anchor_count` — number of equally-spaced spatial buckets around the track.
    /// Derive from the first completed lap time: `floor(lap_time_s / 5.0).max(10)`.
    pub fn new(anchor_count: usize) -> Self;

    /// Process one telemetry frame (call at 5 Hz).
    /// Returns zero or more narrative events.
    pub fn process_frame(&mut self, frame: &TelemetryFrame) -> Vec<RaceEvent>;
}
```

For batch replay:

```rust
// replay.rs
pub fn compute_anchor_count(frames: &[TelemetryFrame]) -> usize;
pub fn replay_frames(frames: &[TelemetryFrame]) -> Vec<RaceEvent>;
```

### 9.3 Internal Components

| Component | Role |
|---|---|
| `AnchorSampler` | Records first gap reading per `(lap, bucket, car_idx)` |
| `RegressionStore` | Per-`(bucket, car_idx)` OLS series, rebuilt each lap |
| `LapTimer` | Lap-crossing detection, lap time estimation |
| `BattleState` FSM | `Idle → Tracking → Push → AttackSetup` transitions |
| `find_cars_ahead()` | Returns up to N nearest cars ahead in race order |
| `find_cars_behind()` | Returns up to N nearest cars behind in race order |
| Frame-level detectors | `BATTLE_ENGAGED/BROKEN`, `OVERTAKE`, `PIT_ENTRY/EXIT`, flag events |

### 9.4 Known Future Detector Implementations

| Detector | Domain | Primary input |
|---|---|---|
| `TireDegradationDetector` | Tyre thermal trend | `TyreTemp*`, lap delta |
| `FuelWindowDetector` | Undercut opportunity | `FuelLevel`, pit delta estimate |

---

## 10. Publisher Binary

The `napi/` crate (Node.js bridge) was removed in issue #27. The live entry point
is the pure Rust publisher binary (`src/bin/publisher.rs`). It reads iRacing
directly via the `sim_bridge` module (`src/irsdk/`) and has no Node.js dependency.

```
iRacing mmap (60 Hz)
  └─ sim_bridge::SharedMemReader::wait_for_frame()
       └─ reader::build_frame()  →  TelemetryFrame
            └─ engine.process_frame(&frame)  →  Vec<RaceEvent>
                 └─ publisher_event::build_event()  →  PublisherEvent
                      └─ transport::PublisherTransport::enqueue()
                           └─ POST /api/publisher/v2/ingest  (every 500ms)
```

### 10.1 Connection Lifecycle

| State | Condition | Engine behaviour |
|---|---|---|
| Disconnected | iRacing not running | Poll for mmap every 1 s; discard |
| Connected | Session not yet Racing | `session_state != 4`; skip regression |
| Racing | `session_state == 4` | Process every tick normally |
| Caution | `SessionFlags & (YELLOW_WAVE \| CAUTION)` | Process ticks; mark readings `is_clean = false` |
| Checkered | `session_state == 5` | Flush final events; emit `RACE_CHECKERED` |
| Reconnected | iRacing restarted | Reinitialise all engine state |

---

## 12. Event Schema

### 12.1 Example payload

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

### 12.2 Event types

| Event | Trigger | Key fields |
|---|---|---|
| `BATTLE_ENGAGED` | Gap < threshold (≥ N frames sustained), lap-1 capable | `car_idx`, `gap_s`, `car_race_position` |
| `BATTLE_BROKEN` | Gap that triggered BATTLE_ENGAGED widens past threshold | `car_idx`, `gap_s` |
| `BATTLE_CLOSING` | OLS regression confirms closing (attacker or defender) | `car_idx`, `closing_rate_sec_per_lap`, `slope_info` |
| `RACE_GREEN` | `session_state` transitions to 4 (Racing) | |
| `FLAG_YELLOW_FULL_COURSE` | Full-course caution bit set in `SessionFlags` | |
| `FLAG_YELLOW_LOCAL` | Local yellow bit set in `SessionFlags` | |
| `RACE_CHECKERED` | `session_state` transitions to 5 (Checkered) | |
| `OVERTAKE` | Position gain at start/finish crossing | `position_from`, `position_to`, `positions_gained` |
| `OVERTAKE_FOR_LEAD` | Position gain into P1 | `position_from`, `positions_gained` |
| `LAP_COMPLETED` | Start/finish crossing | `lap_time_s`, `best_lap_time_s`, `position`, `pit_frames` |
| `PIT_ENTRY` | Car enters pit road | `lap`, `position` |
| `PIT_EXIT` | Car leaves pit road | `lap`, `position` |
| `PUBLISHER_HELLO` | After successful registration | `version`, `scope` |
| `PUBLISHER_GOODBYE` | Clean shutdown | |

Liveness is message-driven in publisher v2: every authenticated ingest request
refreshes the rig check-in TTL, including HELLO, GOODBYE, and normal telemetry
batches. No periodic keepalive message is scheduled. If a rig is connected but
idle it remains silent, and check-in stays valid for up to 30 minutes since the
last received message.

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
Issue #5  — Rust ownership/lifetime strategy for engine state (completed)
Issue #6  — AnchorSampler + RegressionStore (completed)
Issue #7  — BattleState FSM (completed)
Issue #9  — JSONL replay harness (completed)
Issue #17 — Windows iRacing mmap reader (sim_bridge backed by src/irsdk/) (completed)
Issue #20 — Align event names to PublisherEventType schema (completed)
Issue #21 — PublisherEvent envelope + build_event() (completed)
Issue #22 — HTTP transport + Azure AD token (completed)
Issue #25 — Config layer (publisher.toml + env vars) (completed)
Issue #26 — Standalone publisher binary (completed)
Issue #27 — Remove napi/ and listener/ (completed)
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

The engine runs on a single driver's PC. `player_car_idx` is set to that driver's CarIdx. `NarrativeEngine` tracks only battles involving the focus car.

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

The engine still runs on a driver's PC (or an iRacing observer connection), but the engine iterates over **all active car pairs** — not just those involving the focus driver. Every `CarIdx` entry where `car_idx_position[i] > 0` is treated as a potential battle subject; for each such car, the car at `car_idx_position[i] - 1` is the potential opponent.

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
| `NarrativeEngine` scope | player only | all active cars | all active cars |
| State machine count | 1–3 at a time | up to N_cars | up to N_cars |
| `RaceEvent.car_idx` | always `player_car_idx` | any car | any car |
| Director de-duplication needed | no | no | yes |
| Single point of failure | yes | yes | no |

### 15.4 When Two Drivers Fight Each Other

If Driver A and Driver B are in the same race and both run the engine, they will each emit a `PUSH` event for the same battle from opposite perspectives:

- Engine A: `{ subject: A, object: B, closing_rate: -0.12 }` — A closing on B
- Engine B: `{ subject: B, object: A, closing_rate: +0.12 }` — B sees A behind

These are **not** the same event. Both are correct. The director should surface both: A has a `PUSH` narrative, B has a `DEFEND` narrative for the same physical battle. The `event_key` de-duplication (§15.2) only applies to events with the same `subject` and `object` — not to perspective-swapped pairs.


## 9. CarRegistry architecture

The engine keeps a fixed 64-slot `CarRegistry` keyed by `car_idx`. Each slot stores per-car identity, kinematics, anchor sampling state, and retained opponent history. Updates are O(active cars) per frame, stale-slot expiry is O(64), and memory is bounded by the fixed array plus per-car opponent vectors.

## 12. Event table additions

Added events: `HORIZON_CLOSING`, `HORIZON_CLOSING_RESOLVED`, `TIRE_DEGRADATION`, `FUEL_PROJECTION`, `FUEL_SAVING_TECHNIQUE`, `MICRO_SECTOR_GAIN`, `MICRO_SECTOR_LOSS`, `BRAKING_PROFILE`, `TRAFFIC_INTERCEPT`, `VULNERABILITY_ALERT`, `VULNERABILITY_RESOLVED`, `INCIDENT_CLUSTER`, `INCIDENT_CLUSTER_RESOLVED`, and `TRAFFIC_COMPRESSION_ZONE`.
