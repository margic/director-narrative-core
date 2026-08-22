# Narrative Core Race Events Emission Report

## Scope and Method

This report is code-traced against the current implementation in `src/` and documents:

1. Which `RaceEvent` variants are actually emitted.
2. Which module emits each event.
3. Exact trigger conditions as implemented.
4. Example serialized `RaceEvent` payloads for each emitted event.

Primary sources:

- `src/engine.rs`
- `src/basic_incident.rs`
- `src/lift_coast.rs`
- `src/braking_profile.rs`
- `src/micro_sector.rs`
- `src/tire_degradation.rs`
- `src/fuel_projection.rs`
- `src/horizon.rs`
- `src/traffic_intercept.rs`
- `src/incident_cluster.rs`
- `src/compression_zone.rs`
- `src/vulnerability.rs`
- `src/lifecycle.rs`
- `src/bin/publisher.rs`
- `src/race_event.rs`

## Emission Pipeline Trace

### 1) Publisher lifecycle and iRacing connectivity

Emitted in `src/bin/publisher.rs` and `src/lifecycle.rs`:

- `IRACING_CONNECTED`
- `IRACING_DISCONNECTED`
- `PUBLISHER_HELLO`
- `PUBLISHER_GOODBYE`

Behavior notes:

- `IRACING_CONNECTED` is emitted when session metadata is available and `sub_session_id > 0`, guarded by `emit_iracing_connected` (once per attach/re-attach cycle).
- `IRACING_DISCONNECTED` is emitted when `reader.is_connected()` flips false and is flushed immediately.
- `PUBLISHER_HELLO` is emitted from `LifecyclePublisher::on_activate` when lifecycle is fresh and `sub_session_id > 0`.
- `PUBLISHER_GOODBYE` is emitted at shutdown via `LifecyclePublisher::on_deactivate` and enqueued if a `last_frame` exists.

### 2) Per-frame detector pass (every frame)

In `NarrativeEngine::process_frame` (`src/engine.rs`), called every decoded frame:

- Car registry update
- `BasicIncidentDetector::update` -> `INCIDENT_ALERT`
- `LiftCoastDetector::update` -> `FUEL_SAVING_TECHNIQUE`
- `BrakingProfileDetector::update` -> `BRAKING_PROFILE` (configuration-gated; dormant with current defaults, see note below)
- Session state/flag transitions:
  - `RACE_GREEN`
  - `RACE_CHECKERED`
  - `DRIVER_ENTERED_CAR`
  - `DRIVER_EXITED_CAR`
  - `FLAG_YELLOW_FULL_COURSE`
  - `FLAG_YELLOW_LOCAL`
- Pit road transition events:
  - `PIT_ENTRY`
  - `PIT_EXIT`
- Near-gap engagement tracking (ahead and behind):
  - `BATTLE_ENGAGED`
  - `BATTLE_BROKEN`

### 3) Lap-boundary pass (when `lap != prev_lap`)

Still in `src/engine.rs`, on lap crossing:

- `LAP_COMPLETED`
- Position delta/overtake events:
  - `OVERTAKE`
  - `OVERTAKE_FOR_LEAD`
- Regression/classification-driven pressure events:
  - `BATTLE_CLOSING`
- Micro-sector analysis:
  - `MICRO_SECTOR_GAIN`
  - `MICRO_SECTOR_LOSS`
- Tire model:
  - `TIRE_DEGRADATION` (suppressed if telemetry invalid)
- Fuel model:
  - `FUEL_PROJECTION` (suppressed if telemetry invalid)
- Horizon model:
  - `HORIZON_CLOSING`
  - `HORIZON_CLOSING_RESOLVED`
- Traffic model:
  - `TRAFFIC_INTERCEPT`
- Incident clustering:
  - `INCIDENT_CLUSTER`
  - `INCIDENT_CLUSTER_RESOLVED`
