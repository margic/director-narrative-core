"""
prototype_narrative.py
======================
Runs the full two-tier spatial-anchor narrative pipeline against the real
Nürburgring telemetry CSV and prints a human-readable race timeline.

This is a direct translation of the architecture spec into runnable Python,
so you can validate it against what you know really happened in the race
before committing to Rust.

Pipeline (mirrors spec exactly):
  §3   — Dynamic anchor count from last completed lap time
  §4   — (Simplified) single abstract opponent — no CarIdxF2Time in .ibt
  §5.1 — Ring buffer per (anchor_bucket)
  §5.2 — Per-anchor OLS regression: x=lap, y=gap_seconds at that anchor
  §5.5 — Two-tier: per-anchor slope (WHERE) → median slope (WHETHER)
  §6   — Manual yellow zone injection (SessionFlags not in CSV)
  §7   — BattleState machine drives lap-crossing transitions

Limitations vs production Rust:
  - No opponent identity tracking (CarIdxF2Time unavailable in .ibt)
  - Yellow zones manually annotated (SessionFlags not exported)
  - Gap estimate: CarDistAhead_m / Speed_m_s (approximation)

Output:
  Console: per-lap state log + JSON narrative events
  scripts/prototype_narrative.png: 4-panel validation plot
"""

import numpy as np
import pandas as pd
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import json
from enum import Enum, auto

# ── Configuration (mirrors spec constants) ────────────────────────────────────
DATA_PATH            = 'data/porsche992rgt3_nurburgring combinedlong 2026-05-24 06-54-35.csv'
TARGET_CADENCE_S     = 5.0
SENTINEL_M           = 490_000.0
BATTLE_THRESHOLD_M   = 50.0
MIN_PUSH_READINGS    = 2          # long-track threshold (spec §5.4)
MIN_ATTACK_READINGS  = 3
PUSH_SLOPE_THRESHOLD   = -0.05   # s/lap — sustained closing
ATTACK_SLOPE_THRESHOLD = -0.10   # s/lap — accelerating closing
PIT_LAP_ROW_THRESHOLD  = 500     # rows with OnPitRoad==True → treat lap as pit lap

# SessionFlags bitmasks (spec §6) — now read directly from the exported CSV
YELLOW_WAVE = 0x100
CAUTION     = 0x4000

# ── BattleState (spec §7) ──────────────────────────────────────────────────────
class BattleState(Enum):
    IDLE                       = auto()
    TRACKING                   = auto()
    PUSH                       = auto()
    ATTACK_SETUP               = auto()
    RESET_YELLOW_CONTAMINATION = auto()

# ── Step 1: Load and preprocess ───────────────────────────────────────────────
print("Loading telemetry...")
df = pd.read_csv(DATA_PATH)
df = df[(df['IsOnTrack'] == True) & (df['Lap'] >= 1) & (df['Lap'] <= 5)].copy()

# Sentinel → NaN
df['CarDistAhead'] = df['CarDistAhead'].where(df['CarDistAhead'] < SENTINEL_M, np.nan)

# Gap estimate: distance / own speed (spec §8 — approximation for .ibt prototype)
df['GapSeconds'] = np.where(
    df['Speed'] > 1.0,
    df['CarDistAhead'] / df['Speed'],
    np.nan
)

# Detect pit laps automatically (rows with OnPitRoad==True per lap)
pit_lap_rows = df.groupby('Lap')['OnPitRoad'].sum()
pit_laps = set(pit_lap_rows[pit_lap_rows > PIT_LAP_ROW_THRESHOLD].index)
print(f"Pit laps detected: {sorted(pit_laps)}")

# Tag yellow-contaminated rows using real SessionFlags bitmask (spec §6)
df['is_clean'] = ((df['SessionFlags'].astype(int) & (YELLOW_WAVE | CAUTION)) == 0)
df.loc[df['Lap'].isin(pit_laps), 'is_clean'] = False

