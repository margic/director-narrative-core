"""
eda_nurburgring.py
==================
Deep-dive exploration of real iRacing telemetry from:
  Paul Crofts, Car #5, Porsche 992 GT3 Cup
  IMSA Fixed · Nürburgring Combined Long · 2026-05-24

The CSV was exported from the .ibt file and contains ONLY the player's own
car telemetry. Multi-car variables (CarIdxF2Time, etc.) are not available
in .ibt files — only CarDistAhead / CarDistBehind in metres.

Five visual panels:
  1. Full race position story (PlayerCarPosition over time)
  2. CarDistAhead timeline — when was Paul in a battle?
  3. Battle heat map — WHERE on track do battles concentrate?
  4. Close-battle speed/throttle/brake profile (Lap 2 intense battle)
  5. Lap comparison — anchor-sampled CarDistAhead per lap

Note: SessionFlags was not exported from the .ibt. Known yellow wave
sectors (LapDistPct ~0.62, Laps 1 and 2) are annotated manually.

Outputs: scripts/eda_nurburgring_deep_dive.png
"""

import numpy as np
import pandas as pd
import matplotlib.pyplot as plt
import matplotlib.patches as mpatches
from matplotlib.collections import LineCollection
from scipy.ndimage import gaussian_filter1d
import os

# ── Load data ─────────────────────────────────────────────────────────────────
DATA_PATH = os.path.join(
    os.path.dirname(__file__), '..', 'data',
    'porsche992rgt3_nurburgring combinedlong 2026-05-24 06-54-35.csv'
)
df = pd.read_csv(DATA_PATH)

# ── Clean up ───────────────────────────────────────────────────────────────────
# Sentinel value: 500,000m means no car in range
SENTINEL = 490_000
df['CarDistAhead_m']    = df['CarDistAhead'].where(df['CarDistAhead'] < SENTINEL, np.nan)
df['CarDistBehind_m']   = df['CarDistBehind'].where(df['CarDistBehind'] < SENTINEL, np.nan)
df['Speed_kph']         = df['Speed'] * 3.6          # m/s → km/h
df['Throttle_pct']      = df['Throttle'] * 100
df['Brake_pct']         = df['Brake'] * 100
# Only on-track laps 1-4 (lap 0 = formation, lap 5 = cool-down)
df_race = df[(df['Lap'] >= 1) & (df['Lap'] <= 4) & (df['IsOnTrack'] == True)].copy()

# Known yellow wave sectors (from manual race analysis, SessionFlags not in CSV)
YELLOW_SECTORS = [
    {'lap': 1, 'ldp': 0.625, 'label': 'Yellow Lap 1\n(~0.625)'},
    {'lap': 2, 'ldp': 0.616, 'label': 'Yellow Lap 2\n(~0.616)'},
]

# Battle threshold: CarDistAhead < 50m = genuine close battle
BATTLE_THRESHOLD_M = 50.0

# Approximate lap start times from session time
LAP_STARTS = df_race.groupby('Lap')['SessionTime'].min().to_dict()
LAP_ENDS   = df_race.groupby('Lap')['SessionTime'].max().to_dict()

# Colours
LAP_COLORS = {1: '#e74c3c', 2: '#3498db', 3: '#2ecc71', 4: '#f39c12'}
GRID_COLOR = '#f0f0f0'

# ── Dynamic anchor count (per spec: last_lap_time / TARGET_CADENCE) ───────────
TARGET_CADENCE_S = 5.0
# Use Lap 1 time as baseline
lap1_time = df_race[df_race['Lap'] == 1]['LapCurrentLapTime'].max()
NUM_ANCHORS = max(10, int(lap1_time / TARGET_CADENCE_S))
print(f'Lap 1 time: {lap1_time:.0f}s → {NUM_ANCHORS} anchors '
      f'(cadence: {lap1_time/NUM_ANCHORS:.1f}s)')

# ── Anchor-sampled CarDistAhead per lap ───────────────────────────────────────
df_race = df_race.copy()
df_race['Anchor'] = (df_race['LapDistPct'] * NUM_ANCHORS).astype(int).clip(0, NUM_ANCHORS - 1)
anchors = (df_race.groupby(['Lap', 'Anchor'])['CarDistAhead_m']
                  .first()
                  .reset_index())

