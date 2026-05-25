"""
prototype_narrative.py
======================
Runs the full two-tier spatial-anchor narrative pipeline against the real
Nürburgring JSONL telemetry (exported from the iRacing replay via
export_replay.py) and prints a human-readable race timeline.

This is a direct translation of the architecture spec into runnable Python,
so you can validate it against what you know really happened in the race
before committing to Rust.

Pipeline (mirrors spec exactly):
  §3   — Dynamic anchor count from last completed lap time
  §4   — Full-field CarIdx arrays → car_ahead_idx + gap_seconds per frame
  §5.1 — Ring buffer per (anchor_bucket, car_ahead_idx)
  §5.2 — Per-anchor OLS regression: x=lap, y=gap_seconds at that anchor
  §5.5 — Two-tier: per-anchor slope (WHERE) → median slope (WHETHER)
  §6   — SessionFlags bitmask for yellow-contaminated row filtering
  §7   — BattleState machine drives lap-crossing transitions

Improvements over the CSV-based prototype:
  - Opponent identity from CarIdxF2Time (no more false regression slopes
    caused by the car-ahead changing between laps)
  - Regression keyed on (anchor_bucket, car_ahead_idx) — each series
    compares like-with-like
  - Gap in seconds (f2-time delta) instead of CarDistAhead / Speed estimate
  - is_on_track=False is a replay artefact; filter on player_car_position > 0

Output:
  Console: per-lap state log + JSON narrative events
  scripts/prototype_narrative.png: 4-panel validation plot
"""

import json
import numpy as np
import pandas as pd
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from enum import Enum, auto

# ── Configuration (mirrors spec constants) ────────────────────────────────────
import os, sys
JSONL_PATH             = os.environ.get('JSONL_PATH', 'data/session.jsonl')
TARGET_CADENCE_S       = 5.0
MIN_PUSH_READINGS      = 2          # laps per (bucket, opponent) to qualify
MIN_ATTACK_READINGS    = 3
PUSH_SLOPE_THRESHOLD   = -0.05      # s/lap — sustained closing
ATTACK_SLOPE_THRESHOLD = -0.10      # s/lap — accelerating closing
MAX_BATTLE_GAP_S       = 5.0        # only track car ahead if within 5 s
SCAN_FIELD_POSITIONS   = 5          # how many cars ahead AND behind to watch
PIT_LAP_FRAME_THRESH   = 20         # frames with on_pit_road==True → pit lap

# SessionFlags bitmasks (spec §6)
YELLOW_WAVE = 0x100
CAUTION     = 0x4000

# ── BattleState (spec §7) ──────────────────────────────────────────────────────
class BattleState(Enum):
    IDLE         = auto()
    TRACKING     = auto()
    PUSH         = auto()
    ATTACK_SETUP = auto()

# ── Data synthesis helpers ────────────────────────────────────────────────────
# Yellow flag zones identified from CSV telemetry (SessionFlags is 0 in replay)
KNOWN_YELLOW_ZONES = [
    (1, 0.625, 0.646),   # Lap 1 yellow at ~63% of lap
    (2, 0.616, 0.623),   # Lap 2 yellow at ~62% of lap
]
NURBURGRING_LAP_EST_S = 540.0  # Fallback if no lap has completed yet


def synthesize_flags(lap, ldp):
    """Return a SessionFlags bitmask from known yellow zones (replay has none)."""
    for (ylap, p0, p1) in KNOWN_YELLOW_ZONES:
        if lap == ylap and p0 <= ldp <= p1:
            return YELLOW_WAVE
    return 0


class LapTimer:
    """Tracks first-frame session_time per lap to compute lap durations live."""

    def __init__(self):
        self._starts = {}   # lap → first session_time seen
        self._times  = {}   # lap → duration (seconds)

    def update(self, lap, t):
        if lap not in self._starts:
            self._starts[lap] = t
            prev = lap - 1
            if prev in self._starts:
                self._times[prev] = t - self._starts[prev]

    def best_estimate(self):
        """Most recently completed lap time, or Nürburgring fallback."""
        return self._times[max(self._times)] if self._times else NURBURGRING_LAP_EST_S

    def completed(self, lap):
        return self._times.get(lap)