# ── Step 2: Dynamic anchor count (spec §3.2) ──────────────────────────────────
# Use LapLastLapTime max for Lap 2 — iRacing updates this field a moment after
# the start/finish crossing, so the very first rows of Lap 2 still carry 0.
lap1_time = df[df['Lap'] == 2]['LapLastLapTime'].max()
anchor_count = max(10, int(lap1_time / TARGET_CADENCE_S))
print(f"Lap 1 time: {lap1_time:.1f}s → {anchor_count} anchors (cadence: {TARGET_CADENCE_S}s)")

# ── Step 3: Spatial anchor sampling (spec §3.1) ───────────────────────────────
# Assign anchor bucket to every row
df['AnchorBucket'] = (df['LapDistPct'] * anchor_count).astype(int).clip(0, anchor_count - 1)

# First crossing of each (Lap, AnchorBucket) pair = the anchor reading
anchor_samples = (
    df.dropna(subset=['GapSeconds'])
      .sort_values('SessionTime')
      .groupby(['Lap', 'AnchorBucket'])
      .first()
      .reset_index()
)[['Lap', 'AnchorBucket', 'GapSeconds', 'is_clean', 'SessionTime', 'LapDistPct']]

print(f"Anchor samples: {len(anchor_samples)} total "
      f"({anchor_samples['is_clean'].sum()} clean, "
      f"{(~anchor_samples['is_clean']).sum()} dirty)")

# ── Step 4: OLS helper (spec §5.2) ────────────────────────────────────────────
def ols_slope(laps, gaps):
    """Linear regression slope of gap ~ lap. Returns None if < 2 points."""
    n = len(laps)
    if n < 2:
        return None
    l, g = np.array(laps, dtype=float), np.array(gaps, dtype=float)
    l_bar, g_bar = l.mean(), g.mean()
    denom = np.sum((l - l_bar) ** 2)
    return float(np.sum((l - l_bar) * (g - g_bar)) / denom) if denom > 0 else None

# ── Step 5: Lap-by-lap streaming simulation ───────────────────────────────────
# At each lap crossing we have all readings up to and including that lap.
# This mirrors the Rust engine receiving ticks and crossing lap boundaries.

all_laps  = sorted(df['Lap'].unique())
lap_ends  = df.groupby('Lap').agg(
    position=('PlayerCarPosition', 'last'),
    session_time_end=('SessionTime', 'max'),
    session_time_start=('SessionTime', 'min'),
).to_dict('index')

print(f"\nLaps in session: {all_laps}")
print("\n" + "─" * 80)
print(f"{'Lap':>4}  {'State':^30}  {'Median slope':>14}  {'Qual.':>6}  {'Agree':>6}  {'Pos':>4}")
print("─" * 80)

events         = []
state          = BattleState.IDLE
prev_slope     = None
prev_position  = lap_ends[all_laps[0]]['position']

# Storage for the plot — per-lap median slope and per-anchor heatmap
median_slopes_log = {}       # lap → median_slope (or NaN)
heatmap_data      = {}       # lap → {bucket → slope}

