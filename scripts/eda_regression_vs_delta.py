"""
eda_regression_vs_delta.py
==========================
Compares two approaches to computing closing rate from spatial anchor readings:

  Approach A: N vs N-1 delta  — compare this lap's gap to last lap's gap
  Approach B: Rolling linear regression slope — fit a trend line across all laps

The script deliberately injects a "dirty" lap (simulating a yellow flag lap
where the gap is artificially high) to show how each approach handles
contaminated data.

Outputs: scripts/eda_regression_vs_delta.png
"""

import numpy as np
import pandas as pd
import matplotlib.pyplot as plt
import matplotlib.patches as mpatches

# ── Configuration ─────────────────────────────────────────────────────────────
POLLING_HZ        = 5
LAP_TIME_S        = 90.0        # Short circuit (enough laps to show trend)
N_LAPS            = 7
TICK_INTERVAL     = 1.0 / POLLING_HZ
TARGET_CADENCE_S  = 5.0
NUM_ANCHORS       = int(LAP_TIME_S / TARGET_CADENCE_S)   # = 18
DIRTY_LAP         = 3           # Simulates a yellow flag disrupting Lap 3
FOCUS_ANCHOR      = 9           # The anchor we zoom into for panel 3

# ── Step 1: Generate synthetic gap signal ─────────────────────────────────────
# Gap profile (true strategic intent, before noise is added):
#   Laps 1-2  →  Draft train:  base gap ~1.4s  (static following)
#   Laps 3-5  →  The Push:     gap drops linearly 1.4 → 0.6s
#   Laps 6-7  →  Attack:       gap drops linearly 0.6 → 0.2s

np.random.seed(42)
ticks_per_lap = int(LAP_TIME_S * POLLING_HZ)
frames = []

for lap in range(1, N_LAPS + 1):
    if lap <= 2:
        base_gap = 1.4
    elif lap <= 5:
        t = (lap - 2) / 3          # 0 → 1 over laps 3–5
        base_gap = 1.4 - t * 0.8
    else:
        t = (lap - 5) / 2          # 0 → 1 over laps 6–7
        base_gap = 0.6 - t * 0.4

    for tick in range(ticks_per_lap):
        ldp = tick / ticks_per_lap
        # Accordion: 8 compression/expansion cycles per lap, ±0.35s amplitude
        accordion = np.sin(ldp * np.pi * 16) * 0.35
        # Yellow flag: artificially inflates gap on the dirty lap
        yellow_boost = 0.8 if lap == DIRTY_LAP else 0.0
        gap = base_gap + accordion + yellow_boost + np.random.normal(0, 0.02)
        frames.append({
            'Lap':         lap,
            'LapDistPct':  ldp,
            'GapToAhead':  max(0.05, gap),
            'SessionTime': (lap - 1) * LAP_TIME_S + tick * TICK_INTERVAL,
            'is_clean':    lap != DIRTY_LAP,
        })

df = pd.DataFrame(frames)

# ── Step 2: Spatial anchor sampling ───────────────────────────────────────────
df['Anchor'] = (df['LapDistPct'] * NUM_ANCHORS).astype(int).clip(0, NUM_ANCHORS - 1)
# First reading per (Lap, Anchor) = the anchor "crossing" value
anchors = (df.groupby(['Lap', 'Anchor'])
             .first()
             .reset_index()[['Lap', 'Anchor', 'GapToAhead', 'is_clean']])
anchors = anchors.sort_values(['Anchor', 'Lap']).reset_index(drop=True)

# ── Step 3: N vs N-1 delta ────────────────────────────────────────────────────
anchors['prev_gap'] = anchors.groupby('Anchor')['GapToAhead'].shift(1)
anchors['delta']    = anchors['GapToAhead'] - anchors['prev_gap']
# Negative delta = closing on opponent

# ── Step 4: Rolling linear regression slope ───────────────────────────────────
def slope(laps, gaps):
    """Ordinary least-squares slope of (lap, gap) pairs."""
    if len(laps) < 2:
        return np.nan
    x, y   = np.array(laps, dtype=float), np.array(gaps, dtype=float)
    xm, ym = x.mean(), y.mean()
    denom  = ((x - xm) ** 2).sum()
    return ((x - xm) * (y - ym)).sum() / denom if denom else np.nan