def find_cars_ahead_ldp(frame, lap_time_s, n=SCAN_FIELD_POSITIONS):
    """
    Find the N closest cars physically ahead using car_idx_lap_dist_pct.
    Returns a list of (car_idx, gap_seconds) sorted by ascending gap (nearest first).
    Covers slower lapped traffic and cars within striking distance ahead.
    """
    player_idx = frame['player_car_idx']
    player_pos = frame['player_car_position']
    ldp_arr    = frame['car_idx_lap_dist_pct']
    pos_arr    = frame['car_idx_position']
    pit_arr    = frame['car_idx_on_pit_road']

    if player_pos <= 1:
        return []

    player_ldp  = ldp_arr[player_idx]
    candidates  = []
    max_ldp_gap = MAX_BATTLE_GAP_S / lap_time_s

    for i in range(len(ldp_arr)):
        if i == player_idx:
            continue
        p, ldp, pit = pos_arr[i], ldp_arr[i], pit_arr[i]
        if p <= 0 or p >= player_pos:   # not ahead of us in race order
            continue
        if pit or ldp < -0.5:           # on pit road or inactive sentinel
            continue
        diff = ldp - player_ldp
        if diff < -0.5:
            diff += 1.0                 # they've crossed S/F, we haven't yet
        if 0.0 < diff <= max_ldp_gap:
            candidates.append((diff, i))

    candidates.sort()
    return [(idx, diff * lap_time_s) for diff, idx in candidates[:n]]


def find_cars_behind_ldp(frame, lap_time_s, n=SCAN_FIELD_POSITIONS):
    """
    Find the N closest cars physically behind using car_idx_lap_dist_pct.
    Returns a list of (car_idx, gap_seconds) sorted by ascending gap (nearest first).
    Covers faster class cars charging through and cars closing in on us.
    gap_seconds is the gap from that car TO US (positive = they are behind us).
    """
    player_idx = frame['player_car_idx']
    player_pos = frame['player_car_position']
    ldp_arr    = frame['car_idx_lap_dist_pct']
    pos_arr    = frame['car_idx_position']
    pit_arr    = frame['car_idx_on_pit_road']

    if player_pos <= 0:
        return []

    player_ldp  = ldp_arr[player_idx]
    candidates  = []
    max_ldp_gap = MAX_BATTLE_GAP_S / lap_time_s

    for i in range(len(ldp_arr)):
        if i == player_idx:
            continue
        p, ldp, pit = pos_arr[i], ldp_arr[i], pit_arr[i]
        if p <= 0 or p <= player_pos:   # not behind us in race order
            continue
        if pit or ldp < -0.5:           # on pit road or inactive sentinel
            continue
        diff = player_ldp - ldp         # positive = we are ahead of them
        if diff < -0.5:
            diff += 1.0                 # they've crossed S/F, we haven't yet
        if 0.0 < diff <= max_ldp_gap:
            candidates.append((diff, i))

    candidates.sort()
    return [(idx, diff * lap_time_s) for diff, idx in candidates[:n]]


# ── OLS helper (spec §5.2) ────────────────────────────────────────────────────

def ols_slope(laps, gaps):
    """Linear regression slope of gap ~ lap. Returns None if < 2 clean points."""
    n = len(laps)
    if n < 2:
        return None
    l, g  = np.array(laps, dtype=float), np.array(gaps, dtype=float)
    l_bar = l.mean()
    denom = np.sum((l - l_bar) ** 2)
    return float(np.sum((l - l_bar) * (g - g.mean())) / denom) if denom > 0 else None


# ── Anchor sampler (spec §3.1) ────────────────────────────────────────────────

class AnchorSampler:
    """
    Records the FIRST (gap_s, car_ahead_idx) crossing of each anchor bucket
    per lap — this is the spatial anchor sample the spec uses for regression.
    """

    def __init__(self, n_buckets):
        self.n       = n_buckets
        self._seen   = set()    # (lap, bucket, car_idx) — per-car, so multiple cars don't collide
        self.samples = []       # (lap, bucket, gap_s, car_idx, is_clean)

    def update(self, lap, ldp, gap_s, car_idx, is_clean):
        """Feed one frame; returns True if a new anchor sample was captured."""
        if np.isnan(gap_s) or car_idx < 0:
            return False
        bucket = int(ldp * self.n) % self.n
        key    = (lap, bucket, car_idx)   # per-car key: each car tracked independently
        if key in self._seen:
            return False
        self._seen.add(key)
        self.samples.append((lap, bucket, gap_s, car_idx, is_clean))
        return True


# ── Regression store (spec §5.1–5.2) ─────────────────────────────────────────

