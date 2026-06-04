# iRacing Shared Memory Reference

This project reads iRacing's Windows shared-memory feed directly. The publisher attaches to the same two Windows objects iRacing exposes to telemetry tools:

- `Local\IRSDKMemMapFileName`: memory-mapped file containing the header, variable catalog, `SessionInfo` YAML, and a 4-buffer telemetry ring
- `Local\IRSDKDataValidEvent`: event signaled whenever a fresh telemetry frame is written

That interface matters because every detector in the Rust engine is downstream of the exact bytes in that mapping. If the mmap payload is incomplete or inconsistent, the detectors are blind even if a higher-level scenario description sounds plausible.

## Layout

At a high level the mapping is:

```text
0x0000  irsdk_header           112 bytes
0x0070  irsdk_varHeader[]      144 bytes each
....    SessionInfo YAML       null-terminated UTF-8
....    varBuf[0] payload      latest telemetry row candidate
....    varBuf[1] payload
....    varBuf[2] payload
....    varBuf[3] payload
```

The Rust parser in the shared-memory bridge module ([src/irsdk/header.rs](../src/irsdk/header.rs)) only needs a subset of the public SDK header:

- `status`: bit 0 means connected
- `sessionInfoUpdate`: monotonic counter; when it changes the publisher reparses the YAML roster
- `sessionInfoLen` and `sessionInfoOffset`: where the YAML blob lives
- `numVars` and `varHeaderOffset`: where the variable catalog starts
- `numBuf` and `bufLen`: telemetry ring-buffer dimensions
- `varBuf[4]`: each entry contains a `tickCount` and a `bufOffset`; the reader uses the highest `tickCount`

The frame reader in [src/irsdk/reader.rs](../src/irsdk/reader.rs) (exported through `sim_bridge::SharedMemReader`) then resolves byte offsets for the variables the engine depends on.

## Variables That Drive Detection

The minimum viable live frame for this repo is not just lap/time. These variables directly control detector behavior:

- `SessionTime`, `SessionTick`, `SessionState`, `SessionNum`, `SessionFlags`
- `PlayerCarIdx`, `PlayerCarPosition`, `OnPitRoad`, `Lap`, `LapDistPct`, `LapLastLapTime`
- `CarIdxLapDistPct`, `CarIdxPosition`, `CarIdxOnPitRoad`, `CarIdxLapCompleted`
- `SessionInfoUpdate` plus a valid `SessionInfo` YAML roster

Optional but useful for richer detector coverage:

- `FuelLevel` for fuel projection
- `Throttle`, `Brake`, `Speed` for lift/coast and braking-profile detectors
- `LFtempM`, `RFtempM`, `LRtempM`, `RRtempM` for tire-degradation validity
- `CarIdxTrackSurface` for future track-state or off-track logic

## Why JSONL Fixtures Are Not Enough

The JSONL fixture path is still useful for deterministic replay, but it does not exercise the same integration boundary as live iRacing. In particular:

- the publisher connects to a Windows named mapping and event, not a file stream
- `SessionInfoUpdate` and the YAML roster are part of the live contract
- `CarIdx*` arrays must stay internally consistent across positions, lap distance, pit state, and lap-completed counters
- session rollover is represented by `SessionNum` and `SessionInfoUpdate`, not by opening a new file

That is why the mock shared-memory publisher exists in addition to the JSONL fixture generator.

## New Tools In This Repo

### 1. Mock live mmap publisher

[scripts/mock_iracing_mmap.py](../scripts/mock_iracing_mmap.py)

Publishes a synthetic weekend into the same Windows objects the Rust publisher reads. The default `detector-smoke` scenario includes:

- practice, qualifying, and race segments
- `SessionNum` rollover with `SessionInfoUpdate` increments
- a persistent opponent ahead with shrinking lap-over-lap gap
- lap-crossing position gains to trigger `OVERTAKE`
- a brief local yellow
- a brief pit-road excursion
- live throttle, brake, speed, fuel, and tire-temp signals

Run it on Windows:

```powershell
python scripts\mock_iracing_mmap.py --scenario detector-smoke
```

