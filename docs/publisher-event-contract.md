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
| `sequence` | Monotonic per-process counter; orders events sharing a tick. |
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
| `SESSION_RESET` | session | The publisher saw a new sub-session. Carries `previousSubSessionId`/`previousSessionNum` and the new `subSessionId`/`sessionNum` plus a `reason`, and is published against the new session before any of its other events. Every cached car index, battle, and rig-to-car binding for the previous sub-session is stale from here. |
| `RACE_CHECKERED` | session | Now also fires on the checkered flag bit and on a Racing -> CoolDown state jump, once per session, not only on `SessionState::Checkered`. |
| `PIT_EXIT` | car | Now detected before the unclassified-car guard, so a stall-bound car reporting `position == 0` still emits its entry/exit pair. |