class RegressionStore:
    """
    Identity-aware OLS regression over laps.
    Each (bucket, car_ahead_idx) pair is a separate series, so a change of
    opponent mid-race doesn't corrupt the slope signal.
    """

    def __init__(self):
        self._data = {}   # (bucket, car_ahead_idx) → [(lap, gap_s), ...]

    def ingest(self, sampler, max_lap=None):
        """Rebuild from sampler.samples (called after each lap boundary).
        max_lap: only include samples from laps <= max_lap to prevent
        the first frame of the new lap contaminating the previous lap's regression.
        """
        self._data.clear()
        for (lap, bucket, gap_s, car_idx, is_clean) in sampler.samples:
            if not is_clean:
                continue
            if max_lap is not None and lap > max_lap:
                continue
            self._data.setdefault((bucket, car_idx), []).append((lap, gap_s))

    def per_bucket_slopes(self, min_readings):
        """
        Returns {bucket: slope} — the most-negative qualifying slope per bucket
        (if multiple opponents compete at the same bucket, take the closing one).
        """
        bucket_slopes = {}
        for (bucket, _), points in self._data.items():
            if len(points) < min_readings:
                continue
            laps, gaps = zip(*points)
            s = ols_slope(list(laps), list(gaps))
            if s is not None:
                bucket_slopes.setdefault(bucket, []).append(s)
        return {b: min(slopes) for b, slopes in bucket_slopes.items()}

    def per_car_median_slopes(self, min_readings):
        """
        Per-car two-tier analysis: for each tracked opponent compute the median
        of its per-bucket OLS slopes.  Returns:
          {car_idx: {'median': float, 'n_buckets': int, 'n_agree': int}}
        Only cars with >= min_readings qualifying buckets are included.
        Use this instead of the cross-car median when multiple opponents are
        tracked simultaneously — avoids dilution from cars that are not closing.
        """
        car_bucket_slopes = {}   # car_idx → [slope at each qualifying bucket]
        for (bucket, car_idx), points in self._data.items():
            if len(points) < min_readings:
                continue
            laps, gaps = zip(*points)
            s = ols_slope(list(laps), list(gaps))
            if s is not None:
                car_bucket_slopes.setdefault(car_idx, []).append(s)
        result = {}
        for car_idx, bslopes in car_bucket_slopes.items():
            if len(bslopes) >= min_readings:
                result[car_idx] = {
                    'median':    float(np.median(bslopes)),
                    'n_buckets': len(bslopes),
                    'n_agree':   sum(1 for s in bslopes if s < 0),
                }
        return result


# ── Helpers ───────────────────────────────────────────────────────────────────

def _fmt_t(t):
    """Format session_time seconds as mm:ss string."""
    m, s = divmod(int(t), 60)
    return f"{m:02d}:{s:02d}"


def _make_event(event_type, lap, session_time, **ctx):
    return {
        'event_type':        event_type,
        'lap':               lap,
        'session_time':      round(float(session_time), 1),
        'narrative_context': ctx,
    }


# ── Pre-pass: determine anchor count from Lap 1 completed time ────────────────

print("Loading JSONL stream…")
with open(JSONL_PATH) as _fh:
    raw_frames = [json.loads(line) for line in _fh]
print(f"  {len(raw_frames)} frames loaded from {JSONL_PATH}")

_pre_timer = LapTimer()
for _f in raw_frames:
    _pre_timer.update(_f['lap'], _f['session_time'])

lap1_time_s  = _pre_timer.completed(1) or NURBURGRING_LAP_EST_S
anchor_count = max(10, int(lap1_time_s / TARGET_CADENCE_S))
print(f"  Lap 1 time: {lap1_time_s:.1f}s  →  {anchor_count} spatial anchors "
      f"@ {TARGET_CADENCE_S}s cadence")
print(f"  Synthesizing gap from car_idx_lap_dist_pct "
      f"(replay-mode f2_time has only 2 unique values per lap — stale)")
print(f"  Injecting yellow flags from known zones: {KNOWN_YELLOW_ZONES}")
print()

# ── Engine state ──────────────────────────────────────────────────────────────

lap_timer_live   = LapTimer()
sampler          = AnchorSampler(anchor_count)
regression       = RegressionStore()
engine_state     = BattleState.IDLE
prev_slope       = None
# ── Defensive (car-behind) state ─────────────────────────────────────────────
sampler_behind   = AnchorSampler(anchor_count)
regression_behind= RegressionStore()
defensive_state  = BattleState.IDLE
prev_slope_beh   = None
pit_laps         = set()
all_events       = []

# Frame-level streaming state
prev_lap          = None
prev_on_pit       = False
prev_position     = None
lap_pit_frames    = {}     # lap → frame count with on_pit_road == True

CLOSE_APPROACH_THRESH_S    = 1.5   # within 1.5 s → potential overtake window
CLOSE_APPROACH_MIN_FRAMES  = 5     # must persist for ≥ 5 frames before firing

consecutive_close     = 0
last_close_t          = -999.0
tracking_car          = -1    # CarIdx of car currently in close-approach sequence
consecutive_close_beh = 0
last_close_beh_t      = -999.0
tracking_car_beh      = -1    # CarIdx of car in close-pressure-behind sequence

# Logging for visualisation
gap_log           = []    # (session_time, gap_s, car_ahead_idx, lap)
heatmap_data      = {}    # lap → {bucket: slope}  after lap completion
median_slopes_log = {}    # lap → median slope
lap_end_positions = {}    # lap → final position recorded

# ── Main streaming loop ───────────────────────────────────────────────────────

print("═" * 78)
print("  STREAMING NARRATIVE ENGINE  —  processing frame-by-frame")
print("═" * 78)

