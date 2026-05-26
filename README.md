# director-narrative-core

A Rust-native edge telemetry engine for the Sim RaceCenter broadcast agent. Ingests iRacing telemetry at 5Hz and translates raw physics (gaps, lap times, positions) into semantic narrative events (`PUSH`, `ATTACK_SETUP`, `OVERTAKE`, `CLOSE_APPROACH`, `LAP_COMPLETE`, `PIT_ENTRY`, `PIT_EXIT`) for consumption by the AI Director.

The core innovation is **spatial anchoring**: rather than sliding a time window over the raw gap (which is dominated by accordion noise from braking zones), the engine samples the gap at fixed track locations and runs per-anchor OLS regression over successive laps. Accordion noise cancels exactly at each anchor — what remains is a clean closing-rate signal.

Validated against a real 47-car Nürburgring race (35.5 minutes, 9985 frames) and against a synthetic 10-lap test fixture that exercises every battle state.

## Getting Started

### Prerequisites

- **Rust** (edition 2021) — `rustup` + `cargo` on PATH
- **Node.js** ≥ 18 — for the listener and native module
- **Python 3** — for generating test fixtures and EDA scripts

### 1. Clone and build

```bash
git clone https://github.com/margic/director-narrative-core
cd director-narrative-core
cargo build -p director-narrative-core-napi
cp target/debug/libdirector_narrative_core_napi.so napi/index.node
```

### 2. Generate the test fixture

```bash
python3 scripts/synthesize_test_fixture.py
# → writes data/test_fixture.jsonl (7000 frames, 10 laps, 5Hz)
```

### 3. Stream narrative events (Node.js listener)

```bash
node listener/index.js data/test_fixture.jsonl
```

Expected key events:

```
{ "eventType": "PUSH",          "lap": 3, "sessionTime": 480, "narrativeContext": { "carAheadIdx": 7, ... } }
{ "eventType": "ATTACK_SETUP",  "lap": 4, "sessionTime": 620, "narrativeContext": { "carAheadIdx": 7, ... } }
{ "eventType": "CLOSE_APPROACH","lap": 6, "sessionTime": 892, "narrativeContext": { "carAheadIdx": 7, ... } }
```

### 4. Run the Rust test suite

```bash
cargo test
# 17 unit tests + 3 integration tests (PUSH @ lap 3, ATTACK_SETUP @ lap 4, CLOSE_APPROACH @ lap 6)
```

### Python EDA (optional)

The Python prototype in `scripts/` validates the spatial-anchor approach against the real Nürburgring session. It requires `data/session.jsonl` (gitignored, 31 MB).

```bash
# Against the synthetic test fixture
JSONL_PATH=data/test_fixture.jsonl python3 scripts/prototype_narrative.py

# Against real Nürburgring data (requires data/session.jsonl)
python3 scripts/prototype_narrative.py
```

---

## Documentation

| Document | Description |
|---|---|
| [docs/architecture.md](docs/architecture.md) | Why spatial anchors? The accordion effect explained, component diagram, validated outputs |
| [docs/narrative-engine-spec.md](docs/narrative-engine-spec.md) | Full 15-section architecture specification: anchoring, regression, state machines, napi contract |
| [docs/data-models.md](docs/data-models.md) | Complete Rust struct definitions with field-level documentation |
| [docs/test-harness.md](docs/test-harness.md) | JSONL fixtures, Rust unit and scenario tests, CI integration |

## Key Design Decisions

**Spatial anchoring over time-window buffering.** The original RFC proposed `bufferTime()` / `pairwise()` over the raw gap stream. The Nürburgring data shows gap derivatives of ±2-4 s/lap from accordion noise alone — this would fire false `PUSH` events in every braking zone. Anchoring at fixed `LapDistPct` checkpoints cancels this noise exactly.

**Identity-coupled regression.** Each `(opponent_car_idx, anchor_bucket)` pair is an independent regression series. When the car ahead changes identity (pit stop, overtake by a third car), the accumulated readings for the previous opponent are discarded. Comparing a gap to Car A on lap 1 with a gap to Car B on lap 2 produces a meaningless slope.

**Two-tier classification.** With 108 anchors at Nürburgring, emitting one event per qualifying anchor would flood the AI narrator. The engine applies a median across all qualifying per-anchor slopes (the WHETHER signal) and surfaces the single steepest-closing anchor as `hotspot_lap_dist_pct` (the WHERE signal).

**5Hz is sufficient.** Narrative signals are lap-scale. A 200ms poll interval produces anchor samples every ~50 metres at racing speeds — finer than any meaningful spatial resolution for broadcast storytelling.

**Rust + napi-rs.** Node.js garbage collection creates unpredictable pauses on a gaming PC running iRacing, OBS, and Discord simultaneously. The Rust core computes all heuristics natively and crosses the napi boundary only when a narrative state change occurs — typically 1-3 events per lap crossing.

## Project Structure

```
director-narrative-core/
├── src/
│   ├── lib.rs                   # Module declarations
│   ├── engine.rs                # NarrativeEngine — process_frame() entry point
│   ├── anchor_sampler.rs        # Per-lap gap sampling at fixed track positions
│   ├── regression_store.rs      # Per-anchor OLS ring buffers
│   ├── battle_state.rs          # BattleState FSM + classify()
│   ├── gap_finder.rs            # find_cars_ahead() / find_cars_behind()
│   ├── lap_timer.rs             # Lap crossing detection
│   ├── race_event.rs            # RaceEvent enum (all narrative output types)
│   ├── replay.rs                # replay_frames() — batch processing
│   ├── telemetry_frame.rs       # TelemetryFrame input struct
│   └── bin/replay.rs            # CLI binary
├── napi/
│   ├── Cargo.toml               # cdylib with napi4 + serde-json features
│   └── src/lib.rs               # NarrativeEngine napi class + TelemetryFrame/RaceEvent JS types
├── tests/
│   └── fixture.rs               # Integration tests: PUSH @ lap 3, ATTACK_SETUP @ lap 4, CLOSE_APPROACH @ lap 6
├── listener/
│   └── index.js                 # Node.js listener: streams JSONL → narrative events
├── scripts/
│   ├── prototype_narrative.py   # Python validation prototype
│   ├── synthesize_test_fixture.py # Generate synthetic JSONL test fixture
│   └── export_replay.py         # Windows: export iRacing replay → JSONL
├── docs/
│   ├── architecture.md          # The accordion problem, spatial anchors, component diagram
│   ├── narrative-engine-spec.md # Full 15-section architecture specification
│   ├── data-models.md           # Rust struct definitions
│   └── test-harness.md          # JSONL fixtures and test guide
└── data/
    ├── session.jsonl            # Real Nürburgring session (gitignored, 31 MB)
    └── test_fixture.jsonl       # Synthetic fixture (gitignored, generated by synthesize_test_fixture.py)
```

## Development Environment

Codespaces / devcontainer with Rust toolchain, Node.js LTS, and GitHub CLI pre-installed. See `.devcontainer/devcontainer.json`.
