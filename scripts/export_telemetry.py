import csv
import os
import sys
import irsdk

# =============================================================================
# CONFIGURATION
# Pass the path to a .ibt file as a command-line argument, or set IBT_FILE below.
# Output CSV is written to the same directory as the source .ibt file.
#
# Exported variables (all scalar, recorded from the player's car):
#   SessionTime            - Elapsed session time (seconds)
#   SessionFlags           - Bitmask of active flags (0x100=YELLOW_WAVE, 0x4000=CAUTION)
#   Lap                    - Current lap number
#   LapDistPct             - Track position (0.0 = start/finish, 1.0 = full lap)
#   LapCurrentLapTime      - Time elapsed in the current lap (seconds)
#   LapLastLapTime         - Last completed lap time (seconds)
#   Speed                  - Speed (m/s)
#   RPM                    - Engine RPM
#   Gear                   - Current gear (-1=R, 0=N, 1-8=drive)
#   Throttle               - Throttle application (0.0 to 1.0)
#   Brake                  - Brake application (0.0 to 1.0)
#   SteeringWheelAngle     - Steering angle (radians, positive = left)
#   LongAccel              - Longitudinal acceleration (m/s²)
#   LatAccel               - Lateral acceleration (m/s²)
#   CarDistAhead           - Distance to car ahead (meters; 500,000 = no car)
#   CarDistBehind          - Distance to car behind (meters; 500,000 = no car)
#   OnPitRoad              - 1 if on pit road, 0 otherwise
#   PlayerCarClassPosition - Position within class
#   PlayerCarPosition      - Overall race position
#   PlayerCarIdx           - The player's own CarIdx (constant for the session)
#   IsOnTrack              - 1 if car is on track
#
# NOTE — CarIdx* array variables (CarIdxF2Time, CarIdxLapDistPct, CarIdxPosition)
# are NOT available in .ibt files. They exist only in the live iRacing memory-mapped
# API. This means opponent identity (which specific car is ahead) cannot be derived
# from .ibt recordings. The engine's closing-rate regression requires the live API
# to key state machines on (opponent_car_idx, anchor_bucket). See spec §4.
# =============================================================================

EXPORT_VARS = [
    "SessionTime",
    "SessionFlags",
    "Lap",
    "LapDistPct",
    "LapCurrentLapTime",
    "LapLastLapTime",
    "Speed",
    "RPM",
    "Gear",
    "Throttle",
    "Brake",
    "SteeringWheelAngle",
    "LongAccel",
    "LatAccel",
    "CarDistAhead",
    "CarDistBehind",
    "OnPitRoad",
    "PlayerCarClassPosition",
    "PlayerCarPosition",
    "PlayerCarIdx",
    "IsOnTrack",
]

# =============================================================================
# RESOLVE INPUT FILE
# =============================================================================
if len(sys.argv) > 1:
    ibt_path = sys.argv[1]
else:
    # Default to the Nurburgring race file in the data directory
    ibt_path = os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "data",
        "porsche992rgt3_nurburgring combinedlong 2026-05-24 06-54-35.ibt",
    )

if not os.path.isfile(ibt_path):
    print(f"ERROR: File not found: {ibt_path}", file=sys.stderr)
    sys.exit(1)

output_path = os.path.splitext(ibt_path)[0] + ".csv"

# =============================================================================
# LOAD SESSION METADATA (driver name, car number, car index)
# =============================================================================
ir = irsdk.IRSDK()
ir.startup(test_file=ibt_path)

driver_info = ir["DriverInfo"] or {}
player_car_idx = None
player_car_number = "?"
player_name = "?"

for driver in driver_info.get("Drivers", []):
    # DriverCarIdx is the session's own driver index
    if driver.get("CarIdx") == driver_info.get("DriverCarIdx"):
        player_car_idx = driver["CarIdx"]
        player_car_number = driver.get("CarNumber", "?")
        player_name = driver.get("UserName", "?")
        break

ir.shutdown()

# =============================================================================
# EXPORT TICK DATA VIA IBT
# =============================================================================
ibt = irsdk.IBT()
ibt.open(ibt_path)

total_ticks = ibt._disk_header.session_record_count

# Resolve PlayerCarIdx from tick data as a fallback
if player_car_idx is None:
    player_car_idx = ibt.get(0, "PlayerCarIdx")

print(f"File       : {os.path.basename(ibt_path)}")
print(f"Driver     : {player_name}  (Car #{player_car_number}, CarIdx {player_car_idx})")
print(f"Total ticks: {total_ticks:,}")
print(f"Duration   : {ibt.get(0, 'SessionTime'):.1f}s "
      f"-> {ibt.get(total_ticks - 1, 'SessionTime'):.1f}s")
print(f"Exporting  : {output_path}")

# Bulk-read each variable for all ticks at once (most efficient access pattern)
print("Reading variables...", end=" ", flush=True)
columns = {}
for var in EXPORT_VARS:
    data = ibt.get_all(var)
    if data is None:
        print(f"\nWARNING: Variable '{var}' not found in file, skipping.")
    else:
        columns[var] = data

ibt.close()
print("done.")

# Write CSV
print("Writing CSV...", end=" ", flush=True)
available_vars = list(columns.keys())

with open(output_path, "w", newline="") as f:
    writer = csv.writer(f)
    writer.writerow(available_vars)
    for i in range(total_ticks):
        writer.writerow([columns[var][i] for var in available_vars])

print("done.")
print(f"\nExported {total_ticks:,} rows to: {output_path}")