for frame in raw_frames:
    lap    = frame['lap']
    t      = frame['session_time']
    pos    = frame['player_car_position']
    on_pit = frame['on_pit_road']
    ldp    = frame['lap_dist_pct']

    if pos <= 0 or lap < 1:
        prev_lap    = lap
        prev_on_pit = on_pit
        continue

    lap_timer_live.update(lap, t)
    lap_t = lap_timer_live.best_estimate()

    # ── Synthesize missing telemetry ──────────────────────────────────────────
    synth_flags = synthesize_flags(lap, ldp)
    is_clean    = (synth_flags & (YELLOW_WAVE | CAUTION)) == 0 and not on_pit

    if on_pit:
        lap_pit_frames[lap] = lap_pit_frames.get(lap, 0) + 1

    # ── Gap calculation: scan SCAN_FIELD_POSITIONS cars ahead and behind ────────
    cars_ahead  = find_cars_ahead_ldp(frame, lap_t)
    cars_behind = find_cars_behind_ldp(frame, lap_t)

    for (ci, gs) in cars_ahead:
        sampler.update(lap, ldp, gs, ci, is_clean)
    for (ci, gs) in cars_behind:
        sampler_behind.update(lap, ldp, gs, ci, is_clean)

    # Nearest car drives frame-level events; regression sees all N
    car_ahead_idx,  gap_s     = cars_ahead[0]  if cars_ahead  else (-1, np.nan)
    car_behind_idx, gap_beh_s = cars_behind[0] if cars_behind else (-1, np.nan)

    if not np.isnan(gap_s):
        gap_log.append((t, gap_s, car_ahead_idx, lap))

    # ── Frame-level events ────────────────────────────────────────────────────

    # Pit entry / exit transitions
    if on_pit and not prev_on_pit:
        evt = _make_event('PIT_ENTRY', lap, t,
                          position=int(pos),
                          ai_prompt_hint=f"Enters pit lane at P{int(pos)}")
        all_events.append(evt)
        print(f"\n  [{_fmt_t(t)}  L{lap:02d}]  PIT_ENTRY   P{int(pos)}")

    elif not on_pit and prev_on_pit:
        evt = _make_event('PIT_EXIT', lap, t,
                          position=int(pos),
                          ai_prompt_hint=f"Rejoins from pit at P{int(pos)}")
        all_events.append(evt)
        print(f"  [{_fmt_t(t)}  L{lap:02d}]  PIT_EXIT    P{int(pos)}")

    # Close approach: < CLOSE_APPROACH_THRESH_S for ≥ CLOSE_APPROACH_MIN_FRAMES
    if not np.isnan(gap_s) and gap_s < CLOSE_APPROACH_THRESH_S:
        consecutive_close += 1
        if (consecutive_close >= CLOSE_APPROACH_MIN_FRAMES
                and (t - last_close_t) > 30.0
                and car_ahead_idx != tracking_car):
            tracking_car = car_ahead_idx
            last_close_t = t
            car_pos_val  = frame['car_idx_position'][car_ahead_idx]
            evt = _make_event('CLOSE_APPROACH', lap, t,
                              car_ahead_idx=int(car_ahead_idx),
                              gap_s=round(float(gap_s), 2),
                              car_race_position=int(car_pos_val),
                              ai_prompt_hint=(
                                  f"Hunting CarIdx {car_ahead_idx} (P{int(car_pos_val)})"
                                  f" — gap only {gap_s:.2f}s"))
            all_events.append(evt)
            print(f"  [{_fmt_t(t)}  L{lap:02d}]  CLOSE_APPROACH  "
                  f"CarIdx {car_ahead_idx} (P{int(car_pos_val)}) @ {gap_s:.2f}s")
    else:
        consecutive_close = 0
        if car_ahead_idx != tracking_car:
            tracking_car = -1

    # Pressure from behind: car behind within threshold for ≥ MIN_FRAMES
    if not np.isnan(gap_beh_s) and gap_beh_s < CLOSE_APPROACH_THRESH_S:
        consecutive_close_beh += 1
        if (consecutive_close_beh >= CLOSE_APPROACH_MIN_FRAMES
                and (t - last_close_beh_t) > 30.0
                and car_behind_idx != tracking_car_beh):
            tracking_car_beh = car_behind_idx
            last_close_beh_t = t
            car_pos_val_beh  = frame['car_idx_position'][car_behind_idx]
            evt = _make_event('PRESSURE_BEHIND', lap, t,
                              car_behind_idx=int(car_behind_idx),
                              gap_s=round(float(gap_beh_s), 2),
                              car_race_position=int(car_pos_val_beh),
                              ai_prompt_hint=(
                                  f"CarIdx {car_behind_idx} (P{int(car_pos_val_beh)})"
                                  f" is only {gap_beh_s:.2f}s behind"))
            all_events.append(evt)
            print(f"  [{_fmt_t(t)}  L{lap:02d}]  PRESSURE_BEHIND  "
                  f"CarIdx {car_behind_idx} (P{int(car_pos_val_beh)}) @ {gap_beh_s:.2f}s")
    else:
        consecutive_close_beh = 0
        if car_behind_idx != tracking_car_beh:
            tracking_car_beh = -1

    # ── Lap-crossing: regression + state classification ────────────────────────
    if prev_lap is not None and lap != prev_lap:
        done_lap   = prev_lap
        pit_frames = lap_pit_frames.get(done_lap, 0)
        if pit_frames >= PIT_LAP_FRAME_THRESH:
            pit_laps.add(done_lap)

        lap_t_done = lap_timer_live.completed(done_lap)
        lap_end_positions[done_lap] = int(prev_position or pos)

        print(f"\n  ──── LAP {done_lap} COMPLETE ────")

        prev_lap_pos = lap_end_positions.get(done_lap - 1, int(pos))
        pos_change   = prev_lap_pos - lap_end_positions[done_lap]
        lap_t_str    = f"{lap_t_done:.1f}s" if lap_t_done else "?s"

        evt = _make_event('LAP_COMPLETE', done_lap, t,
                          lap_time_s=round(float(lap_t_done), 1) if lap_t_done else None,
                          position=lap_end_positions[done_lap],
                          pit_frames=pit_frames,
                          ai_prompt_hint=(
                              f"Lap {done_lap} complete in {lap_t_str} "
                              f"at P{lap_end_positions[done_lap]}"
                              + ("  [pit stop]" if done_lap in pit_laps else "")))
        all_events.append(evt)
        print(f"  [{_fmt_t(t)}  L{done_lap:02d}]  LAP_COMPLETE  "
              f"time={lap_t_str}  "
              f"P{prev_lap_pos}→P{lap_end_positions[done_lap]}"
              + ("  [PIT]" if done_lap in pit_laps else ""))

        if pos_change > 0 and done_lap not in pit_laps:
            evt = _make_event('OVERTAKE', done_lap, t,
                              position_from=prev_lap_pos,
                              position_to=lap_end_positions[done_lap],
                              positions_gained=pos_change,
                              ai_prompt_hint=(
                                  f"P{prev_lap_pos}→P{lap_end_positions[done_lap]}, "
                                  f"+{pos_change} position{'s' if pos_change > 1 else ''}"))
            all_events.append(evt)
            print(f"  [{_fmt_t(t)}  L{done_lap:02d}]  OVERTAKE    "
                  f"P{prev_lap_pos}→P{lap_end_positions[done_lap]}  (+{pos_change})")

        elif pos_change < 0:
            evt = _make_event('POSITION_LOST', done_lap, t,
                              position_from=prev_lap_pos,
                              position_to=lap_end_positions[done_lap],
                              positions_lost=-pos_change)
            all_events.append(evt)
            print(f"  [{_fmt_t(t)}  L{done_lap:02d}]  POSITION_LOST  "
                  f"P{prev_lap_pos}→P{lap_end_positions[done_lap]}")

        # Rebuild regression from all clean samples so far (spec §5.1)
        # Pass max_lap=done_lap so the first frame of the new lap
        # (already in the sampler) doesn't contaminate this regression.
        regression.ingest(sampler, max_lap=done_lap)
        per_anchor   = regression.per_bucket_slopes(min_readings=MIN_PUSH_READINGS)   # for heatmap
        car_medians  = regression.per_car_median_slopes(min_readings=MIN_PUSH_READINGS)
        new_state    = BattleState.IDLE
        slope_info   = {}

        if done_lap not in pit_laps and car_medians:
            # Most threatening car: lowest (most negative) per-car median slope
            threat_car  = min(car_medians, key=lambda c: car_medians[c]['median'])
            tinfo       = car_medians[threat_car]
            med_slope   = tinfo['median']
            hot_bucket  = min(per_anchor, key=per_anchor.get) if per_anchor else 0
            hotspot_pct = hot_bucket / anchor_count if per_anchor else 0.0
            slope_info  = {
                'car_ahead_idx':        int(threat_car),
                'median_slope':         round(med_slope, 4),
                'anchors_qualifying':   tinfo['n_buckets'],
                'anchors_agreeing':     tinfo['n_agree'],
                'hotspot_lap_dist_pct': round(hotspot_pct, 3),
            }
            if (med_slope <= ATTACK_SLOPE_THRESHOLD
                    and tinfo['n_buckets'] >= MIN_ATTACK_READINGS
                    and prev_slope is not None and med_slope < prev_slope):
                new_state = BattleState.ATTACK_SETUP
            elif med_slope <= PUSH_SLOPE_THRESHOLD and tinfo['n_buckets'] >= MIN_PUSH_READINGS:
                new_state = BattleState.PUSH
            elif car_medians:
                new_state = BattleState.TRACKING

        heatmap_data[done_lap]      = per_anchor
        median_slopes_log[done_lap] = slope_info.get('median_slope', np.nan)

        # State-transition events
        if new_state != engine_state:
            if new_state in (BattleState.PUSH, BattleState.ATTACK_SETUP):
                hint = (
                    f"CarIdx {slope_info['car_ahead_idx']} — "
                    f"closing at {abs(slope_info['median_slope']):.3f}s/lap "
                    f"({slope_info['anchors_agreeing']}/{slope_info['anchors_qualifying']} "
                    f"anchors agree), hotspot @ {slope_info['hotspot_lap_dist_pct']:.0%}"
                ) if slope_info else ""
                evt = _make_event(new_state.name, done_lap, t,
                                  **slope_info, ai_prompt_hint=hint)
                all_events.append(evt)
                print(f"  [{_fmt_t(t)}  L{done_lap:02d}]  {new_state.name}  "
                      f"CarIdx {slope_info.get('car_ahead_idx', '?')}  "
                      f"slope={slope_info.get('median_slope', 0):+.4f}s/lap  "
                      f"n={slope_info.get('anchors_qualifying', 0)}  "
                      f"hotspot@{slope_info.get('hotspot_lap_dist_pct', 0):.0%}")

        if car_medians:
            s   = slope_info.get('median_slope', np.nan)
            n_q = slope_info.get('anchors_qualifying', 0)
            n_a = slope_info.get('anchors_agreeing', 0)
            ci  = slope_info.get('car_ahead_idx', '?')
            print(f"       regression: CarIdx {ci}  slope={s:+.4f}s/lap  "
                  f"n={n_q}  agree={n_a}/{n_q}  → {new_state.name}")
        else:
            print(f"       regression: n/a  → {new_state.name}")

        engine_state = new_state
        if slope_info:
            prev_slope = slope_info['median_slope']

        # ── Defensive regression: car behind closing on us ────────────────────
        regression_behind.ingest(sampler_behind, max_lap=done_lap)
        per_anchor_beh   = regression_behind.per_bucket_slopes(min_readings=MIN_PUSH_READINGS)
        car_medians_beh  = regression_behind.per_car_median_slopes(min_readings=MIN_PUSH_READINGS)
        new_def_state    = BattleState.IDLE
        slope_beh_info   = {}

        if done_lap not in pit_laps and car_medians_beh:
            # Most threatening car: most negative per-car median slope (closing on us)
            threat_beh   = min(car_medians_beh, key=lambda c: car_medians_beh[c]['median'])
            tinfo_beh    = car_medians_beh[threat_beh]
            med_beh      = tinfo_beh['median']
            hot_beh      = min(per_anchor_beh, key=per_anchor_beh.get) if per_anchor_beh else 0
            hotspot_beh  = hot_beh / anchor_count if per_anchor_beh else 0.0
            slope_beh_info = {
                'car_behind_idx':       int(threat_beh),
                'median_slope':         round(med_beh, 4),
                'anchors_qualifying':   tinfo_beh['n_buckets'],
                'anchors_agreeing':     tinfo_beh['n_agree'],
                'hotspot_lap_dist_pct': round(hotspot_beh, 3),
            }
            # Negative slope = gap-behind shrinking = car behind closing on us
            if (med_beh <= ATTACK_SLOPE_THRESHOLD
                    and tinfo_beh['n_buckets'] >= MIN_ATTACK_READINGS
                    and prev_slope_beh is not None and med_beh < prev_slope_beh):
                new_def_state = BattleState.ATTACK_SETUP
            elif med_beh <= PUSH_SLOPE_THRESHOLD and tinfo_beh['n_buckets'] >= MIN_PUSH_READINGS:
                new_def_state = BattleState.PUSH
            elif car_medians_beh:
                new_def_state = BattleState.TRACKING

        if new_def_state != defensive_state:
            if new_def_state in (BattleState.PUSH, BattleState.ATTACK_SETUP):
                hint_beh = (
                    f"CarIdx {slope_beh_info['car_behind_idx']} closing from behind at "
                    f"{abs(slope_beh_info['median_slope']):.3f}s/lap "
                    f"({slope_beh_info['anchors_agreeing']}/{slope_beh_info['anchors_qualifying']} "
                    f"anchors agree), hotspot @ {slope_beh_info['hotspot_lap_dist_pct']:.0%}"
                ) if slope_beh_info else ""
                label = ('DEFEND_PUSH' if new_def_state == BattleState.PUSH
                         else 'DEFEND_ATTACK')
                evt = _make_event(label, done_lap, t,
                                  **slope_beh_info, ai_prompt_hint=hint_beh)
                all_events.append(evt)
                print(f"  [{_fmt_t(t)}  L{done_lap:02d}]  {label}  "
                      f"CarIdx {slope_beh_info.get('car_behind_idx', '?')}  "
                      f"slope={slope_beh_info.get('median_slope', 0):+.4f}s/lap  "
                      f"n={slope_beh_info.get('anchors_qualifying', 0)}  "
                      f"hotspot@{slope_beh_info.get('hotspot_lap_dist_pct', 0):.0%}")

        if car_medians_beh:
            s_b  = slope_beh_info.get('median_slope', np.nan)
            n_b  = slope_beh_info.get('anchors_qualifying', 0)
            n_ab = slope_beh_info.get('anchors_agreeing', 0)
            ci_b = slope_beh_info.get('car_behind_idx', '?')
            print(f"       defensive:  CarIdx {ci_b}  slope={s_b:+.4f}s/lap  "
                  f"n={n_b}  agree={n_ab}/{n_b}  → {new_def_state.name}")
        else:
            print(f"       defensive:  n/a  → {new_def_state.name}")

        defensive_state = new_def_state
        if slope_beh_info:
            prev_slope_beh = slope_beh_info['median_slope']

    prev_lap      = lap
    prev_on_pit   = on_pit
    prev_position = pos