# ── Figure layout ─────────────────────────────────────────────────────────────
fig = plt.figure(figsize=(16, 18))
fig.patch.set_facecolor('white')
gs = fig.add_gridspec(
    5, 2,
    hspace=0.50, wspace=0.30,
    left=0.07, right=0.97, top=0.95, bottom=0.04
)

ax1 = fig.add_subplot(gs[0, :])    # Position story (full width)
ax2 = fig.add_subplot(gs[1, :])    # CarDistAhead timeline (full width)
ax3 = fig.add_subplot(gs[2, 0])    # Battle heat map
ax4 = fig.add_subplot(gs[2, 1])    # Anchor-sampled gaps per lap
ax5 = fig.add_subplot(gs[3:, :])   # Battle speed/throttle/brake (tall)

fig.suptitle(
    'Nürburgring Combined Long — Paul Crofts, Car #5, Porsche 992 GT3 Cup\n'
    'Real iRacing .ibt Telemetry · IMSA Fixed · 2026-05-24',
    fontsize=13, fontweight='bold', y=0.975
)

# ── Panel 1: Position story ───────────────────────────────────────────────────
ax1.plot(df_race['SessionTime'], df_race['PlayerCarPosition'],
         color='#2c3e50', lw=1.5, alpha=0.8)

# Annotate key position changes
annotations = [
    (LAP_STARTS[1], 18, 'P18 (grid)'),
    (LAP_STARTS[2], 11, 'P11 → Lap 2\n(8 cars ahead had incidents)'),
    (LAP_STARTS[3],  9, 'P9 → Lap 3'),
    (LAP_STARTS[4],  8, 'P8 → Lap 4\n(after pit stop)'),
]
for t, pos, label in annotations:
    ax1.annotate(label, xy=(t, pos), xytext=(t + 30, pos - 1.2),
                 arrowprops=dict(arrowstyle='->', color='#7f8c8d', lw=1.2),
                 fontsize=8, color='#2c3e50')

# Final position P5 at end
t_final = df_race['SessionTime'].max()
ax1.annotate('Finish: P5 ✓', xy=(t_final - 20, 5),
             xytext=(t_final - 150, 3),
             arrowprops=dict(arrowstyle='->', color='#27ae60', lw=1.5),
             fontsize=9, color='#27ae60', fontweight='bold')

# Lap boundary lines
for lap, t in LAP_STARTS.items():
    ax1.axvline(t, color='#bdc3c7', lw=0.8, ls='--', alpha=0.6)
    ax1.text(t + 5, 19.5, f'Lap {lap}', fontsize=8, color='#7f8c8d')

ax1.set_ylim(21, 0)    # Invert: P1 at top
ax1.set_ylabel('Race Position')
ax1.set_title('Panel 1 — Race Position over Time  (iRacing positions update at lap crossings only)', fontsize=10)
ax1.yaxis.set_major_locator(plt.MultipleLocator(2))
ax1.yaxis.grid(True, color=GRID_COLOR); ax1.set_axisbelow(True)
ax1.set_xticklabels([])

# ── Panel 2: CarDistAhead timeline ────────────────────────────────────────────
for lap, color in LAP_COLORS.items():
    d = df_race[df_race['Lap'] == lap]
    ax2.plot(d['SessionTime'], d['CarDistAhead_m'],
             color=color, lw=0.8, alpha=0.75, label=f'Lap {lap}')

ax2.axhline(BATTLE_THRESHOLD_M, color='#e74c3c', lw=1.2, ls='--', alpha=0.7,
            label=f'Battle threshold ({BATTLE_THRESHOLD_M:.0f}m)')
ax2.fill_between(
    df_race['SessionTime'],
    df_race['CarDistAhead_m'].clip(upper=BATTLE_THRESHOLD_M),
    BATTLE_THRESHOLD_M,
    where=df_race['CarDistAhead_m'] < BATTLE_THRESHOLD_M,
    alpha=0.18, color='#e74c3c', label='In close battle'
)

# Annotate known yellow sectors
for ys in YELLOW_SECTORS:
    lap_data = df_race[df_race['Lap'] == ys['lap']]
    if not lap_data.empty:
        ldp_match = lap_data.iloc[(lap_data['LapDistPct'] - ys['ldp']).abs().argsort()[:1]]
        t_yellow = ldp_match['SessionTime'].values[0]
        ax2.axvline(t_yellow, color='#f39c12', lw=1.5, ls=':', alpha=0.8)
        ax2.text(t_yellow + 5, 420, ys['label'], fontsize=7.5, color='#e67e22',
                 fontweight='bold')

