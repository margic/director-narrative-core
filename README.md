# director-narrative-core

A Rust-native edge telemetry engine for the Sim RaceCenter broadcast agent. Ingests iRacing telemetry at 5Hz and translates raw physics (gaps, lap times, positions) into semantic narrative events (`PUSH_DETECTED`, `ATTACK_SETUP`, `OVERTAKE_DETECTED`) for consumption by the AI Director.

The core innovation is **spatial anchoring**: rather than sliding a time window over the raw gap (which is dominated by accordion noise from braking zones), the engine samples the gap at fixed track locations and runs per-anchor OLS regression over successive laps. Accordion noise cancels exactly at each anchor — what remains is a clean closing-rate signal.

Validated against a real 47-car Nürburgring race (35.5 minutes, 9985 frames) and against a synthetic 10-lap test fixture that exercises every battle state.

## Documentation

| Document | Description |
|---|---|
| [docs/architecture.md](docs/architecture.md) | Why spatial anchors? The accordion effect explained, component diagram, validated outputs |
| [docs/narrative-engine-spec.md](docs/narrative-engine-spec.md) | Full 15-section architecture specification: anchoring, regression, state machines, napi contract |
| [docs/data-models.md](docs/data-models.md) | Complete Rust struct definitions with field-level documentation |
| [docs/test-harness.md](docs/test-harness.md) | JSONL fixtures, Rust unit and scenario tests, CI integration |

## Quick Start

### Validate the Python prototype

```bash
# Against real Nürburgring data (requires data/session.jsonl)
python3 scripts/prototype_narrative.py

# Against the synthetic test fixture (no iRacing session required)
python3 scripts/synthesize_test_fixture.py
JSONL_PATH=data/test_fixture.jsonl python3 scripts/prototype_narrative.py
```

Expected output for the synthetic fixture:

```
[08:00  L03]  PUSH  slope=-0.4998s/lap  n=28  hotspot@29%
[10:20  L04]  ATTACK_SETUP  slope=-0.5999s/lap  n=28
[12:40  L06]  CLOSE_APPROACH  CarIdx 7 (P10) @ 0.60s
[19:40  L08]  OVERTAKE    P10->P9  (+1)
[19:42  L09]  PIT_ENTRY   P8  ->  IDLE
```

### Run Rust tests (once Rust crate is implemented)

```bash
python3 scripts/synthesize_test_fixture.py   # generate test fixtures
cargo test
```

## Key Design Decisions

**Spatial anchoring over time-window buffering.** The original RFC proposed `bufferTime()` / `pairwise()` over the raw gap stream. The Nürburgring data shows gap derivatives of ±2-4 s/lap from accordion noise alone — this would fire false `PUSH` events in every braking zone. Anchoring at fixed `LapDistPct` checkpoints cancels this noise exactly.

**Identity-coupled regression.** Each `(opponent_car_idx, anchor_bucket)` pair is an independent regression series. When the car ahead changes identity (pit stop, overtake by a third car), the accumulated readings for the previous opponent are discarded. Comparing a gap to Car A on lap 1 with a gap to Car B on lap 2 produces a meaningless slope.

**Two-tier classification.** With 108 anchors at Nürburgring, emitting one event per qualifying anchor would flood the AI narrator. The engine applies a median across all qualifying per-anchor slopes (the WHETHER signal) and surfaces the single steepest-closing anchor as `hotspot_lap_dist_pct` (the WHERE signal).

**5Hz is sufficient.** Narrative signals are lap-scale. A 200ms poll interval produces anchor samples every ~50 metres at racing speeds — finer than any meaningful spatial resolution for broadcast storytelling.

**Rust + napi-rs.** Node.js garbage collection creates unpredictable pauses on a gaming PC running iRacing, OBS, and Discord simultaneously. The Rust core computes all heuristics natively and crosses the napi boundary only when a narrative state change occurs — typically 1-3 events per lap crossing.

## Project Structure

```
scripts/
  prototype_narrative.py       <- Python validation prototype (run this first)
  synthesize_test_fixture.py   <- Generate synthetic JSONL test fixtures
  export_replay.py             <- Windows: export iRacing replay -> JSONL
docs/
  architecture.md              <- Start here: the accordion problem and anchor solution
  narrative-engine-spec.md     <- Full 15-section spec
  data-models.md               <- Rust types
  test-harness.md              <- Testing guide
data/
  session.jsonl                <- Real Nürburgring session (gitignored, 31 MB)
  test_fixture.jsonl           <- Synthetic fixture (gitignored, generated)
```

## Development Environment

Codespaces / devcontainer with Rust toolchain, Node.js LTS, and GitHub CLI pre-installed. See `.devcontainer/devcontainer.json`.
