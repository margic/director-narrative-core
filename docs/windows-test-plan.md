# Windows Test Plan — 60 Hz iRacing Live Integration

**Branch:** `issue-17-iracing-live-mmap`  
**Issue:** [#17](https://github.com/margic/director-narrative-core/issues/17)  
**Date:** 2026-05-26

---

## Context

This branch adds a Rust-native Windows shared-memory reader that connects directly
to iRacing's memory-mapped file (`Local\IRSDKMemMapFileName`) at 60 Hz, replacing
the Python `irsdk` bridge that previously handled this. The implementation lives
entirely in `src/irsdk/` and is `#[cfg(target_os = "windows")]`-gated so Linux
CI is unaffected.

### What was built

| File | Purpose |
|---|---|
| `src/irsdk/header.rs` | Parses `irsdk_header` C struct; builds `VarIndex` (name → byte offset map) |
| `src/irsdk/reader.rs` | Typed read helpers + `build_frame()` from raw buffer bytes |
| `src/irsdk/mod.rs` | `IrsdkReader` — `MapViewOfFile`, `WaitForSingleObject`, `Drop` |
| `src/bin/publisher.rs` | Main loop: mmap reader + engine + HTTP transport |

### How it works

```
iRacing (Windows)
  └─ writes telemetry to Local\IRSDKMemMapFileName @ 60 Hz
  └─ signals Local\IRSDKDataValidEvent on each write

publisher.exe (Rust main loop)
  └─ IrsdkReader::wait_for_frame()  ← 0% CPU when idle
  └─ reads mmap → builds TelemetryFrame
  └─ engine.process_frame() → Vec<RaceEvent>
  └─ build_event() → PublisherEvent → transport.enqueue()
  └─ every 500 ms: POST /api/publisher/v2/ingest
```

### Anchor-count bootstrap

The engine starts with `anchor_count = 108` (Nürburgring default, ~540 s / 5 s).
After the first completed lap (`LapLastLapTime > 0`), the thread recomputes
`floor(lap_time / 5.0).max(10)` and rebuilds the engine if the value differs.
Lap 1 is a warm-up lap with no regression data — no state is lost.

---

## Prerequisites (install once)

```powershell
# Rust toolchain + MSVC target
winget install Rustlang.Rustup
rustup target add x86_64-pc-windows-msvc

# Visual Studio Build Tools (C++ workload — provides the MSVC linker)
winget install Microsoft.VisualStudio.2022.BuildTools
# During install select: "Desktop development with C++"
```

---

## Build

```powershell
git clone https://github.com/margic/director-narrative-core
cd director-narrative-core
git checkout main

# Build the publisher binary
cargo build --bin publisher
# Release build (ships to rig):
cargo build --release --bin publisher
# Output: target\debug\publisher.exe  (or target\release\publisher.exe)
```

---

## Test 1 — Unit tests (no iRacing required)

These run everywhere. The `header.rs` and `reader.rs` tests use synthetic
in-memory buffers with no Windows API calls.

```powershell
cargo test
```

**Expected:** all 26 tests pass (20 existing + 6 new irsdk unit tests).

```
test result: ok. 26 passed; 0 failed
```

---

## Test 2 — Live session with iRacing AI race

### Setup

1. Open iRacing and create an **AI race session**:
   - Any track works. Nürburgring Combined (~540 s laps) gives the richest
     narrative signal; a shorter track (~90 s laps) produces events faster.
   - Add at least 10 AI opponents.
   - Use a standing or rolling start — do not use a time trial (no opponents).

2. Start the race and let it run for at least one full lap before starting the
   publisher (so `LapLastLapTime` is populated for anchor-count bootstrap).

### Run the publisher

```powershell
# From the repo root (ensure publisher.toml is present — see src/config.rs):
target\debug\publisher.exe
```

### Expected output sequence

```
[publisher] config loaded — rig=rig-mypc api=https://simracecenter.com
[publisher] waiting for iRacing...
[publisher] connected — playerCarIdx=3, car=#42 Paul Crofts
[publisher] registered with Race Control
[publisher] publishing at 60 Hz (batch every 500ms)
```

After the green flag:

```
[publisher] RACE_GREEN — lap 1 underway
```

After 2–3 laps running near an opponent:

```json
{
  "event_type": "BATTLE_ENGAGED",
  "lap": 2,
  "session_time": 185.3,
  "car_idx": 12,
  "gap_s": 0.8,
  "car_race_position": 3
}
```

After 3–4 laps of consistent closing on the same opponent:

```json
{
  "event_type": "BATTLE_CLOSING",
  "lap": 3,
  "session_time": 274.1,
  "car_idx": 12,
  "closing_rate_sec_per_lap": 0.42,
  "slope_info": {
    "median_slope": -0.42,
    "anchors_qualifying": 14,
    "anchors_agreeing": 11,
    "hotspot_lap_dist_pct": 0.31
  }
}
```

### Things to observe

| What to check | How |
|---|---|
| Anchor count adapts to track | Log line shows computed `anchor_count` matching `floor(lap_time / 5)` |
| No BATTLE_CLOSING during yellow flags | `BATTLE_CLOSING` should not fire when session flag is CAUTION |
| `OVERTAKE` fires on position change | Pass an opponent — event should appear within ~16 ms (1 frame) |
| `PIT_ENTRY` / `PIT_EXIT` fire | Pit on lap 3 — events should appear as you enter/exit pit lane |
| Ctrl-C exits cleanly | `PUBLISHER_GOODBYE` sent; process exits 0 |

---

## Test 3 — Reconnect

```powershell
target\debug\publisher.exe   # starts waiting
# Launch iRacing → "connected" message appears
# Close iRacing (Alt-F4 or crash)
# → "[publisher] iRacing disconnected — waiting..." printed
# Reopen iRacing and rejoin a session
# → "[publisher] connected" printed again, events resume
```

---

## Test 4 — Not-running path (non-Windows guard)

On any Linux/macOS machine (or WSL):

```bash
cargo build --bin publisher
./target/debug/publisher
```

**Expected:**
```
ERROR: [publisher] only supported on Windows (iRacing is Windows-only)
```

---

## Offline fallback (no iRacing)

The JSONL replay mode works on all platforms via `cargo test`:

```powershell
python scripts\synthesize_test_fixture.py
cargo test
```

---

## Known limitations / follow-up issues

| Limitation | Notes |
|---|---|
| `lap` field is `u8` — caps at lap 255 | Sufficient for all current racing formats |
| Anchor count computed from `LapLastLapTime` | First lap events use 108-anchor default; rebuild is seamless |
| No `SessionInfo` YAML parsing | Driver names not available — only `car_idx` integers in events |
| Windows CI runner not yet configured | Manual test on Windows required; Linux CI covers all non-platform logic |
| `CarIdxF2Time` not used | Engine uses `CarIdxLapDistPct` for gap calculation (correct for both live and replay) |

---

## File map for continuing this work

```
src/                    ← pure Rust logic (platform-agnostic, never changes for this feature)
  engine.rs             ← NarrativeEngine::process_frame() — the core state machine
  telemetry_frame.rs    ← TelemetryFrame struct (what iRacing data looks like in Rust)
  anchor_sampler.rs     ← first-crossing per (lap, bucket, car_idx)
  regression_store.rs   ← OLS slope per (bucket, car_idx)
  battle_state.rs       ← IDLE → TRACKING → PUSH → ATTACK_SETUP FSM

napi/src/
  lib.rs                ← NAPI boundary: TelemetryFrame (JS), RaceEvent (JS), NarrativeEngine class
  irsdk/
    mod.rs              ← IrsdkReader (Windows mmap handle, Drop)
    header.rs           ← irsdk_header parsing, VarIndex
    reader.rs           ← build_frame() from raw bytes
    thread.rs           ← LiveSession background thread

listener/
  live.js               ← Windows live entry point (uses startLive/stopLive)
  index.js              ← JSONL batch entry point (unchanged)

docs/
  architecture.md       ← why spatial anchors? accordion effect explained
  narrative-engine-spec.md ← full 15-section spec
  data-models.md        ← Rust struct definitions
  test-harness.md       ← fixture format, CI integration
  windows-test-plan.md  ← this file
```