# ── Full event timeline ───────────────────────────────────────────────────────

def _json_default(obj):
    if hasattr(obj, 'item'):
        return obj.item()
    raise TypeError(f'Object of type {type(obj).__name__} is not JSON serialisable')


print("\n" + "═" * 78)
print("  FULL RACE EVENT TIMELINE")
print("═" * 78)
for evt in all_events:
    ctx  = evt['narrative_context']
    hint = ctx.get('ai_prompt_hint', '')
    print(f"\n  [{_fmt_t(evt['session_time'])}  L{evt['lap']:02d}]  {evt['event_type']}")
    for k, v in ctx.items():
        if k != 'ai_prompt_hint' and v is not None:
            print(f"    {k}: {v}")
    if hint:
        print(f"    → \"{hint}\"")

print("\n\nFull event JSON:")
print(json.dumps(all_events, indent=2, default=_json_default))

# ── Visualisation: 4-panel plot ───────────────────────────────────────────────

fig, axes = plt.subplots(4, 1, figsize=(18, 22),
                         gridspec_kw={'height_ratios': [3, 2, 2, 1.5]})
fig.suptitle(
    'Prototype Narrative Engine — Nürburgring Race\n'
    'Streaming simulation: events emitted at the moment the engine detects them',
    fontsize=13, fontweight='bold', y=0.99,
)