def regression_slopes_for_anchor(anchor_id, clean_only):
    a = anchors[anchors['Anchor'] == anchor_id]
    if clean_only:
        a = a[a['is_clean']]
    return slope(a['Lap'].tolist(), a['GapToAhead'].tolist())

anchor_positions   = np.arange(NUM_ANCHORS) / NUM_ANCHORS
slopes_all_data    = [regression_slopes_for_anchor(a, clean_only=False) for a in range(NUM_ANCHORS)]
slopes_clean_only  = [regression_slopes_for_anchor(a, clean_only=True)  for a in range(NUM_ANCHORS)]

# ── Step 5: Focus-anchor detail data ──────────────────────────────────────────
fa = anchors[anchors['Anchor'] == FOCUS_ANCHOR].copy()
fa_clean = fa[fa['is_clean']]
fa_dirty = fa[~fa['is_clean']]

# Regression lines for panel 3
def regression_line(laps, gaps):
    x, y   = np.array(laps, dtype=float), np.array(gaps, dtype=float)
    xm, ym = x.mean(), y.mean()
    denom  = ((x - xm) ** 2).sum()
    m      = ((x - xm) * (y - ym)).sum() / denom if denom else 0
    c      = ym - m * xm
    xs     = np.linspace(x.min(), x.max(), 100)
    return xs, m * xs + c, m

fa_all_laps   = fa['Lap'].tolist();   fa_all_gaps   = fa['GapToAhead'].tolist()
fa_clean_laps = fa_clean['Lap'].tolist(); fa_clean_gaps = fa_clean['GapToAhead'].tolist()

xs_all,   ys_all,   m_all   = regression_line(fa_all_laps,   fa_all_gaps)
xs_clean, ys_clean, m_clean = regression_line(fa_clean_laps, fa_clean_gaps)

# N vs N-1 deltas at focus anchor
fa_deltas       = fa.dropna(subset=['delta'])
fa_deltas_clean = fa_deltas[fa_deltas['is_clean']]
fa_deltas_dirty = fa_deltas[~fa_deltas['is_clean']]

# ── Step 6: Plot ──────────────────────────────────────────────────────────────
COLORS = {
    'clean':    '#2ecc71',
    'dirty':    '#e74c3c',
    'reg_all':  '#95a5a6',
    'reg_clean':'#3498db',
    'delta':    '#e67e22',
    'true_gap': '#9b59b6',
    'grid':     '#f0f0f0',
}

fig = plt.figure(figsize=(16, 14))
fig.patch.set_facecolor('white')

gs = fig.add_gridspec(
    3, 2,
    hspace=0.45, wspace=0.30,
    left=0.07, right=0.97, top=0.93, bottom=0.06
)

ax1 = fig.add_subplot(gs[0, :])    # Top — full-width raw signal
ax2 = fig.add_subplot(gs[1, 0])    # Middle-left — anchor sampled per lap
ax3 = fig.add_subplot(gs[1, 1])    # Middle-right — focus anchor regression
ax4 = fig.add_subplot(gs[2, 0])    # Bottom-left — N vs N-1 delta all anchors
ax5 = fig.add_subplot(gs[2, 1])    # Bottom-right — regression slope all anchors

fig.suptitle(
    'Spatial Anchoring: N vs N−1 Delta  vs  Rolling Linear Regression Slope\n'
    f'Synthetic {N_LAPS}-lap race · {NUM_ANCHORS} anchors · Lap {DIRTY_LAP} = dirty (yellow flag simulation)',
    fontsize=13, fontweight='bold', y=0.97
)

# ── Panel 1: Raw time-domain gap (one line per lap) ───────────────────────────
lap_colors = plt.cm.tab10(np.linspace(0, 0.9, N_LAPS))
for i, lap in enumerate(range(1, N_LAPS + 1)):
    d     = df[df['Lap'] == lap]
    alpha = 0.4 if lap == DIRTY_LAP else 0.75
    lw    = 1.0 if lap == DIRTY_LAP else 1.2
    ls    = '--' if lap == DIRTY_LAP else '-'
    ax1.plot(d['SessionTime'], d['GapToAhead'],
             color=lap_colors[i], alpha=alpha, lw=lw, ls=ls,
             label=f'Lap {lap}' + (' ← DIRTY (yellow flag)' if lap == DIRTY_LAP else ''))