Important: the live mock uses the same object names as real iRacing, so iRacing must be closed first. If you only need a reference artifact, use `--snapshot-out` instead of the live publisher.

Then start the publisher in a second terminal:

```powershell
cargo run --bin publisher
```

### 2. Snapshot exporter

[scripts/iracing_mmap_diag.py](../scripts/iracing_mmap_diag.py)

The diagnostic reader can freeze the current mapping to disk for inspection or documentation:

```powershell
python scripts\iracing_mmap_diag.py \
  --export exports\mock_irsdk_snapshot.bin \
  --manifest exports\mock_irsdk_snapshot.json
```

`mock_irsdk_snapshot.bin` is the raw byte-for-byte mapping. `mock_irsdk_snapshot.json` is a parsed manifest containing header values, key telemetry variables, and a YAML preview.

### 3. Standalone exporter wrapper

[scripts/export_iracing_mmap.py](../scripts/export_iracing_mmap.py)

This is a thin wrapper around the diagnostic parser that writes the raw `.bin` and companion `.json` in one command:

```powershell
python scripts\export_iracing_mmap.py exports\live_irsdk_snapshot.bin
```

### 4. Windows smoke test (mock + publisher)

[scripts/windows_smoke_test.py](../scripts/windows_smoke_test.py)

Runs the full Windows integration path in one command:

1. Starts the mock mmap publisher.
2. Launches `publisher.exe --dry-run --no-ui`.
3. Parses emitted envelopes and counts event types.
4. Fails unless minimum event coverage is met.

The smoke harness uses unique mapping/event names per run and sets `SIM_MMAP_NAME` and `SIM_EVENT_NAME` for the publisher, so it can run without colliding with a live iRacing mapping.

```powershell
python scripts\windows_smoke_test.py --scenario detector-smoke --time-scale 12
```

Expected required coverage:

- `PUBLISHER_HELLO`
- `RACE_GREEN`
- `BATTLE_ENGAGED`
- at least one overtake (`OVERTAKE` or `OVERTAKE_FOR_LEAD`)
```

For a deterministic synthetic artifact with no named-object collision risk:

```powershell
python scripts\mock_iracing_mmap.py \
  --snapshot-out exports\mock_irsdk_snapshot.bin \
  --manifest-out exports\mock_irsdk_snapshot.json \
  --snapshot-only
```

## Detector Confidence Workflow

If the question is "how do we know the detectors are actually detecting?", use this stack from lowest to highest fidelity:

1. Unit tests on the pure Rust helpers.
2. JSONL replay for deterministic regression tests.
3. Mock mmap publishing for full publisher/UI/session-info integration.
4. Live iRacing for final parity validation.

For mock-mmap validation specifically, insist on three things:

1. Observable cause in the synthetic signal.
2. Expected `RaceEvent` emitted by the publisher.
3. Matching exported mmap snapshot proving the upstream bytes were present.

That avoids a common failure mode where we blame the detector logic when the real problem is missing or stale upstream telemetry.

## What To Assert For Each Detector

- `RACE_GREEN` and `RACE_CHECKERED`: `SessionState` must cross into `4` and `5`
- Session/UI rollover: `SessionNum` must change and `SessionInfoUpdate` must increment
- `LAP_COMPLETED`: `Lap` must increment while `PlayerCarPosition > 0`
- `OVERTAKE`: `PlayerCarPosition` must improve between lap completions and `CarIdxPosition` must stay consistent for the field
- Battle detectors: the same opponent car must remain ahead or behind with gap deltas inside the 5-second battle window across consecutive laps
- Pit detectors: `OnPitRoad` must toggle with a stable player identity and lap context
- Yellow detectors: `SessionFlags` must transition, ideally with nearby-car context in the arrays

## Recommended Next Step

The next step is to make detector expectations executable instead of manual:

- add a small fixture manifest describing expected event types and minimum counts for each mock scenario
- run the publisher against the mock mmap in CI or a Windows smoke test
- compare emitted event types against that expectation contract

That gives us a fast answer to "did the detector stop seeing the world?" before we ever join a real session.