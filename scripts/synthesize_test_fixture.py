"""
synthesize_test_fixture.py
==========================
Generates a synthetic JSONL stream designed to exercise every state of the
narrative engine — TRACKING → PUSH → ATTACK_SETUP → overtake — in a
controlled, repeatable way.

The real race data never triggers PUSH/ATTACK because Paul gained positions
too quickly for any single opponent to appear in 3+ consecutive laps.  This
fixture creates a fictional 10-lap race where Paul starts P15 and slowly
hunts P10 over several laps before finally making the pass.

Output: data/test_fixture.jsonl

Usage:
    python3 scripts/synthesize_test_fixture.py
    python3 scripts/prototype_narrative.py --fixture data/test_fixture.jsonl
"""

import json, math, random, sys

random.seed(42)

# ── Scenario parameters ──────────────────────────────────────────────────────
N_CARS           = 32
LAP_TIME_S       = 140.0        # short-track lap (~2.3 min, fast feedback)
N_LAPS           = 10
SAMPLE_RATE_HZ   = 5
DT               = 1.0 / SAMPLE_RATE_HZ
TRACK_LENGTH_M   = 3_850        # metres (fictional)

PLAYER_IDX       = 18
PLAYER_START_POS = 15           # P15 at the start
TARGET_IDX       = 7            # the car Paul is hunting
TARGET_START_POS = 10           # P10

# Gap scenario: gap to TARGET_IDX per lap (in seconds)
# The key for ATTACK_SETUP: the OLS slope must become MORE NEGATIVE lap-over-lap.
# That requires each successive lap showing a LARGER absolute decrease in gap.
# Pattern: slow start → accelerating close → overtake
#
# After lap 3: OLS(laps 2-3) slope ≈ -0.50s/lap  → PUSH fires
# After lap 4: OLS(laps 2-4) slope ≈ -0.60s/lap  → ATTACK_SETUP (accelerating)
# After lap 5: OLS(laps 2-5) slope ≈ -0.76s/lap  → stays ATTACK_SETUP
# Lap 6-7: gap < 1.5s → CLOSE_APPROACH fires
# Lap 8: gap < 0 → Paul overtakes
GAP_TO_TARGET_PER_LAP = {
    1:  6.0,   # outside 5s battle window
    2:  4.5,   # enters window (1st reading per bucket)
    3:  4.0,   # -0.5s: small close  → PUSH fires after this lap
    4:  3.3,   # -0.7s: slightly faster → ATTACK_SETUP fires after this lap
    5:  2.2,   # -1.1s: accelerating
    6:  0.6,   # -1.6s: very close — CLOSE_APPROACH fires here
    7:  0.1,   # -0.5s: on bumper
    8: -1.5,   # Paul passes → OVERTAKE detected at lap crossing
    9: -3.0,
   10: -4.5,
}

# How far (in ldp) does the target car advance per lap compared to Paul?
# Positive = target pulls away, negative = Paul catches up
def target_ldp_offset(lap):
    """LapDistPct offset of target car relative to player at lap start."""
    gap = GAP_TO_TARGET_PER_LAP.get(lap, -2.0)
    if gap < 0:
        # Paul is ahead; target is BEHIND in race order
        return -abs(gap) / LAP_TIME_S
    return gap / LAP_TIME_S

# ── Build a full-field positions array ───────────────────────────────────────
def build_positions(lap):
    """Race positions for each car at this lap. Player and target are fixed;
    others are shuffled around them."""
    player_pos = PLAYER_START_POS - max(0, lap - 2)  # gains ~1 pos per lap
    player_pos = max(1, player_pos)

    gap = GAP_TO_TARGET_PER_LAP.get(lap, -2.0)
    if gap < 0:
        # Paul passed the target
        target_pos = player_pos + 1
        player_pos = max(1, player_pos)
    else:
        target_pos = player_pos - 1    # target stays 1 ahead until passed
        target_pos = max(1, target_pos)

    positions = [0] * N_CARS
    positions[PLAYER_IDX]  = player_pos
    positions[TARGET_IDX]  = target_pos

    # Fill other cars into remaining positions
    used = {player_pos, target_pos}
    other_cars = [i for i in range(N_CARS) if i not in (PLAYER_IDX, TARGET_IDX)]
    remaining  = [p for p in range(1, N_CARS + 1) if p not in used]
    for i, car in enumerate(other_cars):
        positions[car] = remaining[i] if i < len(remaining) else 0

    return positions

