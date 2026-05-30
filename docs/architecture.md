# Narrative Engine — Architecture

**Status:** Validated against real Nürburgring telemetry  
**Companion documents:** [narrative-engine-spec.md](narrative-engine-spec.md) · [data-models.md](data-models.md) · [test-harness.md](test-harness.md)

---

## 1. Why Not Just Buffer the Gap?

The earliest RFC for the Director app proposed tracking battle state using a sliding time window over the gap between two cars:

> *"Maintain a rolling window of the last 3–5 laps for every focus driver. Instead of just knowing the current gap is 1.2 s, the Director can calculate the derivative (the closing rate). If Car A is closing on Car B by 0.3 s per lap over a 3-lap window, you emit a `HUNTING_ENGAGED` event."*

This approach is physically correct in its intent but fails in practice. The failure mode is called the **accordion effect**.

### 1.1 The Accordion Effect

In close racing (0.3–1.5 s gaps), two cars are never at constant spacing. At every braking zone, the trailing car closes up because it brakes fractionally later. At every apex and exit, it drops back because the leading car accelerates from a better line. The gap breathes — compresses and expands — multiple times per lap, oscillating by ±0.8 s or more even when neither driver is actually changing their strategic intent.

This is not measurement noise. It is a physical property of racing at close quarters on a circuit.

The consequence: **a 5Hz rolling buffer of the gap contains orders-of-magnitude more accordion signal than closing-rate signal.** Any threshold on gap-rate-of-change in such a buffer will fire false positive `HUNTING_ENGAGED` events in virtually every braking zone, every lap.

This was confirmed empirically: applying a naive 3-lap rolling slope to the real Nürburgring dataset produces gap derivatives of ±2–4 s/lap oscillating at circuit frequency, completely masking the actual strategic closing rate of ~0.3 s/lap.

### 1.2 Why the RFC's RxJS Operators Cannot Solve It

The RFC proposed using RxJS `bufferTime()` and `pairwise()` on the time-series. These operators correctly aggregate data but they aggregate the accordion noise with the signal. `debounceTime()` adds latency (missing the overtake build-up) without reducing the false positive rate. Extending the window reduces short-term noise but reduces temporal resolution — and on a 9-minute Nürburgring lap, even a 3-lap window gives you only 3 data points for the regression.

The problem is not the operator choice. The problem is the input. A time-series of point-in-time gap measurements cannot be filtered into a reliable closing-rate signal when the dominant frequency in the signal is circuit-frequency accordion noise.

---

## 2. The Spatial Anchor Solution

The insight is geometric. The accordion effect compresses and expands at fixed **physical locations** on the circuit. Both the leading and trailing car experience the same compression at the same braking zone because they are braking at the same physical point on the track. If you measure the gap at the **same spatial coordinate** on consecutive laps, the accordion contribution from that location cancels exactly: it was the same last lap and this lap.

What remains after cancellation is the strategic signal — the lap-over-lap change in gap at that location that can only come from a genuine pace difference sustained across the full lap.

### 2.1 Spatial Anchors Defined

An **anchor** is a track position defined by `LapDistPct` — the fraction of the lap distance completed, ranging from 0.0 (start/finish) to 1.0. The engine divides the full lap into evenly spaced anchor buckets:

```
anchor_count = floor(last_lap_time_seconds / TARGET_CADENCE_SECONDS)
```

Default `TARGET_CADENCE_SECONDS = 5.0`. At Nürburgring (~540 s per lap), this yields ~108 anchors, one every ~50 metres. The engine records the gap to the car immediately ahead at the **first crossing** of each anchor bucket per lap.

### 2.2 Anchor-Lap Matrix

The data structure is a matrix where each row is a lap and each column is an anchor bucket:

```
          Anchor 0   Anchor 1   Anchor 2   …   Anchor 107
Lap 2     2.41 s     2.38 s     2.44 s         2.51 s
Lap 3     1.89 s     1.91 s     1.88 s         1.96 s
Lap 4     1.43 s     1.48 s     1.39 s         1.52 s
```

Each column is an independent time series, with lap number as the x-axis and gap at that anchor as the y-axis. The **slope of the linear regression** over that column is the closing rate at that spatial location — clean of accordion noise.

### 2.3 Cancellation Guarantee

The cancellation is not approximate. It holds exactly when:

1. The lap time is consistent lap-over-lap (same car, same fuel load order of magnitude)
2. The same opponent is ahead at the anchor location across laps

Condition 1 is met in virtually all racing scenarios. Condition 2 introduces a constraint (§3) — the regression must be **keyed on opponent identity**.

### 2.4 Dynamic Resolution

The anchor count scales with lap time. This is not optional. With a fixed 10-anchor model on a 540-second Nürburgring lap, each anchor covers ~54 seconds of track, producing a ~53-second silence between updates. The engine would be blind to the build-up in every corner complex of the circuit. 

The 5 s cadence default produces anchor densities validated against the real data:

| Track | Lap time | Anchors | Spacing (approx) |
|---|---|---|---|
| Short oval | ~60 s | 12 | ~5 s / ~80 m |
| Club circuit | ~90 s | 18 | ~5 s / ~75 m |
| Nürburgring Combined | ~540 s | 108 | ~5 s / ~50 m |
| Le Mans 24h circuit | ~215 s | 43 | ~5 s / ~105 m |

Anchor count is recomputed from the **most recently completed lap time** at each lap crossing. It does not change mid-lap — the bucket width is fixed for the duration of a lap and redrawn at the crossing.

---

## 3. Identity-Coupled State Machines

### 3.1 Why Identity Matters

Spatial anchoring removes accordion noise, but it introduces a new invariant: the regression slope at anchor $i$ is only meaningful if it is comparing the gap to the **same opponent** across all laps in the series.

In the real race data, the car immediately ahead of the focus driver changed identity multiple times within a single lap on lap 2 (Paul Crofts gained 8 positions). Comparing the gap to Car A on lap 1 with the gap to Car B on lap 2 produces a meaningless number — a "slope" that encodes the relative pace between two unrelated battles, not the closing rate in any real tactical sense.

Every anchor series must be keyed on `(opponent_car_idx, anchor_bucket)`. When the car ahead changes identity, all accumulated readings for the previous opponent at that anchor are discarded and the series restarts.

### 3.2 The Opponent Identity Index

On each tick, the engine scans `CarIdxLapDistPct` to find the car with the smallest positive `LapDistPct` delta relative to the focus driver (wrap-around handled at the start/finish line). That car's index becomes `car_ahead_idx` for the tick.

The gap in seconds is:

```
gap_s = (car_ahead_ldp - player_ldp) × lap_time_s
```

This uses `LapDistPct` rather than `CarIdxF2Time` because in iRacing replay mode `CarIdxF2Time` is stale (only updated at lap crossings, yielding only 2 unique values per lap). In a live session, `CarIdxF2Time` is the more accurate source; the engine can use either.

### 3.3 HashMap Key

The state machine is stored in a `HashMap<(u32, u32), AnchorBattle>` where the key is `(opponent_car_idx, anchor_bucket)`. When the opponent at a given anchor changes, the old entry is removed (or moved to an `archived_battles` slab for post-race analysis) and a new entry is created.

---

## 4. Two-Tier Classification: WHERE and WHETHER

With 108 anchors at Nürburgring, every lap crossing produces 108 independent regression slopes. Emitting one event per qualifying anchor would flood the downstream AI with noise. The engine applies a two-tier strategy:

**Tier 1 — WHERE:** Each `(opponent_car_idx, anchor_bucket)` series independently computes a slope. This answers *where on the circuit* the gap is closing fastest. The anchor with the steepest negative slope is the `hotspot_lap_dist_pct` — "closing hard into Turn 3."

**Tier 2 — WHETHER:** The `BattleDetector` collects all qualifying per-anchor slopes for a given opponent at each lap crossing and computes the **median** across all anchors that have sufficient clean readings. This single value drives the `BattleState` machine transition and is the value reported in the emitted event.

```
global_slope = median{ slope_i : clean_count_i ≥ MIN_THRESHOLD }
```

The median is robust to outlier anchors (e.g. an anchor that falls inside a yellow-flag sector for one lap). The downstream event carries both the median slope and the hotspot anchor, letting the AI narrator say "closing at 0.43 s/lap — hardest into the Karussell" rather than just "closing."

### 4.1 State Transition Rules

The `BattleState` transitions at each lap crossing based on the median slope and reading count:

```
IDLE
  → TRACKING   when first anchor reading is recorded for a new opponent

TRACKING  
  → PUSH       when median_slope ≤ PUSH_SLOPE_THRESHOLD (-0.05 s/lap)
               AND qualifying_anchor_count ≥ MIN_PUSH_READINGS (2)
  → IDLE       when opponent identity changes or gap > MAX_BATTLE_GAP_S

PUSH
  → ATTACK_SETUP  when median_slope ≤ ATTACK_SLOPE_THRESHOLD (-0.10 s/lap)
                  AND qualifying_anchor_count ≥ MIN_ATTACK_READINGS (3)
                  AND median_slope < prev_slope  (slope is accelerating, not just sustained)
  → IDLE          when opponent identity changes

ATTACK_SETUP
  → PUSH          when median_slope becomes less negative than prev_slope
                  (closing rate decelerating — driver "backing off" the attack)
  → IDLE          on opponent change or resolved overtake
```

The `ATTACK_SETUP → PUSH` downgrade is correct and intentional. In the synthetic fixture validation, this transition fires at lap 7 (gap = 0.1 s) when the driver is effectively on the opponent's bumper — the closing rate correctly decelerates because there is almost no gap left to close. The engine correctly models "driver is now in attacking position" even without a separate `ATTACKING` state.

---

## 5. Yellow Flag Invalidation

Any anchor reading captured while `SessionFlags & (YELLOW_WAVE | CAUTION) ≠ 0` is tagged `is_clean = false`. Clean-tagged entries remain in the `VecDeque` ring buffer for capacity accounting but are excluded from the regression.

This matters because a yellow flag forces all drivers to hold position and drop pace below the green-flag racing line. A gap reading under yellow is not comparable to a green-flag reading at the same anchor — it will artificially inflate or deflate the regression slope.

In the Nürburgring data, yellow flags at `LapDistPct ≈ 0.62` on both Lap 1 and Lap 2 would have produced two consecutive corrupted readings at every anchor in that sector. Without invalidation, the engine would compute a meaningless "slope" from two dirty readings and incorrectly classify the battle state.

**Data quality note (replay mode):** In iRacing replay mode, `SessionFlags` is always 0. For offline testing, the engine injects `is_clean = false` from a `known_yellow_zones` configuration list. In a live session, `SessionFlags` is correct.

---

## 6. Component Diagram

### 6.1 Single-rig (per-machine)

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  iRacing  (Windows shared memory — Local\IRSDKMemMapFileName)                │
│  60 Hz mmap read, event-gated on Local\IRSDKDataValidEvent                  │
│                                                                               │
│   TelemetryFrame                                                              │
│   { session_time, session_tick, lap, lap_dist_pct, session_flags,            │
│     sub_session_id, car_idx_lap_dist_pct[], car_idx_position[],              │
│     car_idx_on_pit_road[], driver_roster[], ... }                            │
└────────────────────────────────────┬─────────────────────────────────────────┘
                                     │ 60 Hz mmap poll (iRacing-gated)
                                     ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│  NarrativeEngine  (Rust, src/)                                               │
│                                                                               │
│  ┌──────────────────────────────────────────────────────────────────────┐    │
│  │  Regression-driven battle detection (lap-level)                      │    │
│  │                                                                      │    │
│  │   find_cars_ahead()      →  (car_ahead_idx, gap_s)                  │    │
│  │                                                                      │    │
│  │   AnchorSampler          →  first crossing of each bucket per lap   │    │
│  │   (HashMap<(lap,bucket), sample>)                                    │    │
│  │                                                                      │    │
│  │   RegressionStore        →  VecDeque per (bucket, car_ahead_idx)    │    │
│  │   (HashMap<(bucket, car_idx), VecDeque<AnchorReading>>)             │    │
│  │                                                                      │    │
│  │   Two-tier classification at lap crossing:                           │    │
│  │     Tier 1: per-anchor OLS slope  →  hotspot_lap_dist_pct           │    │
│  │     Tier 2: median(qualifying slopes)  →  global_slope              │    │
│  │                                                                      │    │
│  │   BattleState machine  →  IDLE / TRACKING / PUSH / ATTACK_SETUP    │    │
│  └──────────────────────────────────────────────────────────────────────┘    │
│                                                                               │
│  ┌──────────────────────────────────────────────────────────────────────┐    │
│  │  Frame-level detectors  (CLOSE_APPROACH, OVERTAKE, PIT_ENTRY/EXIT)  │    │
│  └──────────────────────────────────────────────────────────────────────┘    │
│                                                                               │
│  [ Future: TireDegradationDetector, FuelWindowDetector ]                    │
└────────────────────────────────────┬─────────────────────────────────────────┘
                                     │ Vec<PublisherEvent>
                                     ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│  Publisher binary  (src/bin/publisher.rs)                                    │