- Multiclass compression:
  - `TRAFFIC_COMPRESSION_ZONE`
- Composite vulnerability:
  - `VULNERABILITY_ALERT`
  - `VULNERABILITY_RESOLVED`

## Event Catalog (Emitted)

The examples below show serialized `RaceEvent` shape (`event_type` tag + payload fields), not the outer `PublisherEvent` envelope.

### `BATTLE_ENGAGED`

Trigger:

- Nearest car ahead or behind has `gap < 1.5s` for at least 5 consecutive frames.
- At least 30s since previous close engagement in that direction.
- Car is different from currently tracked car for that direction.

Example:

```json
{
  "event_type": "BATTLE_ENGAGED",
  "lap": 12,
  "session_time": 742.3,
  "player_car_idx": 7,
  "opponent_car_idx": 6,
  "gap_s": 0.84,
  "car_race_position": 4,
  "prior_skirmishes": 2,
  "prior_attack_time_s": 18.5,
  "engagement_started_at_session_time_s": 742.3
}
```

### `BATTLE_BROKEN`

Trigger:

- A tracked engagement target changes/disappears in either ahead or behind tracking path.

Example:

```json
{
  "event_type": "BATTLE_BROKEN",
  "lap": 12,
  "session_time": 768.1,
  "player_car_idx": 7,
  "opponent_car_idx": 6,
  "final_gap_sec": 2.1,
  "car_race_position": 4,
  "engagement_started_at_session_time_s": 742.3
}
```

### `BATTLE_CLOSING`

Trigger:

- On lap crossing, regression classification state changes to `Push` or `AttackSetup` for forward or defensive regression streams.

Example:

```json
{
  "event_type": "BATTLE_CLOSING",
  "lap": 12,
  "session_time": 780.0,
  "player_car_idx": 7,
  "opponent_car_idx": 6,
  "car_race_position": 4,
  "closing_rate_sec_per_lap": 0.31,
  "slope_info": {
    "median_slope": -0.31,
    "anchors_qualifying": 4,
    "anchors_agreeing": 3,
    "hotspot_lap_dist_pct": 0.62
  },
  "prior_skirmishes": 2,
  "prior_attack_time_s": 20.1
}
```

### `HORIZON_CLOSING`

Trigger (`src/horizon.rs`):

- Pair of active cars where attacker is behind defender in race position by at most 3 places.
- Gap in seconds <= 60.
- Attacker slope median `< -0.05`.
- Current gap > 5s.
- Pair was not already active.

Example:

```json
{
  "event_type": "HORIZON_CLOSING",
  "lap": 13,
  "session_time": 840.0,
  "attacker_car_idx": 9,
  "defender_car_idx": 7,
  "attacker_position": 6,
  "defender_position": 5,
  "current_gap_s": 12.4,
  "closing_rate_sec_per_lap": 0.28,
  "estimated_laps_to_contact": 45
}
```

### `HORIZON_CLOSING_RESOLVED`

Trigger:

- Previously active horizon pair now has slope `> -0.02`.

Example:

```json
{
  "event_type": "HORIZON_CLOSING_RESOLVED",
  "lap": 15,
  "session_time": 960.0,
  "attacker_car_idx": 9,
  "defender_car_idx": 7
}
```

### `RACE_GREEN`

Trigger:

- `session_state` transitions into `4` from a different previous state.

The very first frame after connecting has no previous state to transition from,
so a session already running green produces a *snapshot*, not a flag. That case
is marked `synthetic: true` / `origin: "CONNECT_SNAPSHOT"`; a real transition is
`synthetic: false` / `origin: "SESSION_STATE_TRANSITION"`. A consumer modelling
session lifecycle must ignore synthetic ones (this is why a Practice session
could show four `RACE_GREEN` events, one per rig at connect).

Example:

```json
{
  "event_type": "RACE_GREEN",
  "lap": 1,
  "session_time": 0.0,
  "synthetic": false,
  "origin": "SESSION_STATE_TRANSITION"
}
```

