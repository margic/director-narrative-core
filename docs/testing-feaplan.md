# Director Narrative Core — Testing Feature Plan (FeaPlan)

Status: Draft for implementation
Date: 2026-06-01
Owners: Publisher + Engine maintainers
Related docs: [test-harness.md](test-harness.md), [windows-test-plan.md](windows-test-plan.md), [data-models.md](data-models.md)

---

## 1. Why This Plan Exists

Recent dual-publisher validation exposed two production-facing quality gaps:

1. `LAP_COMPLETED` values were present but inconsistently usable by consumers (`lapTime` expectation mismatch and null/invalid timing episodes).
2. Battle events lacked explicit consumer-facing attribution fields (`leaderCarNumber`/`followerCarNumber`) in payloads.

Both failures came from missing contract-level test coverage at the event envelope boundary, not from core engine correctness alone.

This plan defines a comprehensive test strategy and implementation roadmap to prevent these issues and similar hidden defects before release.

---

## 2. Test Strategy Principles

1. Treat Race Control schema compatibility as a first-class contract.
2. Test at multiple layers: unit, component, integration, replay, and contract.
3. Encode domain invariants explicitly (not just happy-path event presence).
4. Use deterministic fixtures and golden snapshots for reproducibility.
5. Fail CI on contract drift or invalid event quality signals.

---

## 3. Scope

### In scope

1. Event-building schema and payload shape tests.
2. Lap-time extraction and validation behavior.
3. Battle attribution and participant role derivation.
4. Session transition/reconnect behavior affecting event correctness.
5. Replay-driven integration validation for dual-publisher scenarios.
6. CI gates and artifact checks for schema/invariant regressions.

### Out of scope (for this phase)

1. Race Control server-side business logic testing.
2. Full end-to-end cloud deployment checks.
3. UI screenshot/visual regression automation.

---

## 4. Risk-Driven Test Matrix

| Risk | Failure Mode | Test Type | Gate |
|---|---|---|---|
| Contract drift | Consumer expects fields not emitted or misnamed | Contract tests over built `PublisherEvent` JSON | Required |
| Invalid lap timing | null/zero/negative/NaN lap times published | Invariant tests + replay assertions | Required |
| Battle attribution gaps | No explicit leader/follower car numbers | Contract tests + replay checks | Required |
| Session boundary leakage | Old session state bleeds into new subSession | Integration session-transition tests | Required |
| Ordering/consistency drift | Non-monotonic tick/time or malformed context | Stream invariants over replay outputs | Required |
| Retry/resend side effects | Duplicate or mutated payloads across retries | Transport-level behavioral tests | Advisory in Phase 1, required in Phase 2 |

---

## 5. Coverage Model

### Tier A: Fast unit tests (existing + enhanced)

Purpose: isolate deterministic behavior in small components.

Targets:

1. `engine.rs`: lap timing selection and sanitization.
2. `lap_timer.rs`: completed-lap math under wraps, resets, sparse updates.
3. `publisher_event.rs`: field mapping and payload enrichment.
4. `session_info.rs`: roster fallback behavior and car-number resolution.

Additions:

1. Table-driven lap-time validation cases: valid, zero, negative, NaN, infinity.
2. Role derivation cases for leader/follower under missing/equal positions.
3. Field alias tests for snake_case + camelCase compatibility where required.

### Tier B: Contract tests (new, mandatory)

Purpose: validate serialized output against consumer expectations.

Approach:

1. Build `PublisherEvent` from representative `RaceEvent` variants.
2. Validate required keys/types per event type.
3. Validate optional keys and nullability policy.

Example required checks:

1. `LAP_COMPLETED`: `payload.lapTime`, `payload.bestLapTime`, `payload.lap_time_s`, `payload.best_lap_time_s`.
2. `BATTLE_ENGAGED/BROKEN/CLOSING`: `payload.leaderCarNumber`, `payload.followerCarNumber`.
3. All events: envelope identity/timing/context fields are present and typed.

### Tier C: Replay integration tests (new and expanded)

Purpose: catch cross-module failures that unit tests miss.

Approach:

1. Run deterministic replay over JSONL fixture streams.
2. Collect emitted envelopes into an output artifact.
3. Assert stream-level invariants and event-type quality gates.

Checks:

1. No negative lap times.
2. No non-finite numeric payload values.
3. Battle events always include attribution fields.
4. Session transitions do not emit stale `raceSessionId`/`subSessionId` mappings.

### Tier D: Golden snapshot tests (new)

Purpose: detect accidental schema changes and payload drift.

Approach:

1. Commit canonical output snapshots for curated fixtures.
2. Diff output in CI with stable ordering and normalized volatile fields.

Normalization policy:

1. Ignore UUID and wall-clock timestamp fields.
2. Compare schema and semantic payload values.

### Tier E: Live smoke tests on Windows (existing + expanded)

Purpose: verify runtime behavior in real iRacing mmap conditions.

Enhancements:

1. Add explicit acceptance checklist for contract-sensitive fields.
2. Require one dual-publisher run before release cut.
3. Archive race event export artifact for post-run validation scripts.

---

## 6. Test Requirements by Event Category

### Lifecycle and session events

1. Verify session identity continuity and reset semantics.
2. Verify no stale events after transition/reconnect.

### Lap events

1. `lapTime` and `lap_time_s` are consistent aliases.
2. Values must be positive finite or null by policy.
3. `bestLapTime` monotonic non-increasing once established.

### Battle events

1. Include subject opponent indices as produced by engine.
2. Include explicit attribution fields `leaderCarNumber` and `followerCarNumber`.
3. Verify attribution fallback behavior when roster data is missing.

### Envelope-level fields

1. Required identity fields present (`id`, `raceSessionId`, `rigId`, `type`).
2. Required timing/context fields present and typed.
3. `payload` never includes un-serializable values.

---

## 7. Hidden Issues This Plan Is Designed to Prevent

1. Field-name mismatch regressions between producer and consumer contracts.
2. Silent numeric corruption (negative durations, NaN, infinities).
3. Partial roster resolution causing un-attributed events.
4. Role inversion (leader/follower swap) due to malformed position inputs.
5. Session crossover contamination during reconnect or session advancement.
6. Drift between replay and live behavior due to mode-specific telemetry quirks.
7. Unintended payload shape changes during refactors.
8. Non-deterministic event ordering in batched outputs.

---

## 8. Implementation Plan

## Phase 0: Baseline and tooling (1-2 days)

1. Add a reusable test helper for serializing `PublisherEvent` to normalized JSON.
2. Add helper assertions for required keys, numeric validity, and alias consistency.
3. Add fixture utilities for deterministic replay output capture.

Deliverables:

1. `tests/contract_helpers.rs` (or module equivalent).
2. CI job lane for contract/replay tests.

## Phase 1: Contract hardening (2-3 days)

1. Create event contract test suite covering all published event types.
2. Encode required/optional field policy as test data tables.
3. Add explicit checks for lap and battle fields that caused recent incidents.

Exit criteria:

1. Contract suite fails on missing/renamed required fields.
2. Contract suite passes on current expected schema.

## Phase 2: Replay invariants (2-4 days)

1. Add replay test that exports envelopes from synthetic fixtures.
2. Add stream invariants: numeric sanity, attribution presence, ordering checks.
3. Add at least one dual-publisher style fixture with overlapping events.

Exit criteria:

1. Replay invariants pass in CI.
2. Any violation produces actionable assertion output.

## Phase 3: Snapshot and drift protection (2-3 days)

1. Add golden snapshot comparisons for curated fixture outputs.
2. Normalize volatile fields before diff.
3. Require review on snapshot updates.

Exit criteria:

1. Snapshot drift is visible and intentional.
2. Unreviewed schema drift is blocked.

## Phase 4: Live validation and release gating (ongoing)

1. Extend Windows live test checklist with contract validation script.
2. Run dual-publisher smoke before release branch cut.
3. Archive export artifacts and test summary.

Exit criteria:

1. Release checklist includes contract and replay pass evidence.
2. No critical contract/invariant regressions in final sign-off.

---

## 9. CI/CD Integration

Required PR checks:

1. Unit tests.
2. Event contract tests.
3. Replay invariant tests.
4. Snapshot diff check.

Release checks:

1. Windows live smoke checklist completed.
2. Dual-publisher artifact validated by script.

Suggested workflow split:

1. Fast lane on every PR: unit + contract.
2. Full lane on merge/release: replay + snapshot + optional Windows smoke.

---

## 10. Metrics and Quality Gates

Primary metrics:

1. Contract regression escape rate: target 0 per release.
2. Replay invariant violations in CI: target 0 on main.
3. Snapshot drift without approved review: target 0.
4. Time-to-detect contract drift: target < 1 PR cycle.

Quality gate examples:

1. Fail build if any required event field is missing.
2. Fail build if any lap time is negative or non-finite.
3. Fail build if any battle payload misses leader/follower car numbers.

---

## 11. Ownership and Responsibilities

1. Engine owner: lap-timing invariants and event semantic correctness.
2. Publisher owner: envelope schema and consumer compatibility.
3. QA/release owner: replay artifact review and live smoke verification.

---

## 12. Rollout Checklist

1. Create contract helper module.
2. Add event type matrix tests.
3. Add replay invariant assertions.
4. Add snapshot baseline fixtures.
5. Wire CI required checks.
6. Update contributor docs with test execution commands.
7. Enforce release checklist usage.

---

## 13. Immediate Next Actions

1. Implement a dedicated contract test file focused on `PublisherEvent` serialization outputs.
2. Add dual-publisher replay fixture assertions for lap-time and battle attribution fields.
3. Add a small validation script to scan exported `raceEvents.json` artifacts for contract violations.

This plan is intentionally implementation-oriented so each phase can be delivered incrementally while improving protection against schema and data quality regressions.
