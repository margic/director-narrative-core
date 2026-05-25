"""
export_replay.py  —  Stream full-field TelemetryFrames from an iRacing replay to JSONL.

REQUIREMENTS
============
  - Windows only (iRacing runs on Windows)
  - iRacing installed and running
  - pip install irsdk

USAGE
=====
1. Open iRacing and load the replay file (File → Open Replay).
2. Start playback at 1× speed.
3. In a separate terminal (while iRacing is running):

       python export_replay.py [output.jsonl] [--hz 5]

   Defaults:  output  = session.jsonl (same folder as this script)
              sample  = 5 Hz

4. The script will wait until iRacing connects, then stream data until
   the replay ends or you press Ctrl-C.
5. Copy the resulting .jsonl file to the dev container's  data/  folder.

OUTPUT FORMAT (one JSON object per line)
========================================
Each line is a TelemetryFrame matching spec §8:

{
  "session_time":            float,   // seconds elapsed in session
  "session_flags":           int,     // bitmask — 0x100=YELLOW_WAVE, 0x4000=CAUTION
  "player_car_idx":          int,     // player's own CarIdx (constant)
  "lap":                     int,
  "lap_dist_pct":            float,   // 0.0–1.0, player track position
  "lap_current_lap_time":    float,   // seconds since lap start
  "lap_last_lap_time":       float,   // last completed lap time (seconds)
  "speed":                   float,   // m/s
  "rpm":                     float,
  "gear":                    int,     // -1=R, 0=N, 1-8
  "throttle":                float,   // 0.0–1.0
  "brake":                   float,   // 0.0–1.0
  "steering_wheel_angle":    float,   // radians, positive=left
  "long_accel":              float,   // m/s²
  "lat_accel":               float,   // m/s²
  "on_pit_road":             bool,    // player on pit road
  "player_car_position":     int,     // overall race position
  "player_car_class_position": int,   // class position
  "is_on_track":             bool,
  "car_idx_lap_dist_pct":    [float × 64],  // track position for every car slot
  "car_idx_f2_time":         [float × 64],  // estimated time-to-leader gap (s)
  "car_idx_position":        [int   × 64],  // official race position per car
  "car_idx_on_pit_road":     [bool  × 64]   // pit-road flag per car
}

NOTES
=====
- car_idx_* arrays are indexed 0–63; unused slots contain 0/false.
- car_idx_f2_time: negative = ahead of leader, positive = behind.
- Replay playback speed should be 1× for accurate timestamps.
- The JSONL file is flushed after each write so you can Ctrl-C and keep
  all data up to that point.
"""

import argparse
import json
import sys
import time

try:
    import irsdk
except ImportError:
    print("ERROR: irsdk not installed.  Run:  pip install irsdk", file=sys.stderr)
    sys.exit(1)

# ---------------------------------------------------------------------------
# Session flag bitmask constants (matches iRacing SDK)
# ---------------------------------------------------------------------------
FLAG_CHECKERED       = 0x0001
FLAG_WHITE           = 0x0002
FLAG_GREEN           = 0x0004
FLAG_YELLOW          = 0x0008
FLAG_RED             = 0x0010
FLAG_BLUE            = 0x0020
FLAG_DEBRIS          = 0x0040
FLAG_CROSSED         = 0x0080
FLAG_YELLOW_WAVE     = 0x0100
FLAG_ONE_LAP_TO_GO   = 0x0200
FLAG_GREEN_HELD      = 0x0400
FLAG_TEN_TO_GO       = 0x0800
FLAG_FIVE_TO_GO      = 0x1000
FLAG_RANDOM_WAVE_EXP = 0x2000
FLAG_CAUTION         = 0x4000
FLAG_CAUTION_WAVE    = 0x8000

# Scalar variables to read from the player's own car
SCALAR_VARS = [
    "SessionTime",
    "SessionFlags",
    "PlayerCarIdx",
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
    "OnPitRoad",
    "PlayerCarPosition",
    "PlayerCarClassPosition",
    "IsOnTrack",
]

# Per-car array variables (each returns a list of 64 values)
ARRAY_VARS = [
    "CarIdxLapDistPct",
    "CarIdxF2Time",
    "CarIdxPosition",
    "CarIdxOnPitRoad",
]