### `RACE_CHECKERED`

Trigger (whichever arrives first, once per session):

- `session_state` transitions into `5` (Checkered) from a different previous state, or
- `session_flags` carries the checkered bit (`0x0001`), or
- `session_state` goes straight from `4` (Racing) to `6` (CoolDown), which is what
  short practice/qualifying sessions do.

Example:

```json
{
  "event_type": "RACE_CHECKERED",
  "lap": 35,
  "session_time": 3600.0,
  "synthetic": false,
  "origin": "SESSION_STATE_TRANSITION"
}
```

Carries the same `synthetic`/`origin` pair as `RACE_GREEN`: connecting to a
session that is already past the flag reports `origin: "CONNECT_SNAPSHOT"`.

### `FLAG_YELLOW_FULL_COURSE`

Trigger:

- `session_flags` gains `CAUTION` bit (`0x4000`) against previous frame.

Example:

```json
{
  "event_type": "FLAG_YELLOW_FULL_COURSE",
  "lap": 18,
  "session_time": 1200.2
}
```

### `FLAG_YELLOW_LOCAL`

Trigger:

- `session_flags` gains `YELLOW_WAVE` bit (`0x0100`) while full-course caution did not just trigger.
- Scope inferred from incident-cluster proximity to player lap distance.

Example:

```json
{
  "event_type": "FLAG_YELLOW_LOCAL",
  "lap": 18,
  "session_time": 1200.2,
  "trigger_car_idx": 23,
  "lap_dist_pct": 0.64,
  "sector": null,
  "scope": "Nearby",
  "linked_incident_id": 12
}
```

### `IRACING_CONNECTED`

Trigger (`src/bin/publisher.rs`):

- Lifecycle is fresh and `sub_session_id > 0`; emitted once per attach/re-attach cycle.

Example:

```json
{
  "event_type": "IRACING_CONNECTED",
  "lap": 0,
  "session_time": 100.57
}
```

### `IRACING_DISCONNECTED`

Trigger:

- Publisher detects `!reader.is_connected()` while previously connected.

Example:

```json
{
  "event_type": "IRACING_DISCONNECTED",
  "lap": 1,
  "session_time": 210.95
}
```

### `DRIVER_ENTERED_CAR`

Trigger:

- `in_car := session_state != 0` transitions from false to true.

Example:

```json
{
  "event_type": "DRIVER_ENTERED_CAR",
  "lap": 0,
  "session_time": 100.57,
  "player_car_idx": 0
}
```

### `DRIVER_EXITED_CAR`

Trigger:

- `in_car := session_state != 0` transitions from true to false.

Example:

```json
{
  "event_type": "DRIVER_EXITED_CAR",
  "lap": 0,
  "session_time": 220.1,
  "player_car_idx": 0
}
```

### `OVERTAKE`

Trigger:

- On lap crossing, previous lap-end position to current lap-end position improves (`delta > 0`).
- Completed lap is not marked a pit lap.
- New position is not P1.

Example:

```json
{
  "event_type": "OVERTAKE",
  "lap": 14,
  "session_time": 900.0,
  "car_idx": 7,
  "overtaken_car_idx": 6,
  "position_from": 5,
  "position_to": 4,
  "positions_gained": 1
}
```

### `OVERTAKE_FOR_LEAD`

Trigger:

- Same overtake conditions as above, but resulting `position_to == 1`.

Example:

```json
{
  "event_type": "OVERTAKE_FOR_LEAD",
  "lap": 20,
  "session_time": 1300.0,
  "car_idx": 7,
  "overtaken_car_idx": 3,
  "position_from": 2,
  "positions_gained": 1
}
```

### `LAP_COMPLETED`

Trigger:

- On any lap increment (`lap != prev_lap`) once player has valid race position (`pos > 0`) and `lap >= 1` processing path.

