# Publisher Event Contract

Wire contract for events published to `POST /api/publisher/v2/ingest`, built by
`build_event` in `src/publisher_event.rs`.

Every envelope carries `contractVersion` (`PAYLOAD_CONTRACT_VERSION`).
**Consumers should key off `contractVersion >= 2`** for everything described in
this document; version 1 is the implicit pre-existing shape (envelope
`rigId` + `car` only). The revision is additive: no field present at version 1
was removed or repurposed.

---

## 1. Envelope

| Field | Notes |
| --- | --- |
| `id` | UUID v4, generated per built event. |
| `eventKey` | `v2-<subSessionId>-<sessionTick>-<TYPE>-<sequence>`. Stable idempotency key, unique even for a burst published on one tick. |
| `sequence` | Monotonic per-process counter; orders events sharing a tick. Strictly increasing within one `publisherRunId`; a gap is a lost or reordered event, a reset to a low value with a new `publisherRunId` is a publisher restart. |
| `emittedAt` | Same instant as `timestamp`, rendered RFC 3339 UTC with millisecond precision (`2026-09-02T23:28:39.745Z`). Compare against an ISO `receivedAt` to measure delivery lag. Additive. |
| `publisherRunId` | UUID minted once per publisher process. Additive. |
| `contractVersion` | Payload contract revision (currently `2`). |
| `raceSessionId`, `rigId`, `type`, `timestamp`, `sessionTime`, `sessionTick`, `scope` | Unchanged from version 1. |
| `publisher` | Identity of the rig that sent the event (§2). |
| `subject` | Identity of the car the event is *about* (§3). Absent for session-wide events. |
| `car` | Unchanged from version 1: roster entry for car-scoped events. |
| `payload`, `context` | Unchanged, payload extended (§2, §3, §4). |

Deduplication should use `eventKey` (or `id`), **not**
`(raceSessionId, sessionTick, type)` — that triple is not unique, since several
events of one family can be published on a single tick.

## 2. Publisher identity — on every event

`publisher` on the envelope, and mirrored into every payload, race and system
events alike:

```json
{
  "publisher": {
    "rigId": "rig-rig1",
    "rigLabel": "rig-rig1",
    "carIdx": 3,
    "carNumber": "42",
    "driverId": "user:512345",
    "driverName": "Pat Crofts"
  },
  "rigId": "rig-rig1",
  "rigLabel": "rig-rig1",
  "publisherCarIdx": 3,
  "publisherCarNumber": "42",
  "publisherDriverId": "user:512345",
  "publisherDriverName": "Pat Crofts"
}
```

`carIdx` is re-resolved per event, so a mid-session index reassignment cannot
leave a stale identity on the wire. `driverId` is durable
(`user:<iRacing user id>`, falling back to `name:<lowercased driver name>`) and
is the field to bind a driver across sessions.

## 3. Subject identity and role vocabulary — on every event

The car an event is about is always named, and is deliberately separate from the
publishing rig — they are frequently different cars.

```json
{
  "subjectRole": "ATTACKER",
  "subject": { "role": "ATTACKER", "carIdx": 0, "carNumber": "7", "driverId": "user:1", "driverName": "…", "car": { … } },
  "subjectCarIdx": 0,
  "subjectCarNumber": "7",
  "subjectDriverId": "user:1",
  "attackerCarIdx": 0, "attackerCarNumber": "7", "attackerDriverId": "user:1", "attackerCar": { … },
  "defenderCarIdx": 5, "defenderCarNumber": "12", "defenderDriverId": "user:2", "defenderCar": { … }
}
```

Roles (`SubjectRole`), one vocabulary across all families:

| Role | Meaning |
| --- | --- |
| `ATTACKER` | Car closing, catching, or passing. Subject of battle, horizon, traffic, compression, and overtake families. |
| `DEFENDER` | Car being caught, held up, or passed. Subject of the vulnerability family. |
| `DRIVER` | The publishing rig's own driving: laps, pit, tires, fuel, braking, micro-sectors, driver swaps, focus requests. |
| `INCIDENT` | Car involved in an incident or cluster. |
| `TRIGGER` | Car that triggered a flag condition. |
| `RIG` | Publisher lifecycle and control events; the rig's own car is still named. |
| `SESSION` | Session-wide event; no subject car (`subject` absent, `subjectCarIdx` null). |

Family-specific fields (`leader_car_idx`, `traffic_car_idx`,
`battle_attacker_idx`, `battle_defender_idx`, …) are unchanged and still
published; `attacker*`/`defender*` are the canonical names to read.

## 4. Payload naming

**camelCase is canonical.** Every payload key is published in camelCase; the
historical snake_case keys are retained alongside for the transition window so
the production consumer keeps working. Hand-written aliases (e.g. `lapTime`)
take precedence over mechanically generated twins.