# Lap boundaries
for lap, t in LAP_STARTS.items():
    ax2.axvline(t, color='#bdc3c7', lw=0.8, ls='--', alpha=0.6)

ax2.set_ylim(-5, 500)
ax2.set_ylabel('Car Distance Ahead (m)')
ax2.set_title('Panel 2 — CarDistAhead over Session Time\n'
              '(NaN = no car within range · yellow lines = known flag sectors)', fontsize=10)
ax2.legend(ncol=3, fontsize=8, loc='upper right')
ax2.yaxis.grid(True, color=GRID_COLOR); ax2.set_axisbelow(True)
ax2.set_xticklabels([])

# ── Panel 3: Battle heat map — WHERE on track ─────────────────────────────────
battle_df = df_race[df_race['CarDistAhead_m'] < BATTLE_THRESHOLD_M]

for lap, color in LAP_COLORS.items():
    d = battle_df[battle_df['Lap'] == lap]
    if len(d) == 0:
        continue
    # KDE-style density
    hist, edges = np.histogram(d['LapDistPct'], bins=60, range=(0, 1), density=True)
    centres     = (edges[:-1] + edges[1:]) / 2
    smoothed    = gaussian_filter1d(hist, sigma=1.5)
    ax3.fill_between(centres, smoothed, alpha=0.35, color=color, label=f'Lap {lap}')
    ax3.plot(centres, smoothed, color=color, lw=1.5, alpha=0.8)

# Annotate yellow sectors
for ys in YELLOW_SECTORS:
    ax3.axvline(ys['ldp'], color='#f39c12', lw=1.5, ls=':', alpha=0.8)
    ax3.text(ys['ldp'] + 0.01, ax3.get_ylim()[1] * 0.9 if ax3.get_ylim()[1] > 0 else 1,
             ys['label'], fontsize=7.5, color='#e67e22')

ax3.set_xlabel('Track Position (LapDistPct)')
ax3.set_ylabel('Battle density\n(CarDistAhead < 50m)')
ax3.set_title(f'Panel 3 — WHERE on track does close racing happen?\n'
              f'(density of moments with CarDistAhead < {BATTLE_THRESHOLD_M:.0f}m)', fontsize=10)
ax3.set_xlim(-0.02, 1.02)
ax3.legend(fontsize=8)
ax3.yaxis.grid(True, color=GRID_COLOR); ax3.set_axisbelow(True)

# ── Panel 4: Anchor-sampled gap per lap ───────────────────────────────────────
for lap, color in LAP_COLORS.items():
    d = anchors[anchors['Lap'] == lap].dropna(subset=['CarDistAhead_m'])
    if d.empty:
        continue
    ax4.plot(d['Anchor'] / NUM_ANCHORS, d['CarDistAhead_m'],
             color=color, lw=1.5, alpha=0.8, marker='o', ms=2.5,
             label=f'Lap {lap}')

ax4.axhline(BATTLE_THRESHOLD_M, color='#e74c3c', lw=1.0, ls='--', alpha=0.6,
            label=f'Battle threshold')
# Annotate yellow sectors
for ys in YELLOW_SECTORS:
    ax4.axvline(ys['ldp'], color='#f39c12', lw=1.2, ls=':', alpha=0.7)

ax4.set_xlabel('Track Position (LapDistPct)')
ax4.set_ylabel('Car Distance Ahead (m)\nat anchor crossing')
ax4.set_title(f'Panel 4 — Anchor-sampled CarDistAhead per lap\n'
              f'({NUM_ANCHORS} anchors, ~{lap1_time/NUM_ANCHORS:.0f}s cadence)', fontsize=10)
ax4.set_xlim(-0.02, 1.02); ax4.set_ylim(-5, 400)
ax4.legend(fontsize=8)
ax4.yaxis.grid(True, color=GRID_COLOR); ax4.set_axisbelow(True)