OPPONENT_COLOURS = {16: '#e74c3c', 24: '#3498db', 31: '#2ecc71', 15: '#f39c12'}
EVENT_COLOURS    = {
    'OVERTAKE':       '#e74c3c',
    'PUSH':           '#f39c12',
    'ATTACK_SETUP':   '#8e44ad',
    'CLOSE_APPROACH': '#00bcd4',
    'PIT_ENTRY':      '#795548',
    'PIT_EXIT':       '#4caf50',
}

# ─ Panel 1: Gap timeline ──────────────────────────────────────────────────────
ax1 = axes[0]
if gap_log:
    arr        = np.array([(t, g, c, l) for t, g, c, l in gap_log], dtype=float)
    ts, gs, cs = arr[:, 0], arr[:, 1], arr[:, 2]
    for car_idx in sorted(set(int(c) for c in cs)):
        mask   = cs == car_idx
        colour = OPPONENT_COLOURS.get(car_idx, '#999999')
        ax1.scatter(ts[mask], gs[mask], s=1.5, color=colour, alpha=0.6,
                    label=f'CarIdx {car_idx}')

ax1.axhline(MAX_BATTLE_GAP_S, color='grey', linestyle=':', linewidth=1, alpha=0.5)
ax1.axhline(CLOSE_APPROACH_THRESH_S, color='orange', linestyle='--', linewidth=1,
            label=f'Close approach ({CLOSE_APPROACH_THRESH_S}s)')