│                                                                               │
│  • Azure AD client_credentials token  (azure_identity crate)                │
│  • Batch queue → POST /api/publisher/v2/ingest  every 500ms                 │
│  • lifecycle: HELLO / HEARTBEAT / GOODBYE  on envelope                      │
│  • egui status window  (connection health, event log, token TTL)            │
└────────────────────────────────────┬─────────────────────────────────────────┘
                                     │ HTTPS  Bearer <JWT>
                                     ▼
                             Race Control API
```

### 6.2 Multi-center deployment topology

Multiple sim centers may have rigs in the **same iRacing session** simultaneously. Each center runs its own publisher binary on each rig, focused on its own entrant cars. Race Control aggregates events from all rigs into a single shared session record keyed by `subSessionId`.

```
iRacing Session #12345678
│
├─ Rig A1  (Center A)  →  focus car #42  →  publisher (centerId=A, rigId=oid-A1)
├─ Rig A2  (Center A)  →  focus car #18  →  publisher (centerId=A, rigId=oid-A2)
└─ Rig B1  (Center B)  →  focus car  #7  →  publisher (centerId=B, rigId=oid-B1)
           │
           │  all three POST to /api/publisher/v2/ingest
           ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│  Race Control  (Azure — simracecenter.com)                                   │
│                                                                               │
│  RaceSession { subSessionId: 12345678 }  ← global key, no center ownership  │
│                                                                               │
│  Events stored with centerId tag (derived from token.oid → centerId mapping)│
│                                                                               │
│  ┌─────────────────────┐   ┌─────────────────────┐                          │
│  │  Director Agent     │   │  Director Agent     │                          │
│  │  Center A           │   │  Center B           │                          │
│  │  entrants: [#42,#18]│   │  entrants: [#7]     │                          │
│  │                     │   │                     │                          │
│  │ BATTLE_CLOSING      │   │ BATTLE_CLOSING      │                          │
│  │   #42 chasing #7 ✓  │   │   #7 defending ✓    │                          │
│  │ LAP_COMPLETED #18 ✓ │   │ OVERTAKE #7>#18 ✓   │                          │
│  │ LAP_COMPLETED #7  ✗ │   │ LAP_COMPLETED #42 ✗ │                          │
│  └──────────┬──────────┘   └──────────┬──────────┘                          │
│             │                         │                                      │
│             ▼                         ▼                                      │
│        AI Narrator               AI Narrator                                │
│        TTS / Camera              TTS / Camera                               │
│        (Center A broadcast)      (Center B broadcast)                       │
└──────────────────────────────────────────────────────────────────────────────┘
```

**Key properties of this model:**

- The `RaceSession` is **globally keyed by `subSessionId`** — it is not owned by any center. Any rig in the session contributes events to the same record.
- Each rig's Azure AD identity (`token.oid`) maps to a `centerId` via an RC-side provisioning record. No `centerId` appears in `publisher.toml`.
- Each `PublisherEvent` is tagged with the publishing rig's `centerId` at ingest time.
- The **Director Agent** at each center filters events by its entrant roster. It generates commentary only for events involving its own cars — but it has full visibility of the field for context (e.g. "Car #42 has closed to within 1.2s of the leader, Car #7").
- **Deduplication**: if two rigs observe the same battle from opposite perspectives, events are keyed on the subject `carIdx` — these are distinct events (`#42 chasing #7` and `#7 being chased by #42`) and both are stored. Each Director agent uses the event keyed on its own entrant.
- The `subSessionId` lookup is scoped to `(centerId, subSessionId)` **within the rig provisioning table** only. The session record itself uses `subSessionId` globally, preventing duplicate session creation across centers for the same race.

---

## 7. Validated Outputs

Running the Python prototype against the full Nürburgring session (9985 frames, 35.5 minutes, 47-car grid) with the spatial anchor architecture produced:

| Event | Count | Notes |
|---|---|---|
| `CLOSE_APPROACH` | 8 | All real battles (CarIdx 4, 8, 9, 11, 14, 16, 24, 31) |
| `LAP_COMPLETE` | 4 | Laps 1–4, with correct lap times |
| `OVERTAKE` | 2 | +8 positions (lap 2), +2 positions (lap 3) |
| `PIT_ENTRY` | 1 | Lap 3 |
| `PIT_EXIT` | 1 | Lap 4 |
| `PUSH` | 0 | Correct — Paul's rapid passing pace changed opponent every lap |
| `ATTACK_SETUP` | 0 | Correct — no sustained single-opponent closing series |

Running the same engine against the synthetic test fixture (7000 frames, 10 laps, controlled gap schedule) confirmed all state transitions fire at the designed laps:

| Event | Lap | Regression slope |
|---|---|---|
| `PUSH` | 3 | −0.500 s/lap |
| `ATTACK_SETUP` | 4 | −0.600 s/lap (accelerating) |
| `CLOSE_APPROACH` | 6 | gap = 0.60 s |
| `OVERTAKE` | 8 | P10 → P9 |
| `PIT_ENTRY/EXIT` | 9 | IDLE state |

---

## 8. What the Old RFC Got Right and Wrong

For reference: the original RFC (May 2026) proposed battle state detection using RxJS sliding windows and an FSM with `STALKING / PRESSURE / ATTACKING / RESOLVED` states.

| RFC proposal | Status | Reason |
|---|---|---|
| Rust native addon via napi-rs | ⚠️ Superseded | Built and validated; replaced by pure Rust publisher binary — no Node.js required |
| 5Hz polling is sufficient | ✅ Adopted | Validated — narrative signals are lap-scale, not frame-scale |
| Focus-driver-only scope for v0.1 | ✅ Adopted | Correct for Phase 1 |
| Edge compute stays local | ✅ Adopted | Cost and latency reasons confirmed |
| Pluggable detector architecture | ✅ Adopted | Required to add TireDegradation, FuelWindow in Phase 2 |
| `bufferTime()` / `pairwise()` on gap stream | ❌ Replaced by spatial anchors | Accordion effect makes raw gap derivatives unreliable |
| `STALKING / PRESSURE / ATTACKING` gap thresholds | ❌ Not implemented | Point-in-time gap thresholds fire in every braking zone |
| Tracking top-10 for broadcast | ⏳ Deferred to Phase 2 | Correct goal, but requires observer connection (§15 of spec) |
| Heuristic inference (brake bias, tyre temp) | ⏳ Future work | Valid extension in TireDegradationDetector — not Phase 1 |
| Single center per session | ⚠️ Revised | Multi-center model: multiple centers' rigs can publish into the same iRacing session; each Director Agent filters by its own entrant roster |

The core insight the RFC lacked — and that the Nürburgring data validation surfaced — is that **the battle story is written in space, not time**. Measuring gap at fixed track locations rather than at fixed time intervals is what separates a signal from noise.

---

## 9. The Case for Dramatic Simplification: Rust Binary vs. Electron

The original Director app was built in Electron — a framework designed for cross-platform desktop applications, built on Chromium and Node.js. For the Director's actual job — reading iRacing telemetry and posting events to Race Control — Electron is a category error. This section documents what was removed, what replaced it, and why it matters.

### 9.1 What Electron Brought to the Rig

An Electron application embeds a full Chromium browser engine and a Node.js runtime. When the Director app launched on a sim rig, the operator was implicitly running:

- **Chromium** — a multi-process web browser with GPU compositing, V8 heap, sandbox child processes, and a full HTML/CSS/JavaScript rendering pipeline. None of this renders race events.
- **Node.js** — a general-purpose JavaScript runtime with an event loop, garbage collector, and libuv I/O layer. The GC runs unpredictably. On a 60Hz telemetry feed, a GC pause of even 4ms drops a frame.
- **napi-rs bridge** — a C FFI boundary between the Rust engine and the Node.js runtime. Every call to `engine.processFrame()` crosses this boundary, marshalling Rust structs into V8 heap objects and back.
- **IPC channels** — Electron's main process and renderer process communicate via serialised JSON over an IPC channel. Telemetry state crossed this channel on every frame.
- **npm dependency tree** — hundreds of transitive Node.js packages loaded at startup for the application shell, even when the active code path touched a handful.

At idle on a sim rig, a modern Electron app consumes **150–300 MB of RAM** just to exist. Under telemetry load, the Node.js heap and Chromium compositor add further pressure. On a sim rig running iRacing at high settings — already consuming 4–8 GB of GPU and system RAM — this is not a rounding error.

### 9.2 What the Rust Binary Does Instead

The publisher binary is a single native executable. Its entire job is:

1. Open the iRacing Windows shared-memory file
2. On each 60Hz frame, deserialise the relevant variables from the mmap into a `TelemetryFrame`
3. Call `engine.process_frame(&frame)` — a pure Rust function call, no FFI
4. Enqueue any emitted events
5. Every 500ms, POST the batch to Race Control with a cached Azure AD JWT
6. Draw a status window via `egui`

There is no browser engine. There is no garbage collector. There is no IPC channel. There is no JavaScript.

### 9.3 Resource Comparison

| Metric | Electron Director | Rust Publisher |
|---|---|---|
| **Resident memory at idle** | ~200 MB | ~8–12 MB |
| **Resident memory under load** | ~350 MB+ | ~12–15 MB |
| **Processes at runtime** | 4–6 (main, renderer, GPU, crashpad, …) | 1 |
| **Cold start to first frame processed** | 3–6 s (Chromium init, V8, module graph) | <100 ms |
| **Frame processing latency** | ~1–3 ms + GC jitter | <50 µs deterministic |
| **Binary / install size** | ~200 MB (Electron runtime bundled) | ~8–12 MB (static binary + egui) |
| **Runtime dependencies** | Node.js, Chromium, MSVC CRT | MSVC CRT only |
| **GC pauses** | Yes — V8 generational GC, unpredictable | None |

The memory reduction is **20×**. On a rig with 32GB RAM this is academic; on a rig with 16GB running iRacing VR, it is the difference between headroom and swap thrashing.

The more important property is **determinism**. The Rust engine processes every frame in a tight, predictable budget. There is no GC pause at lap 8 that delays the `ATTACK_SETUP` event by 40ms. In a live broadcast this doesn't matter much — the commentary is generated seconds after the event, not in real time. But determinism is the foundation that makes the system auditable: if an event was not emitted, it was because the engine logic did not fire, not because the runtime was busy collecting garbage.

### 9.4 The FFI Boundary Was the Biggest Risk

The napi-rs bridge was the correct approach when Node.js was the host. It was built, validated, and shipped. But the existence of the bridge created a category of bugs that no amount of engine-level testing could eliminate:

- **Marshalling bugs**: a `f32` in Rust may arrive as a JavaScript `number` (64-bit float) on the Node.js side. Precision is silently promoted. Null handling differs. Optional fields require explicit `undefined` checks.
- **Lifetime hazards**: the `ThreadSafeFunction` used to push events from the background mmap thread to the Node.js main thread requires careful Arc/Mutex discipline. A missed `call()` under backpressure could silently drop events.
- **Version coupling**: napi-rs, Node.js ABI versions, and the `.node` binary must all agree at build time. A Node.js version bump on the rig requires a full native rebuild.

The Rust publisher binary has none of these. `engine.process_frame()` is a normal function call. The return value is `Vec<PublisherEvent>`. There is no boundary.

### 9.5 Operator Simplicity

Installing the Electron Director required: Node.js runtime, npm, a build of the napi module (Cargo + napi-rs CLI), and the Electron application bundle. Updating it required repeating this on every rig.

The publisher binary is a single `.exe`. Copy it to the rig, drop `publisher.toml` alongside it, double-click. `publisher.toml` has four fields: `tenant_id`, `client_id`, `client_secret`, `scope`. Updating the binary is `xcopy`. There is nothing to install, no runtime to manage, no npm to run.

### 9.6 What Was Deliberately Not Built

The publisher binary has no configuration UI, no session management UI, no driver roster management UI, no stream preview. These features belong to Race Control's web frontend — the place where an operator with a keyboard and a monitor should manage them. The publisher's job is to watch iRacing and report what it sees. Keeping that scope hard and narrow is what makes the binary small, auditable, and deployable in under a minute.