# ── Panel 5: Closest battle — Speed / Throttle / Brake ───────────────────────
# Find the longest continuous battle window (CarDistAhead < 50m, Lap 2)
lap2 = df_race[(df_race['Lap'] == 2) & df_race['CarDistAhead_m'].notna()].copy()
lap2['in_battle'] = lap2['CarDistAhead_m'] < BATTLE_THRESHOLD_M
# Find the longest consecutive battle sequence
lap2['grp'] = (lap2['in_battle'] != lap2['in_battle'].shift()).cumsum()
battle_groups = lap2[lap2['in_battle']].groupby('grp')['SessionTime']
if not battle_groups.first().empty:
    longest_grp   = (battle_groups.last() - battle_groups.first()).idxmax()
    t_start       = battle_groups.first()[longest_grp] - 5
    t_end         = battle_groups.last()[longest_grp]  + 5
    battle_detail = lap2[(lap2['SessionTime'] >= t_start) &
                         (lap2['SessionTime'] <= t_end)].copy()
    battle_detail['RelTime'] = battle_detail['SessionTime'] - t_start
    duration = t_end - t_start
else:
    battle_detail = lap2.head(200).copy()
    battle_detail['RelTime'] = battle_detail['SessionTime'] - battle_detail['SessionTime'].iloc[0]
    duration = battle_detail['RelTime'].max()

ax5b = ax5.twinx()

# Speed on primary y-axis
ax5.fill_between(battle_detail['RelTime'], battle_detail['Speed_kph'],
                 alpha=0.15, color='#3498db')
ax5.plot(battle_detail['RelTime'], battle_detail['Speed_kph'],
         color='#3498db', lw=1.8, label='Speed (km/h)', alpha=0.9)

# Throttle and Brake on secondary (0-100%)
ax5b.fill_between(battle_detail['RelTime'], battle_detail['Throttle_pct'],
                  alpha=0.18, color='#2ecc71')
ax5b.fill_between(battle_detail['RelTime'], -battle_detail['Brake_pct'],
                  alpha=0.18, color='#e74c3c')
ax5b.plot(battle_detail['RelTime'], battle_detail['Throttle_pct'],
          color='#2ecc71', lw=1.4, label='Throttle %', alpha=0.8)
ax5b.plot(battle_detail['RelTime'], -battle_detail['Brake_pct'],
          color='#e74c3c', lw=1.4, label='Brake % (inverted)', alpha=0.8)

# CarDistAhead overlaid as dashed grey
ax5_twin2 = ax5.twinx()
ax5_twin2.spines['right'].set_position(('axes', 1.07))
ax5_twin2.plot(battle_detail['RelTime'], battle_detail['CarDistAhead_m'],
               color='#9b59b6', lw=2.0, ls='--', alpha=0.8,
               label='CarDistAhead (m)')
ax5_twin2.set_ylabel('CarDistAhead (m)', color='#9b59b6', fontsize=9)
ax5_twin2.tick_params(axis='y', labelcolor='#9b59b6')
ax5_twin2.set_ylim(-5, 200)

ax5.set_xlabel('Time within battle window (s)')
ax5.set_ylabel('Speed (km/h)', color='#3498db', fontsize=9)
ax5.tick_params(axis='y', labelcolor='#3498db')
ax5b.set_ylabel('Throttle / Brake (%)', color='#555', fontsize=9)
ax5b.set_ylim(-120, 120)
ax5b.axhline(0, color='#bdc3c7', lw=0.8)
ax5.set_title(
    f'Panel 5 — Lap 2 Longest Close Battle ({duration:.0f}s window)\n'
    'Speed / Throttle / Brake / CarDistAhead — the raw signals underneath a battle',
    fontsize=10
)
ax5.yaxis.grid(True, color=GRID_COLOR); ax5.set_axisbelow(True)

# Combined legend
lines1, labels1 = ax5.get_legend_handles_labels()
lines2, labels2 = ax5b.get_legend_handles_labels()
lines3, labels3 = ax5_twin2.get_legend_handles_labels()
ax5.legend(lines1 + lines2 + lines3, labels1 + labels2 + labels3,
           fontsize=9, loc='upper right')

# ── Save ──────────────────────────────────────────────────────────────────────
OUT = os.path.join(os.path.dirname(__file__), 'eda_nurburgring_deep_dive.png')
plt.savefig(OUT, dpi=150, bbox_inches='tight')
print(f'Saved → {OUT}')
plt.close()