for current_lap in all_laps:
    if current_lap == all_laps[0]:
        # First lap — no prior data to regress against
        median_slopes_log[current_lap] = np.nan
        heatmap_data[current_lap] = {}
        prev_position = lap_ends[current_lap]['position']
        continue

    # All anchor readings available through the end of this lap
    available = anchor_samples[anchor_samples['Lap'] <= current_lap]

    # ── Tier 1: per-anchor slope (WHERE) ─────────────────────────────────────
    per_anchor_slopes = {}
    for bucket in range(anchor_count):
        rows = available[available['AnchorBucket'] == bucket]
        clean = rows[rows['is_clean'] == True]
        if len(clean) < MIN_PUSH_READINGS:
            continue
        slope = ols_slope(clean['Lap'].tolist(), clean['GapSeconds'].tolist())
        if slope is not None:
            per_anchor_slopes[bucket] = slope

    heatmap_data[current_lap] = per_anchor_slopes

    # ── Tier 2: aggregate classification (WHETHER) ───────────────────────────
    slopes = list(per_anchor_slopes.values())

    if not slopes:
        new_state = BattleState.TRACKING if current_lap not in pit_laps else BattleState.IDLE
        median_slope = np.nan
        anchors_agreeing = 0
        hotspot_pct = None
    else:
        median_slope     = float(np.median(slopes))
        anchors_agreeing = sum(1 for s in slopes if s < 0)
        hotspot_bucket   = min(per_anchor_slopes, key=per_anchor_slopes.get)
        hotspot_pct      = hotspot_bucket / anchor_count

        if current_lap in pit_laps:
            new_state = BattleState.IDLE
        elif (median_slope <= ATTACK_SLOPE_THRESHOLD
              and len(slopes) >= MIN_ATTACK_READINGS
              and prev_slope is not None and median_slope < prev_slope):
            new_state = BattleState.ATTACK_SETUP
        elif median_slope <= PUSH_SLOPE_THRESHOLD and len(slopes) >= MIN_PUSH_READINGS:
            new_state = BattleState.PUSH
        elif len(slopes) >= 1:
            new_state = BattleState.TRACKING
        else:
            new_state = BattleState.IDLE

    median_slopes_log[current_lap] = median_slope

    # ── Overtake detection (spec §2) ─────────────────────────────────────────
    current_position = lap_ends[current_lap]['position']
    t_start          = lap_ends[current_lap]['session_time_start']
    positions_gained = (prev_position - current_position) if prev_position else 0

    if positions_gained > 0:
        events.append({
            'lap': current_lap,
            'session_time': round(t_start, 1),
            'event_type': 'OVERTAKE',
            'narrative_context': {
                'position_from': int(prev_position),
                'position_to':   int(current_position),
                'positions_gained': int(positions_gained),
                'ai_prompt_hint': (
                    f"Driver moved from P{int(prev_position)} to P{int(current_position)}, "
                    f"gaining {positions_gained} position{'s' if positions_gained > 1 else ''}"
                ),
            }
        })

    # ── State-change events ───────────────────────────────────────────────────
    if new_state != state and new_state in (BattleState.PUSH, BattleState.ATTACK_SETUP):
        events.append({
            'lap': current_lap,
            'session_time': round(t_start, 1),
            'event_type': new_state.name,
            'narrative_context': {
                'closing_rate_per_lap_s': round(median_slope, 4),
                'anchors_qualifying':     len(slopes),
                'anchors_agreeing':       anchors_agreeing,
                'hotspot_lap_dist_pct':   round(hotspot_pct, 3) if hotspot_pct is not None else None,
                'ai_prompt_hint': (
                    f"Driver closing at {abs(median_slope):.3f}s/lap "
                    f"({anchors_agreeing}/{len(slopes)} anchors agree), "
                    f"hotspot at {hotspot_pct:.0%} track position"
                ) if hotspot_pct is not None else "",
            }
        })
    elif new_state != state:
        events.append({
            'lap': current_lap,
            'session_time': round(t_start, 1),
            'event_type': f'STATE_{new_state.name}',
            'narrative_context': {
                'median_slope':       round(median_slope, 4) if not np.isnan(median_slope) else None,
                'anchors_qualifying': len(slopes),
            }
        })

    slope_str = f"{median_slope:+.4f}" if not np.isnan(median_slope) else "    n/a"
    pit_tag   = " [PIT]" if current_lap in pit_laps else ""
    pos_tag   = f"P{int(prev_position)}→P{int(current_position)}" if positions_gained > 0 else f"P{int(current_position)}"
    print(f"{current_lap:>4}  {new_state.name + pit_tag:^30}  {slope_str:>14} s/lap"
          f"  {len(slopes):>6}  {anchors_agreeing if slopes else 0:>6}  {pos_tag:>6}")

    state          = new_state
    prev_slope     = median_slope if not np.isnan(median_slope) else prev_slope
    prev_position  = current_position