ax1.set_title('Panel 1 — Raw Time-Domain Gap Signal (accordion noise clearly visible)', fontsize=11)
ax1.set_xlabel('Session Time (s)'); ax1.set_ylabel('Gap to Ahead (s)')
ax1.set_ylim(-0.1, 3.2)
ax1.yaxis.grid(True, color=COLORS['grid']); ax1.set_axisbelow(True)
ax1.legend(ncol=N_LAPS, fontsize=8, loc='upper right')
ax1.annotate(
    '← Accordion noise:\n   same as braking zones,\n   ~0.7s amplitude',
    xy=(20, 1.75), xytext=(35, 2.5),
    arrowprops=dict(arrowstyle='->', color='#7f8c8d'),
    fontsize=8, color='#7f8c8d'
)

# ── Panel 2: Anchor-sampled gaps (one line per lap) ───────────────────────────
for i, lap in enumerate(range(1, N_LAPS + 1)):
    d     = anchors[anchors['Lap'] == lap]
    alpha = 0.35 if lap == DIRTY_LAP else 0.85
    ls    = '--' if lap == DIRTY_LAP else '-'
    ax2.plot(d['Anchor'] / NUM_ANCHORS, d['GapToAhead'],
             color=lap_colors[i], alpha=alpha, lw=1.5, ls=ls, marker='o', ms=3,
             label=f'Lap {lap}' + (' DIRTY' if lap == DIRTY_LAP else ''))

ax2.axvline(FOCUS_ANCHOR / NUM_ANCHORS, color='#e74c3c', lw=1.2, ls=':', alpha=0.7,
            label=f'Focus anchor (§{FOCUS_ANCHOR / NUM_ANCHORS:.2f})')
ax2.set_title('Panel 2 — Anchor-Sampled Gaps per Lap\n(accordion noise cancelled; dirty lap visible as elevated line)', fontsize=10)
ax2.set_xlabel('Track Position (LapDistPct)'); ax2.set_ylabel('Gap at Anchor (s)')
ax2.set_ylim(-0.1, 3.2); ax2.set_xlim(-0.02, 1.02)
ax2.yaxis.grid(True, color=COLORS['grid']); ax2.set_axisbelow(True)
ax2.legend(ncol=2, fontsize=7)

# ── Panel 3: Focus-anchor detail — regression lines + delta bars ──────────────
# Plot raw points
ax3.scatter(fa_clean['Lap'], fa_clean['GapToAhead'],
            s=70, color=COLORS['clean'], zorder=5, label='Clean reading')
ax3.scatter(fa_dirty['Lap'], fa_dirty['GapToAhead'],
            s=120, color=COLORS['dirty'], marker='X', zorder=6,
            label=f'Dirty reading (Lap {DIRTY_LAP})')

# Regression lines
ax3.plot(xs_all,   ys_all,   color=COLORS['reg_all'],  lw=2.0, ls='--',
         label=f'Regression (all data), slope={m_all:.3f}s/lap')
ax3.plot(xs_clean, ys_clean, color=COLORS['reg_clean'], lw=2.5,
         label=f'Regression (clean only), slope={m_clean:.3f}s/lap')

# N vs N-1 delta arrows (delta shown as vertical distance between consecutive laps)
for _, row in fa_deltas_clean.iterrows():
    prev_gap = row['prev_gap']
    curr_gap = row['GapToAhead']
    ax3.annotate('', xy=(row['Lap'], curr_gap), xytext=(row['Lap'], prev_gap),
                 arrowprops=dict(arrowstyle='->', color=COLORS['delta'], lw=1.5))
for _, row in fa_deltas_dirty.iterrows():
    prev_gap = row['prev_gap']
    curr_gap = row['GapToAhead']
    ax3.annotate('', xy=(row['Lap'], curr_gap), xytext=(row['Lap'], prev_gap),
                 arrowprops=dict(arrowstyle='->', color=COLORS['dirty'], lw=2.0, linestyle='dashed'))