for (lap_y, p0, p1) in KNOWN_YELLOW_ZONES:
    ys_ts = [f['session_time'] for f in raw_frames
             if f['lap'] == lap_y and p0 <= f['lap_dist_pct'] <= p1]
    if ys_ts:
        ax1.axvspan(min(ys_ts), max(ys_ts), alpha=0.15, color='yellow',
                    label=f'Synthesized yellow (L{lap_y})')

for evt in all_events:
    col = next((EVENT_COLOURS[k] for k in EVENT_COLOURS if k in evt['event_type']), None)
    if col:
        ax1.axvline(evt['session_time'], color=col, linestyle=':', linewidth=1.5, alpha=0.8)
        ax1.text(evt['session_time'] + 3, MAX_BATTLE_GAP_S * 0.92,
                 evt['event_type'], rotation=90, fontsize=6.5, color=col, va='top')

ax1.set_ylim(0, MAX_BATTLE_GAP_S + 0.5)
ax1.set_ylabel('Gap to car ahead (s)')
ax1.set_title('Real-time gap to nearest car ahead (ldp-based) — events at detection moment')
ax1.legend(loc='upper right', fontsize=7, ncol=3, markerscale=4)

# ─ Panel 2: Per-anchor slope heatmap (WHERE) ──────────────────────────────────
ax2 = axes[1]
lap_list = sorted(heatmap_data.keys())
if lap_list:
    heat = np.full((len(lap_list), anchor_count), np.nan)
    for i, lap in enumerate(lap_list):
        for bucket, slope in heatmap_data[lap].items():
            heat[i, bucket] = slope
    im = ax2.imshow(heat, aspect='auto', cmap='RdYlGn', vmin=-0.25, vmax=0.25,
                    extent=[0, 1, len(lap_list) + 0.5, 0.5])
    ax2.set_yticks(range(1, len(lap_list) + 1))
    ax2.set_yticklabels([f'Lap {l}' for l in lap_list])
    plt.colorbar(im, ax=ax2, label='slope (s/lap)', shrink=0.7)
    for (lap_y, p0, p1) in KNOWN_YELLOW_ZONES:
        if lap_y in lap_list:
            ax2.axvspan(p0, p1, alpha=0.2, color='yellow')