Example:

```json
{
  "event_type": "LAP_COMPLETED",
  "lap": 14,
  "session_time": 900.0,
  "player_car_idx": 7,
  "lap_time_s": 89.45,
  "best_lap_time_s": 88.91,
  "position": 4,
  "pit_frames": 0
}
```

### `PIT_ENTRY`

Trigger:

- `on_pit_road` transitions false -> true.

Example:

```json
{
  "event_type": "PIT_ENTRY",
  "lap": 21,
  "session_time": 1410.4,
  "player_car_idx": 7,
  "position": 5
}
```

### `PIT_EXIT`

Trigger:

- `on_pit_road` transitions true -> false.

Pit transitions are detected before the unclassified-car guard, so a car sitting
in its stall with `position == 0` (or on lap 0) still publishes its
`PIT_ENTRY`/`PIT_EXIT` pair. `position` is published as reported, `0` included.

Example:

```json
{
  "event_type": "PIT_EXIT",
  "lap": 21,
  "session_time": 1455.7,
  "player_car_idx": 7,
  "position": 9
}
```

### `TIRE_DEGRADATION`

Trigger (`src/tire_degradation.rs`, called at lap crossing):

- Not a pit lap (pit lap resets state and emits nothing).
- At least 3 lap EMA points available.
- Engine additionally suppresses event if `has_valid_data()` is false (all tire channels effectively unavailable).

Example:

```json
{
  "event_type": "TIRE_DEGRADATION",
  "lap": 16,
  "session_time": 1020.0,
  "lf_temp_c": 93.2,
  "rf_temp_c": 95.1,
  "lr_temp_c": 90.4,
  "rr_temp_c": 91.0,
  "lf_slope_c_per_min": 1.7,
  "rf_slope_c_per_min": 2.1,
  "lr_slope_c_per_min": 1.1,
  "rr_slope_c_per_min": 1.3,
  "hottest_corner": "RF"
}
```

### `FUEL_PROJECTION`

Trigger (`src/fuel_projection.rs`, called at lap crossing):

- Not a pit lap.
- At least one clean fuel delta observed (`clean_deltas` non-empty).
- Engine additionally suppresses event if `has_valid_data()` is false (all deltas zero / no meaningful consumption).

Example:

```json
{
  "event_type": "FUEL_PROJECTION",
  "lap": 16,
  "session_time": 1020.0,
  "fuel_remaining_l": 42.8,
  "fuel_per_lap_l": 2.45,
  "laps_remaining": 17.47,
  "is_provisional": false
}
```

### `FUEL_SAVING_TECHNIQUE`

Trigger (`src/lift_coast.rs`):

- Coasting starts when throttle < 0.05, brake < 0.05, speed > 25 m/s.
- Event emits when coast ends by brake > 0.05 or throttle > 0.2.
- Coast duration must be >= 1.0s.

Example:

```json
{
  "event_type": "FUEL_SAVING_TECHNIQUE",
  "lap": 17,
  "session_time": 1092.3,
  "coast_duration_s": 1.4,
  "coast_start_lap_dist_pct": 0.38,
  "coast_start_speed_mps": 61.7
}
```

### `INCIDENT_ALERT`

Trigger (`src/basic_incident.rs`):

- Per-car edge-triggered incident condition when not in pit transition and either:
  - track surface changed, or
  - severe speed drop (`drop >= 8.0 m/s` and speed ratio <= 0.80).
- Additional player-only alert when `player_incident_count` increases and no player alert already emitted that frame, and the car is in the world, off pit road, and was moving (>= 5 m/s).
- `severity` is a raw magnitude whose units depend on `reason`; `severity_normalized` is always 0.0-1.0. See `docs/publisher-event-contract.md`.

Example:

```json
{
  "event_type": "INCIDENT_ALERT",
  "lap": 22,
  "session_time": 1472.8,
  "car_idx": 7,
  "driver_incident_count": 6,
  "previous_track_surface": 3,
  "current_track_surface": 1,
  "previous_speed_mps": 71.2,
  "current_speed_mps": 49.7,
  "speed_drop_mps": 21.5,
  "severity": 0.30,
  "severity_normalized": 0.30,
  "incident_count_delta": null,
  "reason": "surface_change_and_speed_drop"
}
```

### `MICRO_SECTOR_GAIN`

Trigger (`src/micro_sector.rs`, on lap end):

- Clean lap only.
- At least one prior clean lap exists.
- Emits for contiguous improved anchor-segment streaks where current segment time is faster than best historical segment.

Example:

```json
{
  "event_type": "MICRO_SECTOR_GAIN",
  "lap": 23,
  "session_time": 1540.0,
  "bucket_from": 12,
  "bucket_to": 15,
  "lap_dist_pct_from": 0.60,
  "lap_dist_pct_to": 0.80,
  "cumulative_delta_s": 0.18,
  "technique_hint": "carried more speed"
}
```

### `MICRO_SECTOR_LOSS`

Trigger (`src/micro_sector.rs`, on lap end):

- Clean lap only.
- At least one prior clean lap exists.
- Emits for contiguous degraded anchor-segment streaks where current segment time is slower than best historical segment.
- Very small jitter is suppressed by per-bucket and per-streak thresholds (`MIN_BUCKET_DELTA_S`, `MIN_STREAK_DELTA_S`).

Example:

```json
{
  "event_type": "MICRO_SECTOR_LOSS",
  "lap": 23,
  "session_time": 1540.0,
  "bucket_from": 8,
  "bucket_to": 10,
  "lap_dist_pct_from": 0.40,
  "lap_dist_pct_to": 0.55,
  "cumulative_delta_s": 0.11
}
```

### `BRAKING_PROFILE` (configuration-gated)

Trigger (`src/braking_profile.rs`):

- Requires anchor bucket to be in `heavy_braking_anchors`.
- Starts a brake window when heavy bucket and `brake > 0` and not already emitted for that bucket/lap.
- Window closes when bucket changes and brake no longer active, then emits profile.

Current runtime default in `NarrativeEngine` uses `BrakingProfileDetector::new()` with no heavy anchors configured, so this event is dormant unless anchors are configured.

Example:

```json
{
  "event_type": "BRAKING_PROFILE",
  "lap": 23,
  "session_time": 1542.5,
  "anchor_bucket": 6,
  "brake_point_pct": 0.305,
  "brake_release_pct": 0.344,
  "peak_brake_pct": 0.86,
  "braking_energy": 24.9,
  "entry_speed_mps": 72.1,
  "min_speed_mps": 41.3,
  "throttle_release_pct": 0.298
}
```

### `TRAFFIC_INTERCEPT`

Trigger (`src/traffic_intercept.rs`):

- For each leader/traffic pair:
  - cross-class OR same-class with leader at least 1 lap ahead.
  - relative speed positive (leader faster).
  - predicted intercept time < 30s.
  - de-duplicated by intercept bucket proximity (<2 buckets to previous for same pair is suppressed).

Example:

```json
{
  "event_type": "TRAFFIC_INTERCEPT",
  "lap": 24,
  "session_time": 1600.0,
  "leader_car_idx": 7,
  "traffic_car_idx": 41,
  "cross_class": true,
  "distance_m": 420.0,
  "relative_speed_mps": 11.8,
  "time_to_intercept_s": 18.6,
  "intercept_bucket": 9,
  "intercept_lap_dist_pct": 0.46,
  "predicted_intercept_session_time": 1618.6
}
```

### `VULNERABILITY_ALERT`

Trigger (`src/vulnerability.rs`):

- Composite vulnerability score crosses configured critical threshold (`>= 0.60`) while alert not active and not on pit road.

Example:

```json
{
  "event_type": "VULNERABILITY_ALERT",
  "lap": 24,
  "session_time": 1600.0,
  "vulnerability": 0.72,
  "defender_idx": 7,
  "attacker_idx": 6,
  "tire_contribution": 0.17,
  "closing_contribution": 0.29,
  "proximity_contribution": 0.19,
  "fuel_contribution": 0.07
}
```

### `VULNERABILITY_RESOLVED`

Trigger:

- If on pit road while alert active, resolves immediately.
- Otherwise resolves when alert active and vulnerability drops below `0.75 * threshold`.

Example:

```json
{
  "event_type": "VULNERABILITY_RESOLVED",
  "lap": 25,
  "session_time": 1660.0,
  "defender_idx": 7,
  "attacker_idx": 6
}
```

### `INCIDENT_CLUSTER`

Trigger (`src/incident_cluster.rs`):

- In a bucket, at least 3 cars are slowed relative to class/bucket baseline (`speed < baseline * 0.7`) and not in full-course caution.
- Bucket not already active.

Example:

```json
{
  "event_type": "INCIDENT_CLUSTER",
  "lap": 25,
  "session_time": 1675.0,
  "bucket": 18,
  "lap_dist_pct_from": 0.90,
  "lap_dist_pct_to": 0.95,
  "car_idxs": [12, 18, 23],
  "severity": 3.0,
  "primary_car_idx": 12,
  "incident_type": "Incident"
}
```

### `INCIDENT_CLUSTER_RESOLVED`

Trigger:

- For a currently evaluated bucket with fewer than 3 slowed cars where that bucket was previously active.

Example:

```json
{
  "event_type": "INCIDENT_CLUSTER_RESOLVED",
  "lap": 25,
  "session_time": 1682.0,
  "bucket": 18
}
```

### `TRAFFIC_COMPRESSION_ZONE`

Trigger (`src/compression_zone.rs`):

- Multiclass session (`class_count > 1`).
- Attacker/defender are same class, adjacent in position (difference exactly 1), attacker behind defender.
- At least 3 traffic cars (other classes) have predicted intercept in < 30s to either attacker or defender.

Example:

```json
{
  "event_type": "TRAFFIC_COMPRESSION_ZONE",
  "lap": 26,
  "session_time": 1740.0,
  "battle_attacker_idx": 7,
  "battle_defender_idx": 6,
  "window_start_pct": 0.41,
  "window_end_pct": 0.43,
  "traffic_car_idxs": [41, 42, 43],
  "compression_score": 3,
  "first_intercept_car_idx": 41,
  "first_intercept_time_s": 18.4,
  "first_intercept_bucket": 11
}
```

### `PUBLISHER_HELLO`

Trigger (`src/lifecycle.rs` + `src/bin/publisher.rs`):

- Lifecycle is fresh and session metadata resolution allows publishing (`sub_session_id > 0`).

Example:

```json
{
  "event_type": "PUBLISHER_HELLO",
  "lap": 0,
  "session_time": 100.57,
  "version": "0.1.1",
  "scope": "driver"
}
```

### `PUBLISHER_GOODBYE`

Trigger:

- Publisher shutdown path (`on_deactivate`), enqueued when `last_frame` is available.

Example:

```json
{
  "event_type": "PUBLISHER_GOODBYE",
  "lap": 26,
  "session_time": 1799.8
}
```

### `DRIVER_MATERIAL`

Trigger:

- Wall-clock cadence, every `publisher.driver_material_interval_ms` (default
  25 000 ms; `0` disables, env override `PUBLISHER_DRIVER_MATERIAL_INTERVAL_MS`).
  The timer restarts on a sub-session change, and the event is suppressed while
  the roster has not yet resolved the rig driver's name.
- Suppressed while the car is on pit road (including sitting in the box) or not
  in the world (`CarIdxTrackSurface < 0`); the gap-trend baseline is dropped at
  the same time so the first material after a stop reports `UNKNOWN` rather than
  a trend spanning the stop.