# ── Console narrative output ───────────────────────────────────────────────────
print("\n" + "=" * 70)
print("  RACE NARRATIVE TIMELINE")
print("=" * 70)
for evt in events:
    ctx  = evt['narrative_context']
    hint = ctx.get('ai_prompt_hint', '')
    print(f"\n  [Lap {evt['lap']:2d}  t={evt['session_time']:.0f}s]  {evt['event_type']}")
    for k, v in ctx.items():
        if k != 'ai_prompt_hint' and v is not None:
            print(f"    {k}: {v}")
    if hint:
        print(f"    → \"{hint}\"")

print("\n\nFull event JSON:")
def _json_default(obj):
    """Convert numpy scalars to native Python types for JSON serialisation."""
    if hasattr(obj, 'item'):
        return obj.item()
    raise TypeError(f'Object of type {type(obj).__name__} is not JSON serialisable')

print(json.dumps(events, indent=2, default=_json_default))

# ── Visualisation: 4-panel validation plot ────────────────────────────────────
fig, axes = plt.subplots(4, 1, figsize=(18, 22), gridspec_kw={'height_ratios': [3, 2, 2, 1.5]})
fig.suptitle(
    'Prototype Narrative Engine — Nürburgring Race\n'
    'Does the engine see what you know happened?',
    fontsize=14, fontweight='bold', y=0.99
)

lap_colours = {1: '#e74c3c', 2: '#3498db', 3: '#2ecc71', 4: '#f39c12', 5: '#9b59b6'}
event_colours = {
    'OVERTAKE': '#e74c3c',
    'PUSH': '#f39c12',
    'ATTACK_SETUP': '#8e44ad',
}

# ─ Panel 1: CarDistAhead timeline with event markers ─────────────────────────
ax1 = axes[0]
for lap in all_laps:
    ldf = df[df['Lap'] == lap]
    ax1.plot(ldf['SessionTime'], ldf['CarDistAhead'],
             color=lap_colours.get(lap, '#aaa'), alpha=0.75, linewidth=0.7,
             label=f'Lap {lap}')

ax1.axhline(BATTLE_THRESHOLD_M, color='orange', linestyle='--', linewidth=1.2,
            label=f'Battle threshold ({BATTLE_THRESHOLD_M:.0f}m)')

# Yellow zone spans
for (lap, p0, p1) in YELLOW_ZONES:
    lap_df = df[df['Lap'] == lap]
    if len(lap_df) == 0:
        continue
    t0 = lap_df[lap_df['LapDistPct'].between(p0 - 0.01, p0 + 0.01)]['SessionTime'].mean()
    t1 = lap_df[lap_df['LapDistPct'].between(p1 - 0.01, p1 + 0.01)]['SessionTime'].mean()
    if not (np.isnan(t0) or np.isnan(t1)):
        ax1.axvspan(t0, t1, alpha=0.12, color='yellow', label=f'Yellow (Lap {lap})')

for evt in events:
    et = evt['event_type']
    base = next((k for k in event_colours if k in et), None)
    if base:
        col = event_colours[base]
        ax1.axvline(evt['session_time'], color=col, linestyle=':', linewidth=1.5, alpha=0.9)
        ax1.text(evt['session_time'] + 2, 290, et.replace('STATE_', ''),
                 rotation=90, fontsize=7, color=col, va='top')

ax1.set_ylim(0, 320)
ax1.set_ylabel('CarDistAhead (m)')
ax1.set_title('Gap to Car Ahead — narrative event markers overlaid')
ax1.legend(loc='upper right', fontsize=7, ncol=3)

# ─ Panel 2: Per-anchor slope heatmap (WHERE) ─────────────────────────────────
ax2 = axes[1]
lap_list = [l for l in all_laps if l != all_laps[0]]
heat = np.full((len(lap_list), anchor_count), np.nan)
for i, lap in enumerate(lap_list):
    for bucket, slope in heatmap_data.get(lap, {}).items():
        heat[i, bucket] = slope

vmax = 0.25
im = ax2.imshow(heat, aspect='auto', cmap='RdYlGn', vmin=-vmax, vmax=vmax,
                extent=[0, 1, len(lap_list) + 0.5, 0.5])