ax2.set_xlabel('Track position (LapDistPct)')
ax2.set_ylabel('After lap')
ax2.set_title('WHERE — per-anchor OLS slope (green=closing, red=opening, grey=no data)')

# ─ Panel 3: Median slope bar chart (WHETHER) ──────────────────────────────────
ax3 = axes[2]
m_laps   = list(median_slopes_log.keys())
m_slopes = [float(median_slopes_log[l])
            if not np.isnan(float(median_slopes_log[l])) else 0.0
            for l in m_laps]
bar_cols = []
for l, s in zip(m_laps, [median_slopes_log[l] for l in m_laps]):
    if l in pit_laps:
        bar_cols.append('#aaaaaa')
    elif np.isnan(float(s)):
        bar_cols.append('#dddddd')
    elif float(s) <= ATTACK_SLOPE_THRESHOLD:
        bar_cols.append('#8e44ad')
    elif float(s) <= PUSH_SLOPE_THRESHOLD:
        bar_cols.append('#f39c12')
    else:
        bar_cols.append('#3498db')

ax3.bar(m_laps, m_slopes, color=bar_cols, alpha=0.85, width=0.6)
ax3.axhline(PUSH_SLOPE_THRESHOLD, color='#f39c12', linestyle='--', linewidth=1.5,
            label=f'PUSH threshold ({PUSH_SLOPE_THRESHOLD} s/lap)')
ax3.axhline(ATTACK_SLOPE_THRESHOLD, color='#8e44ad', linestyle='--', linewidth=1.5,
            label=f'ATTACK threshold ({ATTACK_SLOPE_THRESHOLD} s/lap)')
ax3.axhline(0, color='black', linewidth=0.8)
for l, s in zip(m_laps, [median_slopes_log[l] for l in m_laps]):
    n = len(heatmap_data.get(l, {}))
    if n > 0 and not np.isnan(float(s)):
        ax3.text(l, float(s) - 0.005, f'n={n}', ha='center', va='top', fontsize=8)

ax3.set_xlabel('Lap')
ax3.set_ylabel('Median slope (s/lap)')
ax3.set_title('WHETHER — aggregate classification '
              '(orange=PUSH, purple=ATTACK_SETUP, blue=TRACKING, grey=pit/no data)')
ax3.legend(fontsize=8)
ax3.set_xticks(m_laps)

# ─ Panel 4: Position timeline ─────────────────────────────────────────────────
ax4 = axes[3]
pos_laps = sorted(lap_end_positions.keys())
pos_vals = [lap_end_positions[l] for l in pos_laps]
if pos_laps:
    ax4.step(pos_laps, pos_vals, where='post', color='#2c3e50', linewidth=2,
             marker='o', markersize=7)
    ax4.invert_yaxis()
    ax4.set_xticks(pos_laps)
    for evt in all_events:
        if evt['event_type'] == 'OVERTAKE':
            ctx = evt['narrative_context']
            ax4.annotate(
                f"+{ctx['positions_gained']}",
                xy=(evt['lap'], ctx['position_to']),
                xytext=(evt['lap'] + 0.2, ctx['position_to'] + 1.5),
                fontsize=9, color='#e74c3c',
                arrowprops=dict(arrowstyle='->', color='#e74c3c', lw=1.2),
            )

ax4.set_xlabel('Lap')
ax4.set_ylabel('Position')
ax4.set_title('Race position (snaps at lap crossings — engine limitation in replay mode)')

plt.tight_layout(rect=[0, 0, 1, 0.98])
out_path = 'scripts/prototype_narrative.png'
plt.savefig(out_path, dpi=150, bbox_inches='tight')
print(f"\nSaved plot → {out_path}")