`INCIDENT_ALERT` now publishes the documented `trackSurface` (the surface the car
is on *now*) alongside the pre-existing `currentTrackSurface` /
`current_track_surface`.

Fields whose snake_case spellings are expected to be dropped in a later,
breaking revision — not in this one: all snake_case payload twins, and
`currentTrackSurface`.

## 5. Incident semantics

| Field | Meaning |
| --- | --- |
| `severity` / `severityScore` | Raw magnitude; **units depend on `reason`** — m/s of speed drop scaled for surface/speed reasons, raw iRacing incident points (1/2/4x) for `incident_count_increase`. Unchanged. |
| `severityNormalized` | Always `0.0`–`1.0`. This is what a quality floor should compare against. |
| `incidentCountDelta` | Incident points gained, when `reason` is `incident_count_increase`; null otherwise. |

Noise is filtered at source (`src/basic_incident.rs`): incident-point increases
are only published when the car is in the world, not on pit road, and was moving
(≥ 5 m/s). Surface/speed-drop alerts already required not being in a pit
transition.

## 6. Periodic material and session lifecycle

Added in the same additive contract (v2); no existing field changed.

| Event | Scope | Purpose |
| --- | --- | --- |
| `DRIVER_MATERIAL` | car | The publishing rig's own driver/car state on a wall-clock cadence (default 25 s, `publisher.driver_material_interval_ms` / `PUBLISHER_DRIVER_MATERIAL_INTERVAL_MS`, `0` disables). Position, laps completed, last/best lap, nearest car ahead/behind with gaps, pit road, surface, speed, fuel, incident count, and the actual `interval_s` since the previous one. Prefer this over inferring freshness from narrative events: a quiet stint still produces material every cadence. |
| `SESSION_RESET` | session | The publisher saw a new sub-session **or** the session clock restarting inside one. Carries `previousSubSessionId`/`previousSessionNum`/`previousSessionTime` and the new `subSessionId`/`sessionNum` plus a machine-readable `reason` (`sub_session_changed` or `session_clock_restarted`). For a sub-session change every cached car index, battle, and rig-to-car binding for the previous sub-session is stale from here. |
| `RACE_CHECKERED` | session | Now also fires on the checkered flag bit and on a Racing -> CoolDown state jump, once per session, not only on `SessionState::Checkered`. |
| `PIT_EXIT` | car | Now detected before the unclassified-car guard, so a stall-bound car reporting `position == 0` still emits its entry/exit pair. |
| `RACE_GREEN` / `RACE_CHECKERED` | session | Now carry `synthetic` (bool) and `origin` (`SESSION_STATE_TRANSITION` or `CONNECT_SNAPSHOT`). A connect-time snapshot of the current session state is `synthetic: true` and must not be read as a real flag — this is what produced four `RACE_GREEN` events in a Practice session. |

### `DRIVER_MATERIAL` payload

Beyond the raw car state above, each material event carries the driver's own
reference frame so a consumer can narrate a car running alone:

| Field | Meaning |
| --- | --- |
| `deltaToBest` | Last lap minus the driver's own best lap, seconds; null until both exist. |
| `sectorBucket` / `sectorDeltaToBest` | The most recently completed spatial anchor segment (from the existing `MicroSectorTracker`) and its time versus the driver's best for that same segment. |
| `gapAhead` / `carAhead`, `gapBehind` / `carBehind` | Nearest car ahead/behind (resolved car reference) and the gap in seconds; null in clean air. |
| `gapAheadTrend` / `gapBehindTrend` | `CLOSING`, `OPENING`, `STABLE`, or `UNKNOWN`. Trend is only computed against the *same* neighbouring car index as the previous cadence tick; otherwise `UNKNOWN`. |
| `effort` | `PUSHING` or `HOLDING`, derived from sector/lap pace against the driver's own best and from a closing gap ahead. |
| `interval_s` | Actual wall-clock seconds since the previous material event. |

Material is suppressed while the car is on pit road (including the box) or not
in the world (`CarIdxTrackSurface < 0`), and the trend baseline is dropped at
that point so a stop never produces a fabricated trend. It is emitted
regardless of proximity to other cars.

## 7. Battle identity (third-party pairs)

Additive, same contract version (v2). `BATTLE_ENGAGED`, `BATTLE_CLOSING` and
`BATTLE_BROKEN` are now emitted for **every close pair near the publishing
rig** (any two cars within `SCAN_FIELD_POSITIONS` race positions of the rig,
including pairs the rig is not part of), not only for the rig's own threat.
The legacy player-threat path is unchanged and remains the sole source of
events for pairs the rig is part of; when the pair tracker also holds that
pair, those events carry the identity fields below so a consumer sees one
`battleId` per fight, never two events for the same pair.