ax2.set_xlabel('Track position (LapDistPct)')
ax2.set_ylabel('After lap')
ax2.set_yticks(range(1, len(lap_list) + 1))
ax2.set_yticklabels([f'Lap {l}' for l in lap_list])
ax2.set_title('Per-Anchor Slope Heatmap — WHERE is the gap closing? (Green = closing, Red = opening, Grey = insufficient data)')
plt.colorbar(im, ax=ax2, label='slope (s/lap)', shrink=0.7, orientation='vertical')

# Mark yellow zones on heatmap x-axis
for (lap, p0, p1) in YELLOW_ZONES:
    ax2.axvspan(p0, p1, alpha=0.15, color='yellow')

# ─ Panel 3: Median slope bar chart (WHETHER) ─────────────────────────────────
ax3 = axes[2]
laps_x    = list(median_slopes_log.keys())
slopes_y  = [median_slopes_log[l] for l in laps_x]
bar_cols  = []
for l, s in zip(laps_x, slopes_y):
    if l in pit_laps:
        bar_cols.append('#aaa')
    elif np.isnan(s):
        bar_cols.append('#ddd')
    elif s <= ATTACK_SLOPE_THRESHOLD:
        bar_cols.append('#8e44ad')
    elif s <= PUSH_SLOPE_THRESHOLD:
        bar_cols.append('#f39c12')
    else:
        bar_cols.append('#3498db')

ax3.bar(laps_x, [0 if np.isnan(s) else s for s in slopes_y], color=bar_cols, alpha=0.8, width=0.6)
ax3.axhline(PUSH_SLOPE_THRESHOLD,   color='#f39c12', linestyle='--', linewidth=1.5,
            label=f'PUSH threshold ({PUSH_SLOPE_THRESHOLD} s/lap)')
ax3.axhline(ATTACK_SLOPE_THRESHOLD, color='#8e44ad', linestyle='--', linewidth=1.5,
            label=f'ATTACK threshold ({ATTACK_SLOPE_THRESHOLD} s/lap)')
ax3.axhline(0, color='black', linewidth=0.8)

# Annotate qualifying anchor count
for lap, slope in zip(laps_x, slopes_y):
    n = len(heatmap_data.get(lap, {}))
    if n > 0 and not np.isnan(slope):
        ax3.text(lap, slope - 0.004, f'n={n}', ha='center', va='top', fontsize=8)

ax3.set_xlabel('Lap')
ax3.set_ylabel('Median slope (s/lap)')
ax3.set_title('Two-Tier Aggregate — WHETHER to classify (orange=PUSH, purple=ATTACK_SETUP, grey=pit)')
ax3.legend(fontsize=8)
ax3.set_xticks(laps_x)

# ─ Panel 4: Position timeline ────────────────────────────────────────────────
ax4 = axes[3]
pos_laps = list(lap_ends.keys())
pos_vals = [lap_ends[l]['position'] for l in pos_laps]
ax4.step(pos_laps, pos_vals, where='post', color='#2c3e50', linewidth=2, marker='o', markersize=7)
ax4.invert_yaxis()
ax4.set_xlabel('Lap')
ax4.set_ylabel('Position')
ax4.set_title('Race Position — overtakes only visible at lap crossings (engine limitation)')
ax4.set_xticks(pos_laps)

for evt in events:
    if 'OVERTAKE' in evt['event_type']:
        ctx = evt['narrative_context']
        y = ctx['position_to']
        ax4.annotate(
            f"P{ctx['position_from']}→P{ctx['position_to']}\n(+{ctx['positions_gained']})",
            xy=(evt['lap'], y), xytext=(evt['lap'] + 0.15, y + 1.5),
            fontsize=8, color='#e74c3c',
            arrowprops=dict(arrowstyle='->', color='#e74c3c', lw=1.2)
        )

plt.tight_layout(rect=[0, 0, 1, 0.98])
out_path = 'scripts/prototype_narrative.png'
plt.savefig(out_path, dpi=150, bbox_inches='tight')
print(f"\nSaved → {out_path}")
