import csv
import math
import pandas as pd
import matplotlib.pyplot as plt

# =============================================================================
# CONFIGURATION
# iRacing Telemetry Variables:
#   SessionTime : Total elapsed time in seconds
#   Lap         : Current lap number
#   LapDistPct  : Percentage around the track (0.0 to 1.0)
#   GapToAhead  : Simulated CarIdxF2Time (seconds)
# =============================================================================
FILENAME = "synthetic_race_data.csv"
POLLING_RATE_HZ = 5
TICK_INTERVAL = 1.0 / POLLING_RATE_HZ
LAP_TIME_SECONDS = 90.0  # 1 minute 30 second lap
TOTAL_LAPS = 4
NUM_ANCHORS = 10

# =============================================================================
# STEP 1: GENERATE SYNTHETIC TELEMETRY
# =============================================================================
with open(FILENAME, mode='w', newline='') as file:
    writer = csv.writer(file)
    writer.writerow(["SessionTime", "Lap", "LapDistPct", "GapToAhead"])

    session_time = 0.0

    for lap in range(1, TOTAL_LAPS + 1):
        lap_time_elapsed = 0.0

        while lap_time_elapsed < LAP_TIME_SECONDS:
            # Track position (0.0 to 1.0)
            lap_dist_pct = lap_time_elapsed / LAP_TIME_SECONDS

            # Model the "Accordion Effect": sine wave peaking 4 times per lap,
            # simulating 4 major braking zones
            accordion_compression = math.sin(lap_dist_pct * math.pi * 8) * 0.25

            # Model the strategic narrative
            if lap <= 2:
                # DRAFT TRAIN: Saving tires, holding steady at ~1.2s base gap
                base_gap = 1.2
            elif lap == 3:
                # THE PUSH: Base gap drops linearly from 1.2s down to 0.4s
                base_gap = 1.2 - (0.8 * lap_dist_pct)
            else:
                # THE ATTACK: Hovering around 0.3s, looking for a way past
                base_gap = 0.3

            # Final gap = base strategy + track physics (accordion)
            current_gap = max(base_gap + accordion_compression, -0.1)

            writer.writerow([
                round(session_time, 3),
                lap,
                round(lap_dist_pct, 4),
                round(current_gap, 3)
            ])

            session_time += TICK_INTERVAL
            lap_time_elapsed += TICK_INTERVAL

print(f"Generated synthetic telemetry: {FILENAME}")

# =============================================================================
# STEP 2: SPATIAL ANCHOR ANALYSIS
# =============================================================================
df = pd.read_csv(FILENAME)

# Divide the track into NUM_ANCHORS micro-sectors via LapDistPct
df['Anchor'] = (df['LapDistPct'] * NUM_ANCHORS).astype(int)

# Extract the first telemetry tick that crosses into each anchor bucket per lap
anchors_df = df.drop_duplicates(subset=['Lap', 'Anchor'], keep='first').copy()

# Lap-over-lap delta at each spatial anchor — the clean strategic signal.
# Accordion noise cancels out because both laps experience the same braking
# compression at the same spatial coordinate.
anchors_df['PrevLapGap'] = anchors_df.groupby('Anchor')['GapToAhead'].shift(1)
anchors_df['AnchorDelta'] = anchors_df['GapToAhead'] - anchors_df['PrevLapGap']

# =============================================================================
# STEP 3: VISUALIZE
# =============================================================================
fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(12, 8), sharex=True)

# Top: Raw noisy gap vs. clean spatial anchor points
ax1.plot(df['SessionTime'], df['GapToAhead'], color='lightgray', label='Raw 5Hz Gap (Noisy Accordion)')
ax1.plot(anchors_df['SessionTime'], anchors_df['GapToAhead'], 'ro-', markersize=5, label='Spatial Anchors (Clean Trend)')
ax1.set_ylabel('Gap to Car Ahead (Seconds)')
ax1.set_title('Filtering the Accordion Effect via Dynamic Spatial Anchoring')
ax1.legend()
ax1.grid(True, alpha=0.3)

# Bottom: The clean strategic delta (negative = gaining on the car ahead)
# Lap 1 is excluded — no previous lap to compare against
deltas = anchors_df.dropna(subset=['AnchorDelta'])
ax2.bar(deltas['SessionTime'], deltas['AnchorDelta'], width=2.0, color='royalblue', label='Closing Rate (Negative = Gaining)')
ax2.axhline(0, color='black', linewidth=1)
ax2.set_ylabel('Lap-over-Lap Delta (Seconds)')
ax2.set_xlabel('Session Time (Seconds)')
ax2.legend()
ax2.grid(True, alpha=0.3)

plt.tight_layout()
plt.savefig("spatial_anchors_plot.png")
print("Analysis complete. Saved visualization to spatial_anchors_plot.png")