Existing fields keep their names. For a pair the rig is not part of,
`player_car_idx` carries the **behind** car and `opponent_car_idx` the
**ahead** car, so `leaderCar`/`followerCar`, `attacker*`/`defender*` and the
`car` envelope derive exactly as before. Use `publisherCarIdx` for the rig.

Identity fields (absent when no pair is tracked, so an older consumer sees
today's payload unchanged):

| Field | Meaning |
| --- | --- |
| `battleId` | Stable for the life of one engagement; a new id is minted if the fight breaks and re-forms. |
| `battlePhase` | `ENGAGED`, `CLOSING` or `BROKEN` — the lifecycle position of this event. |
| `aheadCarIdx` / `behindCarIdx` | Explicit roles, re-evaluated each frame; an overtake swaps them without changing `battleId`. |
| `engagedAt` | Session time the pair engaged. |
| `battleAgeS` | Seconds since `engagedAt`. |
| `currentGapS` | Latest gap between the two cars in seconds; null once the pair is no longer observable. |
| `closingRateSPerLap` | Positive = the behind car is closing, seconds per lap over a ~20 s window; null until enough samples exist. |
| `battleConfidence` | `0.0`–`1.0`; grows with samples, discounted while the lap time is only an estimate. |
| `battleInvolvesPublisher` | `true` when the rig is one of the two cars. |
| `battleBreakReason` | `BROKEN` only: `GAP_OPENED`, `CAR_PITTED` or `CAR_LEFT_WORLD`. |

Lifecycle: one `ENGAGED` after five consecutive close frames, `CLOSING`
updates rate-limited to one per 10 s while the behind car is measurably
closing, and exactly one `BROKEN` per `battleId`.

## 8. Session lifecycle and sub-session stamping

Additive; no existing field or type changed.

### `SESSION_STATE` (new, session-scoped)

Which session this is and what phase it is in, emitted once on connect
(`synthetic: true`), on every iRacing `SessionNum` or `SessionState` change,
when the `SessionType` string first resolves, and once when the session clock
enters its final minute.

| Field | Meaning |
| --- | --- |
| `sessionNum`, `previousSessionNum` | iRacing `SessionNum`. |
| `sessionType` | Raw `SessionInfo` `SessionType` (`Practice`, `Lone Qualify`, `Open Qualify`, `Race`, ...); `null` until parsed. |
| `sessionKind` | `PRACTICE`, `QUALIFYING`, `RACE` or `UNKNOWN`, classified from `sessionType`. |
| `sessionState`, `sessionStateName`, `previousSessionState` | iRacing `SessionState` raw value and name (`INVALID`, `GET_IN_CAR`, `WARMUP`, `PARADE_LAPS`, `RACING`, `CHECKERED`, `COOL_DOWN`). |
| `phase`, `previousPhase` | Broadcast phase derived from kind x state: `PENDING`, `PRACTICE_OPEN`, `QUALIFYING_HOTLAPS`, `QUALIFYING_CLOSED`, `RACE_GRIDDING`, `RACE_FORMATION`, `RACE_GREEN`, `RACE_CHECKERED`, `COOL_DOWN`. |
| `reason` | `CONNECT_SNAPSHOT`, `SESSION_NUM_CHANGED`, `SESSION_STATE_CHANGED`, `SESSION_TYPE_RESOLVED`, `SESSION_ENDING`. |
| `synthetic` | `true` for the connect snapshot only. |
| `sessionTimeRemainS` | iRacing `SessionTimeRemain`; `null` when absent or untimed. |
| `sessionLapsRemain` | iRacing `SessionLapsRemainEx`; `null` when absent or unlimited. |
| `sessionChangeImminent` | `true` when the state is `CHECKERED`/`COOL_DOWN` or the clock is inside the final 60 s. |

iRacing exposes no explicit "next session begins in N seconds" variable; the
imminent-change signal is derived from the state machine and the session
clock, and is the closest the SDK provides. Qualifying hotlap start/end for
the rig's own car is already carried by `PIT_EXIT` / `LAP_COMPLETED` /
`PIT_ENTRY`; read them under `phase == QUALIFYING_HOTLAPS`.

### `PUBLISHER_GOODBYE` additions

| Field | Meaning |
| --- | --- |
| `reason` | `shutdown` or `session_transition`. |
| `previousSubSessionId` | The sub-session the publisher is leaving; only set for `session_transition`. |

### Sub-session stamping rule

Every event built after iRacing advances to a new sub-session carries the
*live* `subSessionId` in `raceSessionId`, `eventKey` and `context`. This
includes the transition `PUBLISHER_GOODBYE` and `SESSION_RESET`: the departed
id lives in their payload (`previousSubSessionId`), never in the envelope. A
session clock running backwards forces an immediate `SessionInfo` re-read so
the `session_clock_restarted` reset cannot be stamped with an id that has
already been superseded.