# ── Frame generator ───────────────────────────────────────────────────────────
def make_frame(lap, t, ldp, positions, car_ldps, on_pit, flags):
    """Build one JSONL frame matching the prototype's expected schema."""
    return {
        "session_time":           round(t, 3),
        "lap":                    lap,
        "lap_dist_pct":           round(ldp, 5),
        "lap_last_lap_time":      0.0,   # not used; LapTimer derives from session_time
        "player_car_idx":         PLAYER_IDX,
        "player_car_position":    positions[PLAYER_IDX],
        "on_pit_road":            on_pit,
        "session_flags":          flags,
        "car_idx_lap_dist_pct":   [round(v, 5) for v in car_ldps],
        "car_idx_position":       positions[:],
        "car_idx_on_pit_road":    [False] * N_CARS,
        "car_idx_f2_time":        [0.0] * N_CARS,   # stale in replay — not used
    }


# ── Synthesise ────────────────────────────────────────────────────────────────
frames    = []
session_t = 60.0   # start 1 minute into session (formation lap done)

print("Generating synthetic test fixture…")
print(f"  {N_LAPS} laps × {LAP_TIME_S:.0f}s/lap × {SAMPLE_RATE_HZ}Hz = "
      f"{int(N_LAPS * LAP_TIME_S * SAMPLE_RATE_HZ)} frames expected")
print(f"  Player CarIdx {PLAYER_IDX}  start P{PLAYER_START_POS}")
print(f"  Target CarIdx {TARGET_IDX}  start P{TARGET_START_POS}")
print()
print(f"  {'Lap':>4}  {'Gap to target':>14}  {'Player pos':>11}  {'State hint':>20}")
print("  " + "─" * 56)

for lap in range(1, N_LAPS + 1):
    lap_start_t = session_t
    frames_in_lap = int(LAP_TIME_S * SAMPLE_RATE_HZ)
    positions     = build_positions(lap)
    player_pos    = positions[PLAYER_IDX]
    target_ldp_off = target_ldp_offset(lap)
    gap_s          = GAP_TO_TARGET_PER_LAP.get(lap, -4.0)

    state_hint = ("ahead of target" if gap_s < 0
                  else "out of window" if gap_s > 5
                  else f"closing {gap_s:.1f}s gap")
    print(f"  {lap:>4}  {gap_s:>+13.1f}s  P{player_pos:>9d}  {state_hint:>20}")

    for frame_i in range(frames_in_lap):
        t   = session_t + frame_i * DT
        ldp = (frame_i / frames_in_lap)    # player advances 0→1 uniformly

        # Build per-car LapDistPct arrays
        car_ldps = [-1.0] * N_CARS
        for car_i in range(N_CARS):
            if positions[car_i] <= 0:
                continue
            if car_i == PLAYER_IDX:
                car_ldps[car_i] = ldp
                continue
            # Cars ahead in race are also ahead in ldp (same lap assumed)
            pos_offset = (positions[PLAYER_IDX] - positions[car_i]) / N_CARS
            car_ldp    = ldp + pos_offset + random.gauss(0, 0.001)
            # Target car uses the scenario-defined gap
            if car_i == TARGET_IDX:
                car_ldp = ldp + target_ldp_off
            # Clamp to [0, 1)
            car_ldp = car_ldp % 1.0
            car_ldps[car_i] = car_ldp

        # Inject a yellow flag in lap 3 between 40-45% of the lap
        flags = 0
        if lap == 3 and 0.40 <= ldp <= 0.45:
            flags = 0x100   # YELLOW_WAVE

        # Pit stop in lap 9 (brief, ~10 frames)
        on_pit = (lap == 9 and 0.02 <= ldp <= 0.05)

        frames.append(make_frame(lap, t, ldp, positions, car_ldps, on_pit, flags))

    session_t += LAP_TIME_S

print()

out_path = "data/test_fixture.jsonl"
with open(out_path, "w") as fh:
    for f in frames:
        fh.write(json.dumps(f) + "\n")

print(f"Written {len(frames)} frames → {out_path}")
print()
print("Run prototype against it:")
print(f"  JSONL_PATH=data/test_fixture.jsonl python3 scripts/prototype_narrative.py")
print()
print("Expected events:")
print("  Lap 2: (first anchor readings recorded, no regression yet)")
print("  Lap 3: PUSH fires (OLS slope ≈ -0.50s/lap over laps 2-3)")
print("  Lap 4: ATTACK_SETUP fires (slope ≈ -0.60s/lap, more negative than prev)")
print("  Laps 5-6: state stays ATTACK_SETUP (slope accelerates to ≈-0.76, ≈-0.88)")
print("  Lap 6: CLOSE_APPROACH (gap ≈ 0.6s < 1.5s threshold)")
print("  Lap 7: CLOSE_APPROACH (gap ≈ 0.1s)")
print("  Lap 8: OVERTAKE +1 (Paul passes target at lap crossing)")