This is the rig's own driver/car state on a fixed cadence, so a consumer always
has current material for its favored drivers through a quiet stint rather than
waiting for a narrative event. It does **not** depend on any other car being
nearby: the `gap_*` fields are simply null in clean air.

`delta_to_best_s` is the last lap against the driver's own best.
`sector_bucket`/`sector_delta_to_best_s` come from the existing
`MicroSectorTracker` spatial anchors — the segment just completed and its time
against the driver's best for that same segment. Gap trends are only computed
when the neighbouring car index is unchanged since the previous material event,
otherwise `UNKNOWN`. `effort` is `PUSHING` when the driver is within threshold
of their own sector/lap best or closing on the car ahead, else `HOLDING`.

Example:

```json
{
  "event_type": "DRIVER_MATERIAL",
  "lap": 12,
  "session_time": 1234.5,
  "player_car_idx": 7,
  "position": 5,
  "laps_completed": 11,
  "lap_dist_pct": 0.51,
  "last_lap_time_s": 88.2,
  "best_lap_time_s": 87.9,
  "gap_ahead_s": 1.4,
  "car_ahead_idx": 3,
  "gap_behind_s": 2.8,
  "car_behind_idx": 9,
  "delta_to_best_s": 0.3,
  "sector_bucket": 4,
  "sector_delta_to_best_s": -0.12,
  "gap_ahead_trend": "CLOSING",
  "gap_behind_trend": "OPENING",
  "effort": "PUSHING",
  "on_pit_road": false,
  "track_surface": 3,
  "speed_mps": 61.5,
  "fuel_level_l": 42.25,
  "incident_count": 4,
  "session_state": 4,
  "interval_s": 25.0
}
```

### `SESSION_RESET`

Trigger:

- The publisher observes a new `sub_session_id` while a previous one was active,
  emitted against the **new** session id before any of its events
  (`reason: "sub_session_changed"`).
- The session clock restarts inside one sub-session — `session_time` drops by
  more than 5 s with the same `sub_session_id` and `session_num`
  (`reason: "session_clock_restarted"`, carrying `previous_session_time`).

For a sub-session change, every car index, battle, and rig-to-car binding cached
for `previous_sub_session_id` is stale from this event onward.

Example:

```json
{
  "event_type": "SESSION_RESET",
  "lap": 0,
  "session_time": 12.0,
  "previous_sub_session_id": 88087370,
  "sub_session_id": 88087411,
  "previous_session_num": 2,
  "session_num": 0,
  "previous_session_time": null,
  "reason": "sub_session_changed"
}
```

Session clock restart inside one sub-session:

```json
{
  "event_type": "SESSION_RESET",
  "lap": 0,
  "session_time": 1.05,
  "previous_sub_session_id": 88087370,
  "sub_session_id": 88087370,
  "previous_session_num": 0,
  "session_num": 0,
  "previous_session_time": 1983.23,
  "reason": "session_clock_restarted"
}
```

## Variants Defined But Not Currently Emitted

### `BRAKING_PROFILE` runtime status

`BRAKING_PROFILE` has working detector logic, but `NarrativeEngine` currently initializes `BrakingProfileDetector::new()` with no configured `heavy_braking_anchors`.

- This means no heavy bucket is ever recognized.
- In current runtime defaults, `BRAKING_PROFILE` is effectively dormant unless anchor configuration is introduced.

## Accuracy Notes

1. Event field names in examples follow the serialized `RaceEvent` shape (`snake_case` payload fields with `event_type` discriminator).
2. Actual transmitted payloads are wrapped in `PublisherEvent` envelope (`src/publisher_event.rs`) with additional context and enrichment.
3. Car-scoped events may be temporarily buffered in publisher until roster has non-empty `driver_name`.
4. Publishing is paused until `sub_session_id` resolves (`sub_session_id != 0`).