# Label the false signal
false_signal_row = fa_deltas[fa_deltas['Lap'] == DIRTY_LAP + 1]
if not false_signal_row.empty:
    row = false_signal_row.iloc[0]
    ax3.annotate(
        f'FALSE SIGNAL\nN vs N−1 = {row["delta"]:.2f}s\n(dirty lap inflated gap)',
        xy=(row['Lap'], row['GapToAhead'] + 0.05),
        xytext=(row['Lap'] + 0.4, row['GapToAhead'] + 0.7),
        arrowprops=dict(arrowstyle='->', color=COLORS['dirty']),
        color=COLORS['dirty'], fontsize=8, fontweight='bold'
    )

ax3.set_title(f'Panel 3 — Focus Anchor at {FOCUS_ANCHOR / NUM_ANCHORS:.0%} track position\n'
              'N vs N−1 arrows vs regression lines', fontsize=10)
ax3.set_xlabel('Lap Number'); ax3.set_ylabel('Gap at Anchor (s)')
ax3.set_xticks(range(1, N_LAPS + 1))
ax3.set_ylim(-0.1, 3.2)
ax3.yaxis.grid(True, color=COLORS['grid']); ax3.set_axisbelow(True)
ax3.legend(fontsize=8)

# ── Panel 4: N vs N-1 delta — all anchors, all lap transitions ───────────────
for i, lap in enumerate(range(2, N_LAPS + 1)):
    d     = anchors[(anchors['Lap'] == lap) & anchors['delta'].notna()]
    alpha = 0.3 if lap == DIRTY_LAP else 0.7
    ls    = '--' if lap == DIRTY_LAP else '-'
    label = f'Lap {lap-1}→{lap}' + (' DIRTY' if lap == DIRTY_LAP else '')
    ax4.plot(d['Anchor'] / NUM_ANCHORS, d['delta'],
             color=lap_colors[i], alpha=alpha, lw=1.3, ls=ls, label=label)

ax4.axhline(0, color='black', lw=0.8, ls=':')
ax4.set_title('Panel 4 — N vs N−1 Delta across all anchors\n(dirty lap creates a large misleading spike then collapse)', fontsize=10)
ax4.set_xlabel('Track Position (LapDistPct)'); ax4.set_ylabel('Δ Gap per lap transition (s)\n← negative = closing')
ax4.set_xlim(-0.02, 1.02)
ax4.yaxis.grid(True, color=COLORS['grid']); ax4.set_axisbelow(True)
ax4.legend(ncol=2, fontsize=7)

# ── Panel 5: Regression slope — all data vs clean only ───────────────────────
ax5.plot(anchor_positions, slopes_all_data,  color=COLORS['reg_all'],  lw=2.0, ls='--',
         label='Regression (all data, incl. dirty)')
ax5.plot(anchor_positions, slopes_clean_only, color=COLORS['reg_clean'], lw=2.5,
         label='Regression (clean data only)')
ax5.axhline(0, color='black', lw=0.8, ls=':')
ax5.fill_between(anchor_positions, slopes_clean_only, 0,
                 where=[s is not None and s < 0 for s in slopes_clean_only],
                 alpha=0.12, color=COLORS['reg_clean'], label='Closing (negative slope)')

ax5.set_title('Panel 5 — Rolling Regression Slope across all anchors\n(clean-only line is smooth and reliable; dirty data included barely shifts it)', fontsize=10)
ax5.set_xlabel('Track Position (LapDistPct)'); ax5.set_ylabel('Slope (s/lap)\n← negative = consistently closing')
ax5.set_xlim(-0.02, 1.02)
ax5.yaxis.grid(True, color=COLORS['grid']); ax5.set_axisbelow(True)
ax5.legend(fontsize=9)

# ── Save ──────────────────────────────────────────────────────────────────────
OUT = 'scripts/eda_regression_vs_delta.png'
plt.savefig(OUT, dpi=150, bbox_inches='tight')
print(f'Saved → {OUT}')
plt.close()
