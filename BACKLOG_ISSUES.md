# SRC Telemetry Engine Backlog Issues

This backlog captures issue-ready work items derived from the prototype roadmap.

## Phase 1: Data Science & Feature Engineering

### Issue 1: Define human heuristics and extract first derivative metrics
- **Phase:** 1
- **Milestone:** 1
- **Goal:** Define racecraft heuristics and compute velocity/closing-rate features from telemetry streams.
- **Deliverables:**
  - Heuristic definitions for identifying dynamic race state shifts
  - First-derivative feature extraction pipeline (velocity and relative closing rate)
  - Validation notes against replay data samples

### Issue 2: Extract second derivative aggression signals
- **Phase:** 1
- **Milestone:** 2
- **Goal:** Quantify aggression using acceleration and change-of-closing-rate features.
- **Deliverables:**
  - Second-derivative metrics (acceleration, jerk-like aggression proxies)
  - Event thresholds for ramping pressure vs. stable following
  - Initial false-positive analysis

### Issue 3: Implement rolling variance for accordion-effect detection
- **Phase:** 1
- **Milestone:** 3
- **Goal:** Measure local volatility to detect pack compression/expansion dynamics.
- **Deliverables:**
  - Rolling variance calculations over configurable windows
  - Accordion-effect signal definition
  - Telemetry replay plots and summary metrics

## Phase 2: Rust Fundamentals for Streaming Data

### Issue 4: Document ownership, borrowing, and lifetime strategy
- **Phase:** 2
- **Milestone:** 1
- **Goal:** Establish safe memory patterns for streaming telemetry processing.
- **Deliverables:**
  - Ownership and borrowing guidelines for the telemetry engine
  - Lifetime design notes for shared/stateful components
  - Example code snippets demonstrating safe patterns

### Issue 5: Build `VecDeque` ring buffer for O(1) time-series memory
- **Phase:** 2
- **Milestone:** 2
- **Goal:** Implement bounded, high-throughput buffering for rolling computations.
- **Deliverables:**
  - `VecDeque`-based ring buffer abstraction
  - O(1) push/pop behavior for sliding windows
  - Basic correctness/performance tests

### Issue 6: Model race-state machine with Rust enums
- **Phase:** 2
- **Milestone:** 3
- **Goal:** Encode narrative-relevant race states using algebraic enums.
- **Deliverables:**
  - Enum-based state definitions and transitions
  - Deterministic transition logic
  - Test scenarios for core transition paths

## Phase 3: Edge Prototype Integration

### Issue 7: Set up N-API bindings with `napi-rs`
- **Phase:** 3
- **Milestone:** 1
- **Goal:** Expose Rust telemetry engine capabilities to Node.js/Electron.
- **Deliverables:**
  - `napi-rs` project scaffolding and build configuration
  - Typed API surface for core telemetry outputs
  - Smoke test from Node.js entry point

### Issue 8: Build JSON-line replay harness
- **Phase:** 3
- **Milestone:** 2
- **Goal:** Reproduce race sessions deterministically for validation and iteration.
- **Deliverables:**
  - JSONL ingestion and playback controls
  - Replay timing utilities for accelerated and real-time modes
  - Fixture set for representative race scenarios

### Issue 9: Emit high-level narrative events to Node.js listener
- **Phase:** 3
- **Milestone:** 3
- **Goal:** Stream semantic race events to downstream narrative consumers.
- **Deliverables:**
  - Event schema for battle, undercut, and tension-state narratives
  - Mock Node.js listener integration
  - End-to-end demo from replay input to emitted events