def build_frame(ir: "irsdk.IRSDK") -> dict:
    """Read one snapshot from iRacing shared memory and return a TelemetryFrame dict."""
    frame: dict = {}

    # --- player scalars ---
    frame["session_time"]               = ir["SessionTime"]
    frame["session_flags"]              = int(ir["SessionFlags"])
    frame["player_car_idx"]             = int(ir["PlayerCarIdx"])
    frame["lap"]                        = int(ir["Lap"])
    frame["lap_dist_pct"]               = ir["LapDistPct"]
    frame["lap_current_lap_time"]       = ir["LapCurrentLapTime"]
    frame["lap_last_lap_time"]          = ir["LapLastLapTime"]
    frame["speed"]                      = ir["Speed"]
    frame["rpm"]                        = ir["RPM"]
    frame["gear"]                       = int(ir["Gear"])
    frame["throttle"]                   = ir["Throttle"]
    frame["brake"]                      = ir["Brake"]
    frame["steering_wheel_angle"]       = ir["SteeringWheelAngle"]
    frame["long_accel"]                 = ir["LongAccel"]
    frame["lat_accel"]                  = ir["LatAccel"]
    frame["on_pit_road"]                = bool(ir["OnPitRoad"])
    frame["player_car_position"]        = int(ir["PlayerCarPosition"])
    frame["player_car_class_position"]  = int(ir["PlayerCarClassPosition"])
    frame["is_on_track"]                = bool(ir["IsOnTrack"])

    # --- full-field CarIdx arrays ---
    # iRacing returns these as lists; convert to plain Python types for JSON.
    raw_lap_dist  = ir["CarIdxLapDistPct"] or []
    raw_f2        = ir["CarIdxF2Time"]     or []
    raw_pos       = ir["CarIdxPosition"]   or []
    raw_pit       = ir["CarIdxOnPitRoad"]  or []

    frame["car_idx_lap_dist_pct"] = [float(v) for v in raw_lap_dist]
    frame["car_idx_f2_time"]      = [float(v) for v in raw_f2]
    frame["car_idx_position"]     = [int(v)   for v in raw_pos]
    frame["car_idx_on_pit_road"]  = [bool(v)  for v in raw_pit]

    return frame


def wait_for_iracing(ir: "irsdk.IRSDK", poll_s: float = 1.0) -> None:
    """Block until iRacing is running and the SDK is connected."""
    print("Waiting for iRacing...", end="", flush=True)
    while True:
        if ir.startup() and ir.is_initialized and ir.is_connected:
            print(" connected.")
            return
        time.sleep(poll_s)
        print(".", end="", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Export iRacing replay data to JSONL at a fixed sample rate."
    )
    parser.add_argument(
        "output",
        nargs="?",
        default="session.jsonl",
        help="Output JSONL file path (default: session.jsonl)",
    )
    parser.add_argument(
        "--hz",
        type=float,
        default=5.0,
        help="Sample rate in Hz (default: 5)",
    )
    args = parser.parse_args()

    if args.hz <= 0 or args.hz > 60:
        print("ERROR: --hz must be between 0 and 60.", file=sys.stderr)
        sys.exit(1)

    interval_s = 1.0 / args.hz

    ir = irsdk.IRSDK()
    wait_for_iracing(ir)

    print(f"Sampling at {args.hz} Hz  →  {args.output}")
    print("Press Ctrl-C to stop.\n")

    frame_count = 0
    last_session_time = -1.0

    try:
        with open(args.output, "w", encoding="utf-8") as fh:
            while True:
                # ir.freeze_var_buffer_latest() updates all variables atomically
                if not ir.is_connected:
                    print("\niRacing disconnected.  Stopping.")
                    break

                ir.freeze_var_buffer_latest()

                session_time = ir["SessionTime"]

                # Skip duplicate ticks (iRacing updates at 60 Hz; we sample slower)
                if session_time is None or session_time == last_session_time:
                    time.sleep(interval_s * 0.1)
                    continue

                last_session_time = session_time

                frame = build_frame(ir)
                fh.write(json.dumps(frame) + "\n")
                fh.flush()
                frame_count += 1

                if frame_count % 50 == 0:
                    lap = frame.get("lap", "?")
                    ldp = frame.get("lap_dist_pct", 0)
                    print(
                        f"  {frame_count:6d} frames  |  "
                        f"session_time={session_time:.1f}s  "
                        f"lap={lap}  lap_dist={ldp:.3f}",
                        flush=True,
                    )

                time.sleep(interval_s)

    except KeyboardInterrupt:
        print(f"\nInterrupted.  {frame_count} frames written to {args.output}")

    finally:
        ir.shutdown()
        print(f"Done.  {frame_count} frames  →  {args.output}")


if __name__ == "__main__":
    main()
