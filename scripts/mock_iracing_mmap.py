"""
mock_iracing_mmap.py — publish a synthetic iRacing shared-memory session on Windows.

Creates the same named Windows objects the Rust publisher reads in production:
  - Local\\IRSDKMemMapFileName
  - Local\\IRSDKDataValidEvent

This lets the publisher, UI, and detector stack run unchanged against a
repeatable synthetic weekend that exercises session rollover, battle tracking,
lap completion, overtakes, pit transitions, and flag handling.
"""

from __future__ import annotations

import argparse
import ctypes
import math
import json
import os
import struct
import sys
import time
from dataclasses import dataclass


MMAP_NAME_DEFAULT = "Local\\IRSDKMemMapFileName"
EVENT_NAME_DEFAULT = "Local\\IRSDKDataValidEvent"
PAGE_READWRITE = 0x04
FILE_MAP_ALL_ACCESS = 0x000F001F
INVALID_HANDLE_VALUE = ctypes.c_void_p(-1).value

IR_BOOL = 1
IR_INT = 2
IR_BITFIELD = 3
IR_FLOAT = 4
IR_DOUBLE = 5

HEADER_SIZE = 0x70
VAR_HDR_STRIDE = 144
VAR_BUF_STRIDE = 16
VAR_NAME_OFFSET = 0x10
VAR_NAME_LEN = 32
VAR_DESC_OFFSET = 0x30
VAR_DESC_LEN = 64
VAR_UNIT_OFFSET = 0x70
VAR_UNIT_LEN = 32
MAX_CARS = 64
ERROR_ALREADY_EXISTS = 183


@dataclass(frozen=True)
class VarDef:
    name: str
    type_code: int
    count: int
    description: str
    unit: str = ""


VAR_DEFS = [
    VarDef("SessionTime", IR_DOUBLE, 1, "Elapsed session time", "s"),
    VarDef("SessionFlags", IR_BITFIELD, 1, "Active session flags"),
    VarDef("PlayerCarIdx", IR_INT, 1, "Player car slot index"),
    VarDef("Lap", IR_INT, 1, "Current lap number"),
    VarDef("LapDistPct", IR_FLOAT, 1, "Track position percentage"),
    VarDef("PlayerCarPosition", IR_INT, 1, "Overall race position"),
    VarDef("OnPitRoad", IR_BOOL, 1, "Player on pit road"),
    VarDef("CarIdxLapDistPct", IR_FLOAT, MAX_CARS, "Track position by car"),
    VarDef("CarIdxPosition", IR_INT, MAX_CARS, "Race position by car"),
    VarDef("CarIdxOnPitRoad", IR_BOOL, MAX_CARS, "Pit-road state by car"),
    VarDef("LapLastLapTime", IR_FLOAT, 1, "Previous completed lap time", "s"),
    VarDef("PlayerCarMyIncidentCount", IR_INT, 1, "Player incident count"),
    VarDef("SessionInfoUpdate", IR_INT, 1, "Session info update tick"),
    VarDef("SessionTick", IR_INT, 1, "Session tick counter"),
    VarDef("SessionState", IR_INT, 1, "Session state enum"),
    VarDef("SessionNum", IR_INT, 1, "Active session number"),
    VarDef("CarIdxLapCompleted", IR_INT, MAX_CARS, "Completed laps by car"),
    VarDef("CarIdxTrackSurface", IR_INT, MAX_CARS, "Track surface by car"),
    VarDef("FuelLevel", IR_FLOAT, 1, "Fuel remaining", "L"),
    VarDef("Throttle", IR_FLOAT, 1, "Throttle application"),
    VarDef("Brake", IR_FLOAT, 1, "Brake application"),
    VarDef("Speed", IR_FLOAT, 1, "Vehicle speed", "m/s"),
    VarDef("LFtempM", IR_FLOAT, 1, "Left-front tire temp", "C"),
    VarDef("RFtempM", IR_FLOAT, 1, "Right-front tire temp", "C"),
    VarDef("LRtempM", IR_FLOAT, 1, "Left-rear tire temp", "C"),
    VarDef("RRtempM", IR_FLOAT, 1, "Right-rear tire temp", "C"),
]


def align(value: int, boundary: int) -> int:
    return (value + boundary - 1) // boundary * boundary


def type_size(type_code: int) -> int:
    if type_code == IR_BOOL:
        return 1
    if type_code in (IR_INT, IR_BITFIELD, IR_FLOAT):
        return 4
    if type_code == IR_DOUBLE:
        return 8
    raise ValueError(f"unsupported type code {type_code}")


def pack_c_string(buf: bytearray, offset: int, length: int, text: str) -> None:
    raw = text.encode("ascii", errors="replace")[: length - 1]
    buf[offset : offset + length] = b"\x00" * length
    buf[offset : offset + len(raw)] = raw


def build_layout():
    offsets = {}
    cursor = 0
    for var in VAR_DEFS:
        cursor = align(cursor, min(type_size(var.type_code), 8))
        offsets[var.name] = cursor
        cursor += type_size(var.type_code) * var.count
    return offsets, align(cursor, 8)


def build_driver_roster(player_idx: int = 7, n_cars: int = 24):
    roster = []
    for car_idx in range(n_cars):
        if car_idx == player_idx:
            roster.append(
                {
                    "car_idx": car_idx,
                    "driver_name": "Paul Crofts",
                    "team_name": "Paul Crofts",
                    "car_number": "8",
                    "car_class_id": 4011,
                    "car_class_short_name": "IMSA23",
                    "user_id": 341237,
                }
            )
            continue
        roster.append(
            {
                "car_idx": car_idx,
                "driver_name": f"Mock Driver {car_idx:02d}",
                "team_name": f"Mock Team {car_idx:02d}",
                "car_number": str(car_idx + 1),
                "car_class_id": 4029 if car_idx % 2 == 0 else 4011,
                "car_class_short_name": "GTP" if car_idx % 2 == 0 else "IMSA23",
                "user_id": 900000 + car_idx,
            }
        )
    return roster


def build_session_yaml(roster, sub_session_id: int) -> bytes:
    lines = [
        "WeekendInfo:",
        " TrackDisplayName: Mock Detector Raceway",
        " TrackName: mock_detector_raceway",
        f" SubSessionID: {sub_session_id}",
        "SessionInfo:",
        " Sessions:",
        "  - SessionType: Practice",
        '    SessionLaps: "unlimited"',
        "  - SessionType: Qualify",
        "    SessionLaps: 2",
        "  - SessionType: Race",
        "    SessionLaps: 35",
        "DriverInfo:",
        " DriverCarIdx: 7",
        " Drivers:",
    ]
    for driver in roster:
        lines.extend(
            [
                f"  - CarIdx: {driver['car_idx']}",
                f"    UserName: {driver['driver_name']}",
                f"    UserID: {driver['user_id']}",
                f"    TeamName: {driver['team_name']}",
                f"    CarNumber: \"{driver['car_number']}\"",
                f"    CarClassID: {driver['car_class_id']}",
                f"    CarClassShortName: {driver['car_class_short_name']}",
            ]
        )
    return ("\n".join(lines) + "\n\x00").encode("utf-8")


class WeekendScenario:
    def __init__(self, name: str, rate_hz: int):
        self.name = name
        self.rate_hz = rate_hz
        self.player_idx = 7
        self.target_idx = 6
        self.n_cars = 24
        self.roster = build_driver_roster(self.player_idx, self.n_cars)
        self.sub_session_id = 990001
        self.segments = self._build_segments(name)
        self.total_duration = sum(segment["duration_s"] for segment in self.segments)

    def _build_segments(self, name: str):
        if name == "session-rollover":
            return [
                {"session_num": 0, "duration_s": 45.0, "lap_time_s": 90.0, "position_marks": [22], "gap_marks": [4.0]},
                {"session_num": 1, "duration_s": 30.0, "lap_time_s": 60.0, "position_marks": [22, 22], "gap_marks": [3.0, 2.7]},
                {"session_num": 2, "duration_s": 240.0, "lap_time_s": 60.0, "position_marks": [22, 18, 12, 8, 5], "gap_marks": [4.5, 3.6, 2.4, 0.7, -0.8]},
            ]
        return [
            {"session_num": 0, "duration_s": 36.0, "lap_time_s": 72.0, "position_marks": [22], "gap_marks": [4.6]},
            {"session_num": 1, "duration_s": 48.0, "lap_time_s": 48.0, "position_marks": [22], "gap_marks": [4.2]},
            {"session_num": 2, "duration_s": 360.0, "lap_time_s": 60.0, "position_marks": [22, 18, 12, 8, 5, 4], "gap_marks": [4.5, 4.0, 3.3, 2.2, 0.6, -1.0]},
        ]

    def frame_at(self, global_t: float, global_tick: int):
        elapsed = 0.0
        for seg_idx, segment in enumerate(self.segments):
            seg_end = elapsed + segment["duration_s"]
            if global_t < seg_end or seg_idx == len(self.segments) - 1:
                return self._frame_for_segment(segment, seg_idx, global_t - elapsed, global_tick)
            elapsed = seg_end
        return self._frame_for_segment(self.segments[-1], len(self.segments) - 1, 0.0, global_tick)

    def _frame_for_segment(self, segment, seg_idx: int, local_t: float, global_tick: int):
        lap_time_s = segment["lap_time_s"]
        n_marks = len(segment["position_marks"])
        lap_index = min(int(local_t / lap_time_s), n_marks - 1)
        lap = lap_index + 1
        lap_progress = (local_t % lap_time_s) / lap_time_s if lap_time_s > 0 else 0.0
        player_position = segment["position_marks"][lap_index]
        target_gap_s = segment["gap_marks"][lap_index]

        positions = [0] * MAX_CARS
        active = list(range(self.n_cars))
        target_position = min(self.n_cars, player_position + 1) if target_gap_s < 0 else max(1, player_position - 1)
        positions[self.player_idx] = player_position
        positions[self.target_idx] = target_position

        used_positions = {player_position, target_position}
        remaining_positions = [pos for pos in range(1, self.n_cars + 1) if pos not in used_positions]
        remaining_cars = [car_idx for car_idx in active if car_idx not in (self.player_idx, self.target_idx)]
        for car_idx, pos in zip(remaining_cars, remaining_positions):
            positions[car_idx] = pos

        lap_dist_pct = lap_progress
        lap_dist = [-1.0] * MAX_CARS
        on_pit = [False] * MAX_CARS
        lap_completed = [0] * MAX_CARS
        track_surface = [0] * MAX_CARS

        lap_completed_value = max(0, lap - 1)
        for car_idx in active:
            lap_completed[car_idx] = lap_completed_value
            track_surface[car_idx] = 3

        lap_dist[self.player_idx] = lap_dist_pct
        for car_idx in remaining_cars:
            pos_offset = (player_position - positions[car_idx]) / self.n_cars
            lap_dist[car_idx] = (lap_dist_pct + pos_offset * 0.08) % 1.0

        target_offset = target_gap_s / lap_time_s if lap_time_s > 0 else 0.0
        lap_dist[self.target_idx] = (lap_dist_pct + target_offset) % 1.0

        race_segment = segment["session_num"] == 2
        session_flags = 0x0100 if race_segment and lap == 3 and 0.35 <= lap_progress <= 0.42 else 0
        player_on_pit = race_segment and lap == 5 and 0.08 <= lap_progress <= 0.12
        on_pit[self.player_idx] = player_on_pit
        if player_on_pit:
            track_surface[self.player_idx] = 2

        if race_segment and lap == 5 and 0.10 <= lap_progress <= 0.11:
            positions[self.player_idx] = min(self.n_cars, player_position + 1)

        base_speed = 72.0 + 8.0 * math.sin(lap_progress * math.tau)
        braking_zone = math.sin(lap_progress * math.tau * 4)
        brake = max(0.0, braking_zone)
        throttle = max(0.0, 1.0 - brake * 1.15)
        speed = max(18.0, base_speed - brake * 32.0)
        tire_base = 82.0 + lap * 0.35
        fuel_level = max(5.0, 92.0 - global_tick / self.rate_hz * 0.045)
        session_state = 5 if lap_progress > 0.97 else 4

        return {
            "SessionTime": float(local_t),
            "SessionFlags": int(session_flags),
            "PlayerCarIdx": int(self.player_idx),
            "Lap": int(lap),
            "LapDistPct": float(lap_dist_pct),
            "PlayerCarPosition": int(positions[self.player_idx]),
            "OnPitRoad": bool(player_on_pit),
            "CarIdxLapDistPct": lap_dist,
            "CarIdxPosition": positions,
            "CarIdxOnPitRoad": on_pit,
            "LapLastLapTime": float(lap_time_s if lap > 1 else 0.0),
            "PlayerCarMyIncidentCount": 0,
            "SessionInfoUpdate": int(seg_idx + 1),
            "SessionTick": int(global_tick),
            "SessionState": int(session_state),
            "SessionNum": int(segment["session_num"]),
            "CarIdxLapCompleted": lap_completed,
            "CarIdxTrackSurface": track_surface,
            "FuelLevel": float(fuel_level),
            "Throttle": float(throttle),
            "Brake": float(brake),
            "Speed": float(speed),
            "LFtempM": float(tire_base + brake * 3.2),
            "RFtempM": float(tire_base + brake * 3.5),
            "LRtempM": float(tire_base - 1.1),
            "RRtempM": float(tire_base - 0.8),
        }


class MockIrsdkPublisher:
    def __init__(self, scenario: WeekendScenario, rate_hz: int, mmap_name: str, event_name: str):
        self.scenario = scenario
        self.rate_hz = rate_hz
        self.mmap_name = mmap_name
        self.event_name = event_name
        self.yaml_bytes = build_session_yaml(scenario.roster, scenario.sub_session_id)
        self.var_offsets, self.buf_len = build_layout()
        self.var_header_offset = HEADER_SIZE
        self.session_info_offset = align(self.var_header_offset + len(VAR_DEFS) * VAR_HDR_STRIDE, 8)
        self.buf0_offset = align(self.session_info_offset + len(self.yaml_bytes), 8)
        self.total_size = self.buf0_offset + self.buf_len * 4

        self.k32 = ctypes.windll.kernel32
        self.k32.CreateFileMappingW.restype = ctypes.c_void_p
        self.k32.MapViewOfFile.restype = ctypes.c_void_p
        self.k32.CreateEventW.restype = ctypes.c_void_p
        self.k32.SetEvent.argtypes = [ctypes.c_void_p]
        self.k32.UnmapViewOfFile.argtypes = [ctypes.c_void_p]
        self.k32.CloseHandle.argtypes = [ctypes.c_void_p]

        self.mapping = None
        self.view = None
        self.event = None
        self.memory = None

    def open(self):
        self.mapping = self.k32.CreateFileMappingW(
            ctypes.c_void_p(INVALID_HANDLE_VALUE),
            None,
            PAGE_READWRITE,
            0,
            self.total_size,
            self.mmap_name,
        )
        if not self.mapping:
            raise OSError("CreateFileMappingW failed")
        if self.k32.GetLastError() == ERROR_ALREADY_EXISTS:
            self.k32.CloseHandle(self.mapping)
            self.mapping = None
            raise OSError(
                f"{self.mmap_name} already exists. Choose a unique --mmap-name/--event-name pair, close iRacing, or use --snapshot-out for an offline synthetic export."
            )

        self.view = self.k32.MapViewOfFile(self.mapping, FILE_MAP_ALL_ACCESS, 0, 0, self.total_size)
        if not self.view:
            self.k32.CloseHandle(self.mapping)
            raise OSError("MapViewOfFile failed")

        self.event = self.k32.CreateEventW(None, False, False, self.event_name)
        if not self.event:
            self.k32.UnmapViewOfFile(self.view)
            self.k32.CloseHandle(self.mapping)
            raise OSError("CreateEventW failed")

        self.memory = (ctypes.c_ubyte * self.total_size).from_address(self.view)
        self._initialise_layout()

    def close(self):
        if self.view:
            self.k32.UnmapViewOfFile(self.view)
            self.view = None
        if self.mapping:
            self.k32.CloseHandle(self.mapping)
            self.mapping = None
        if self.event:
            self.k32.CloseHandle(self.event)
            self.event = None

    def _initialise_layout(self):
        buf = self._base_buffer()
        self.memory[:] = buf

    def _base_buffer(self) -> bytearray:
        buf = bytearray(self.total_size)
        struct.pack_into("<i", buf, 0x00, 1)
        struct.pack_into("<i", buf, 0x04, 0x01)
        struct.pack_into("<i", buf, 0x08, self.rate_hz)
        struct.pack_into("<i", buf, 0x0C, 1)
        struct.pack_into("<i", buf, 0x10, len(self.yaml_bytes))
        struct.pack_into("<i", buf, 0x14, self.session_info_offset)
        struct.pack_into("<i", buf, 0x18, len(VAR_DEFS))
        struct.pack_into("<i", buf, 0x1C, self.var_header_offset)
        struct.pack_into("<i", buf, 0x20, 4)
        struct.pack_into("<i", buf, 0x24, self.buf_len)

        for idx in range(4):
            base = 0x30 + idx * VAR_BUF_STRIDE
            struct.pack_into("<i", buf, base, 0)
            struct.pack_into("<i", buf, base + 4, self.buf0_offset + idx * self.buf_len)

        for idx, var in enumerate(VAR_DEFS):
            base = self.var_header_offset + idx * VAR_HDR_STRIDE
            struct.pack_into("<i", buf, base, var.type_code)
            struct.pack_into("<i", buf, base + 4, self.var_offsets[var.name])
            struct.pack_into("<i", buf, base + 8, var.count)
            pack_c_string(buf, base + VAR_NAME_OFFSET, VAR_NAME_LEN, var.name)
            pack_c_string(buf, base + VAR_DESC_OFFSET, VAR_DESC_LEN, var.description)
            pack_c_string(buf, base + VAR_UNIT_OFFSET, VAR_UNIT_LEN, var.unit)

        buf[self.session_info_offset : self.session_info_offset + len(self.yaml_bytes)] = self.yaml_bytes
        return buf

    def _encode_row(self, frame_values: dict) -> bytearray:
        row = bytearray(self.buf_len)
        for var in VAR_DEFS:
            offset = self.var_offsets[var.name]
            value = frame_values[var.name]
            if var.count == 1:
                if var.type_code == IR_BOOL:
                    struct.pack_into("<?", row, offset, bool(value))
                elif var.type_code in (IR_INT, IR_BITFIELD):
                    struct.pack_into("<i", row, offset, int(value))
                elif var.type_code == IR_FLOAT:
                    struct.pack_into("<f", row, offset, float(value))
                elif var.type_code == IR_DOUBLE:
                    struct.pack_into("<d", row, offset, float(value))
                continue

            if len(value) < var.count:
                value = list(value) + ([0] * (var.count - len(value)))
            if var.type_code == IR_BOOL:
                struct.pack_into(f"<{var.count}?", row, offset, *[bool(v) for v in value[: var.count]])
            elif var.type_code in (IR_INT, IR_BITFIELD):
                struct.pack_into(f"<{var.count}i", row, offset, *[int(v) for v in value[: var.count]])
            elif var.type_code == IR_FLOAT:
                struct.pack_into(f"<{var.count}f", row, offset, *[float(v) for v in value[: var.count]])
        return row

    def build_snapshot_blob(self, frame_number: int, frame_values: dict) -> bytearray:
        buf = self._base_buffer()
        buf_index = frame_number % 4
        buf_offset = self.buf0_offset + buf_index * self.buf_len
        row = self._encode_row(frame_values)
        buf[buf_offset : buf_offset + self.buf_len] = row
        var_buf_base = 0x30 + buf_index * VAR_BUF_STRIDE
        struct.pack_into("<i", buf, var_buf_base, frame_number)
        struct.pack_into("<i", buf, 0x0C, int(frame_values["SessionInfoUpdate"]))
        return buf

    def write_snapshot(self, output_path: str, manifest_path: str | None, frame_number: int, frame_values: dict) -> None:
        blob = self.build_snapshot_blob(frame_number, frame_values)
        os.makedirs(os.path.dirname(os.path.abspath(output_path)), exist_ok=True)
        with open(output_path, "wb") as fh:
            fh.write(blob)
        if manifest_path:
            manifest = {
                "scenario": self.scenario.name,
                "frameNumber": frame_number,
                "sessionNum": frame_values["SessionNum"],
                "sessionTime": frame_values["SessionTime"],
                "lap": frame_values["Lap"],
                "playerPosition": frame_values["PlayerCarPosition"],
                "header": {
                    "numVars": len(VAR_DEFS),
                    "varHeaderOffset": self.var_header_offset,
                    "sessionInfoOffset": self.session_info_offset,
                    "bufLen": self.buf_len,
                },
                "yamlPreview": self.yaml_bytes.decode("utf-8", errors="replace").splitlines()[:40],
                "vars": [
                    {
                        "name": var.name,
                        "offset": self.var_offsets[var.name],
                        "count": var.count,
                        "type": var.type_code,
                    }
                    for var in VAR_DEFS
                ],
            }
            os.makedirs(os.path.dirname(os.path.abspath(manifest_path)), exist_ok=True)
            with open(manifest_path, "w", encoding="utf-8") as fh:
                json.dump(manifest, fh, indent=2)

    def write_frame(self, frame_number: int, frame_values: dict):
        buf_index = frame_number % 4
        buf_offset = self.buf0_offset + buf_index * self.buf_len
        row = self._encode_row(frame_values)

        self.memory[buf_offset : buf_offset + self.buf_len] = row
        var_buf_base = 0x30 + buf_index * VAR_BUF_STRIDE
        struct.pack_into("<i", self.memory, var_buf_base, frame_number)
        struct.pack_into("<i", self.memory, 0x0C, int(frame_values["SessionInfoUpdate"]))
        self.k32.SetEvent(self.event)


def run(args):
    scenario = WeekendScenario(args.scenario, args.rate_hz)
    publisher = MockIrsdkPublisher(scenario, args.rate_hz, args.mmap_name, args.event_name)

    if args.snapshot_out:
        dt = 1.0 / args.rate_hz
        total_frames = int(math.ceil(scenario.total_duration * args.rate_hz))
        frame_number = total_frames if args.snapshot_frame < 0 else max(1, min(args.snapshot_frame, total_frames))
        frame = scenario.frame_at((frame_number - 1) * dt, frame_number - 1)
        publisher.write_snapshot(args.snapshot_out, args.manifest_out, frame_number, frame)
        print(f"[mock-mmap] wrote synthetic snapshot to {args.snapshot_out}")
        if args.manifest_out:
            print(f"[mock-mmap] wrote synthetic manifest to {args.manifest_out}")
        if args.snapshot_only:
            return 0

    if sys.platform != "win32":
        print("ERROR: mock_iracing_mmap.py is Windows-only.", file=sys.stderr)
        return 1
    publisher.open()

    dt = 1.0 / args.rate_hz
    sleep_dt = dt / max(args.time_scale, 0.1)
    total_frames = int(math.ceil(scenario.total_duration * args.rate_hz))

    print(f"[mock-mmap] publishing {args.scenario} to {args.mmap_name} @ {args.rate_hz} Hz")
    print(f"[mock-mmap] event {args.event_name}")
    print(f"[mock-mmap] total scenario duration {scenario.total_duration:.1f}s ({total_frames} frames)")
    print("[mock-mmap] start the Rust publisher now; stop with Ctrl-C")

    try:
        while True:
            for frame_number in range(total_frames):
                global_t = frame_number * dt
                frame = scenario.frame_at(global_t, frame_number)
                publisher.write_frame(frame_number + 1, frame)
                time.sleep(sleep_dt)
            if not args.loop:
                while True:
                    final_frame = scenario.frame_at(scenario.total_duration - dt, total_frames)
                    publisher.write_frame(total_frames + 1, final_frame)
                    time.sleep(1.0)
            print("[mock-mmap] looping scenario")
    except KeyboardInterrupt:
        print("\n[mock-mmap] stopped")
    finally:
        publisher.close()
    return 0


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Publish a synthetic iRacing shared-memory session")
    parser.add_argument("--scenario", choices=["detector-smoke", "session-rollover"], default="detector-smoke")
    parser.add_argument("--rate-hz", type=int, default=60, help="Frame rate for SetEvent updates")
    parser.add_argument("--time-scale", type=float, default=1.0, help="Playback speed multiplier (sim-time / wall-time)")
    parser.add_argument("--mmap-name", default=MMAP_NAME_DEFAULT, help="Windows mapping name")
    parser.add_argument("--event-name", default=EVENT_NAME_DEFAULT, help="Windows event name")
    parser.add_argument("--loop", action="store_true", help="Replay the scenario continuously")
    parser.add_argument("--snapshot-out", help="Write a synthetic mmap snapshot to this file")
    parser.add_argument("--manifest-out", help="Write synthetic snapshot metadata to JSON")
    parser.add_argument("--snapshot-frame", type=int, default=-1, help="Frame number to export for --snapshot-out (-1 = last frame)")
    parser.add_argument("--snapshot-only", action="store_true", help="Write the synthetic snapshot and exit without publishing the live mock")
    sys.exit(run(parser.parse_args()))
    total_size: int


def align(value: int, alignment: int) -> int:
    if alignment <= 1:
        return value
    return (value + alignment - 1) // alignment * alignment


def scalar_size(type_code: int) -> int:
    if type_code in (IR_BOOL,):
        return 1
    if type_code in (IR_INT, IR_BITFIELD, IR_FLOAT):
        return 4
    if type_code == IR_DOUBLE:
        return 8
    raise ValueError(f"unsupported type {type_code}")


def build_var_defs() -> list[VarDef]:
    return [
        VarDef("SessionTime", IR_DOUBLE, 1, "s"),
        VarDef("SessionFlags", IR_BITFIELD, 1),
        VarDef("PlayerCarIdx", IR_INT, 1),
        VarDef("Lap", IR_INT, 1),
        VarDef("LapDistPct", IR_FLOAT, 1),
        VarDef("PlayerCarPosition", IR_INT, 1),
        VarDef("OnPitRoad", IR_BOOL, 1),
        VarDef("CarIdxLapDistPct", IR_FLOAT, CAR_SLOTS),
        VarDef("CarIdxPosition", IR_INT, CAR_SLOTS),
        VarDef("CarIdxOnPitRoad", IR_BOOL, CAR_SLOTS),
        VarDef("CarIdxTrackSurface", IR_INT, CAR_SLOTS),
        VarDef("LapLastLapTime", IR_FLOAT, 1, "s"),
        VarDef("SessionInfoUpdate", IR_INT, 1),
        VarDef("SessionTick", IR_INT, 1),
        VarDef("SessionState", IR_INT, 1),
        VarDef("SessionNum", IR_INT, 1),
        VarDef("CarIdxLapCompleted", IR_INT, CAR_SLOTS),
        VarDef("FuelLevel", IR_FLOAT, 1, "L"),
        VarDef("Throttle", IR_FLOAT, 1),
        VarDef("Brake", IR_FLOAT, 1),
        VarDef("Speed", IR_FLOAT, 1, "m/s"),
        VarDef("LFtempM", IR_FLOAT, 1, "C"),
        VarDef("RFtempM", IR_FLOAT, 1, "C"),
        VarDef("LRtempM", IR_FLOAT, 1, "C"),
        VarDef("RRtempM", IR_FLOAT, 1, "C"),
    ]


def build_layout(yaml_text: str) -> Layout:
    var_defs = build_var_defs()
    offsets: dict[str, int] = {}
    buf_len = 0
    for var in var_defs:
        item_size = scalar_size(var.type_code)
        buf_len = align(buf_len, min(item_size, 8))
        offsets[var.name] = buf_len
        buf_len += item_size * var.count
    buf_len = align(buf_len, 8)

    var_header_offset = align(HEADER_SIZE, 16)
    session_info_offset = align(var_header_offset + len(var_defs) * VAR_HDR_STRIDE, 16)
    session_info_reserved = align(len(yaml_text.encode("utf-8")) + 1, 256)
    first_buf_offset = align(session_info_offset + session_info_reserved, 16)
    buf_offsets = [first_buf_offset + i * buf_len for i in range(4)]
    total_size = buf_offsets[-1] + buf_len

    return Layout(
        var_defs=var_defs,
        offsets=offsets,
        buf_len=buf_len,
        var_header_offset=var_header_offset,
        session_info_offset=session_info_offset,
        session_info_reserved=session_info_reserved,
        buf_offsets=buf_offsets,
        total_size=total_size,
    )


def pack_var_header(buf: bytearray, entry_off: int, var: VarDef, var_off: int) -> None:
    struct.pack_into("<iii", buf, entry_off, var.type_code, var_off, var.count)
    name = var.name.encode("ascii")[:31]
    unit = var.unit.encode("ascii")[:31]
    buf[entry_off + 0x10 : entry_off + 0x10 + len(name)] = name
    buf[entry_off + 0x70 : entry_off + 0x70 + len(unit)] = unit


def write_scalar(buf: bytearray, off: int, type_code: int, value) -> None:
    if type_code == IR_BOOL:
        buf[off] = 1 if value else 0
    elif type_code in (IR_INT, IR_BITFIELD):
        struct.pack_into("<i", buf, off, int(value))
    elif type_code == IR_FLOAT:
        struct.pack_into("<f", buf, off, float(value))
    elif type_code == IR_DOUBLE:
        struct.pack_into("<d", buf, off, float(value))
    else:
        raise ValueError(f"unsupported type {type_code}")


def write_array(buf: bytearray, off: int, type_code: int, values: list) -> None:
    if type_code == IR_BOOL:
        for i, value in enumerate(values):
            buf[off + i] = 1 if value else 0
        return
    if type_code in (IR_INT, IR_BITFIELD):
        fmt = "<" + "i" * len(values)
        struct.pack_into(fmt, buf, off, *[int(v) for v in values])
        return
    if type_code == IR_FLOAT:
        fmt = "<" + "f" * len(values)
        struct.pack_into(fmt, buf, off, *[float(v) for v in values])
        return
    raise ValueError(f"unsupported array type {type_code}")


def build_roster() -> list[dict]:
    roster = []
    for car_idx in range(24):
        if car_idx == PLAYER_IDX:
            roster.append(
                {
                    "CarIdx": car_idx,
                    "UserName": "Paul Crofts",
                    "UserID": 341237,
                    "TeamName": "Paul Crofts",
                    "CarNumber": "8",
                    "CarClassID": 4011,
                    "CarClassShortName": "IMSA23",
                }
            )
            continue
        roster.append(
            {
                "CarIdx": car_idx,
                "UserName": f"Mock Driver {car_idx:02d}",
                "UserID": 900000 + car_idx,
                "TeamName": f"Mock Team {car_idx:02d}",
                "CarNumber": str(car_idx + 1),
                "CarClassID": 4029 if car_idx < 10 else 4011,
                "CarClassShortName": "GTP" if car_idx < 10 else "IMSA23",
            }
        )
    roster[0]["UserName"] = "Ansis Law"
    roster[0]["TeamName"] = "Ansis Law"
    roster[0]["CarNumber"] = "1"
    return roster


def build_session_yaml(roster: list[dict]) -> str:
    lines = [
        "WeekendInfo:",
        " TrackDisplayName: Mock Nürburgring Combined",
        f" SubSessionID: {SUB_SESSION_ID}",
        "SessionInfo:",
        " Sessions:",
        " - SessionType: Practice",
        "   SessionLaps: unlimited",
        " - SessionType: Qualify",
        "   SessionLaps: 2",
        " - SessionType: Race",
        "   SessionLaps: 35",
        "DriverInfo:",
        f" DriverCarIdx: {PLAYER_IDX}",
        " Drivers:",
    ]
    for car in roster:
        lines.extend(
            [
                f" - CarIdx: {car['CarIdx']}",
                f"   UserName: {car['UserName']}",
                f"   UserID: {car['UserID']}",
                f"   TeamName: {json.dumps(car['TeamName'])}",
                f"   CarNumber: {json.dumps(car['CarNumber'])}",
                f"   CarClassID: {car['CarClassID']}",
                f"   CarClassShortName: {car['CarClassShortName']}",
            ]
        )
    return "\n".join(lines) + "\n"


def clamp(value: float, lo: float, hi: float) -> float:
    return max(lo, min(hi, value))


def build_positions(player_pos: int, target_pos: int, target_idx: int, total_cars: int = 24) -> list[int]:
    positions = [0] * CAR_SLOTS
    used_positions = {player_pos, target_pos}
    positions[PLAYER_IDX] = player_pos
    positions[target_idx] = target_pos
    next_positions = [p for p in range(1, total_cars + 1) if p not in used_positions]
    write_at = 0
    for car_idx in range(total_cars):
        if car_idx in (PLAYER_IDX, target_idx):
            continue
        positions[car_idx] = next_positions[write_at]
        write_at += 1
    return positions


def stepped_position(start_pos: int, end_pos: int, lap_dist_pct: float) -> int:
    if end_pos >= start_pos:
        return start_pos
    gain = start_pos - end_pos
    if lap_dist_pct < 0.35:
        return start_pos
    progress = clamp((lap_dist_pct - 0.35) / 0.5, 0.0, 1.0)
    moved = min(gain, int(math.floor(progress * gain + 1e-6)))
    return start_pos - moved


def build_car_ldp(
    player_ldp: float,
    player_pos: int,
    positions: list[int],
    lap_time_s: float,
    target_idx: int,
    target_gap_s: float,
    defender_idx: int | None,
    defender_gap_s: float | None,
) -> list[float]:
    ldp = [-1.0] * CAR_SLOTS
    ldp[PLAYER_IDX] = player_ldp

    for car_idx, pos in enumerate(positions):
        if pos <= 0 or car_idx == PLAYER_IDX:
            continue
        if car_idx == target_idx:
            ldp[car_idx] = (player_ldp + target_gap_s / lap_time_s) % 1.0
            continue
        if defender_idx is not None and car_idx == defender_idx and defender_gap_s is not None:
            ldp[car_idx] = (player_ldp - defender_gap_s / lap_time_s) % 1.0
            continue
        if pos < player_pos:
            gap_s = 8.0 + 0.9 * (player_pos - pos)
            ldp[car_idx] = (player_ldp + gap_s / lap_time_s) % 1.0
        else:
            gap_s = 8.0 + 0.8 * (pos - player_pos)
            ldp[car_idx] = (player_ldp - gap_s / lap_time_s) % 1.0
    return ldp


def build_track_surface(positions: list[int], on_pit: bool) -> list[int]:
    surfaces = [0] * CAR_SLOTS
    for i, pos in enumerate(positions):
        if pos > 0:
            surfaces[i] = 1
    if on_pit:
        surfaces[PLAYER_IDX] = 2
    return surfaces


def official_weekend_frames(hz: int) -> list[dict]:
    dt = 1.0 / hz
    frames: list[dict] = []
    global_tick = 1
    session_info_update = 1

    practice_duration = 45.0
    qualify_duration = 80.0
    race_lap_time = 82.0
    race_laps = 7
    race_duration = race_lap_time * race_laps

    race_positions_after_lap = {
        0: 22,
        1: 18,
        2: 12,
        3: 8,
        4: 5,
        5: 5,
        6: 4,
        7: 4,
    }
    target_gap_by_lap = {
        1: 4.8,
        2: 4.1,
        3: 3.2,
        4: 2.1,
        5: 1.0,
        6: 0.35,
        7: -0.9,
    }
    defender_gap_by_lap = {
        5: 1.4,
        6: 1.0,
        7: 1.1,
    }
    target_idx = 0
    defender_idx = 12

    def emit_segment(session_num: int, session_type: str, duration: float, lap_time: float, position: int, info_tick: int) -> None:
        nonlocal global_tick
        frame_count = max(1, int(duration * hz))
        for i in range(frame_count):
            session_time = i * dt
            lap = max(1, int(session_time / lap_time) + 1)
            lap_dist_pct = (session_time % lap_time) / lap_time
            player_pos = position
            target_pos = max(1, player_pos - 1)
            positions = build_positions(player_pos, target_pos, target_idx)
            car_ldp = build_car_ldp(lap_dist_pct, player_pos, positions, lap_time, target_idx, 7.0, None, None)
            session_state = 5 if i >= frame_count - hz // 2 else 4
            frame = {
                "segment": session_type,
                "session_num": session_num,
                "session_time": session_time,
                "lap": lap,
                "lap_dist_pct": lap_dist_pct,
                "player_pos": player_pos,
                "positions": positions,
                "car_idx_lap_dist_pct": car_ldp,
                "car_idx_on_pit_road": [False] * CAR_SLOTS,
                "car_idx_track_surface": build_track_surface(positions, False),
                "car_idx_lap_completed": [max(0, lap - 1) if p > 0 else 0 for p in positions],
                "session_flags": 0,
                "session_state": session_state,
                "session_tick": global_tick,
                "session_info_update": info_tick,
                "lap_last_lap_time": lap_time if lap > 1 else 0.0,
                "fuel_level": 90.0,
                "throttle": clamp(0.8 + 0.15 * math.sin(lap_dist_pct * math.pi * 4), 0.0, 1.0),
                "brake": clamp(0.5 if 0.18 <= lap_dist_pct <= 0.22 else 0.0, 0.0, 1.0),
                "speed": 78.0 + 22.0 * math.sin(lap_dist_pct * math.pi * 2),
                "temps": [78.0, 79.0, 82.0, 81.0],
            }
            frames.append(frame)
            global_tick += 1

    emit_segment(0, "Practice", practice_duration, 70.0, 22, session_info_update)
    session_info_update += 1
    emit_segment(1, "Qualify", qualify_duration, 40.0, 22, session_info_update)
    session_info_update += 1

    race_frame_count = int(race_duration * hz)
    for i in range(race_frame_count):
        session_time = i * dt
        lap_index = min(race_laps - 1, int(session_time / race_lap_time))
        lap = lap_index + 1
        lap_dist_pct = (session_time % race_lap_time) / race_lap_time

        start_pos = race_positions_after_lap[lap_index]
        end_pos = race_positions_after_lap[lap_index + 1]
        player_pos = stepped_position(start_pos, end_pos, lap_dist_pct)

        target_gap = target_gap_by_lap[lap]
        player_ahead = target_gap < 0 and lap_dist_pct > 0.45
        target_pos = player_pos + 1 if player_ahead else max(1, player_pos - 1)

        positions = build_positions(player_pos, target_pos, target_idx)
        defender_gap = defender_gap_by_lap.get(lap)
        if defender_gap is not None:
            positions[defender_idx] = min(24, player_pos + 1)

        car_ldp = build_car_ldp(
            lap_dist_pct,
            player_pos,
            positions,
            race_lap_time,
            target_idx,
            target_gap,
            defender_idx if defender_gap is not None else None,
            defender_gap,
        )

        on_pit = lap == 5 and 0.08 <= lap_dist_pct <= 0.12
        flags = YELLOW_WAVE if lap == 3 and 0.44 <= lap_dist_pct <= 0.52 else 0
        session_state = 5 if i >= race_frame_count - hz else 4
        fuel_level = max(10.0, 82.0 - session_time * 0.11)
        brake = 0.75 if lap_dist_pct in [lap_dist_pct] and (0.19 <= lap_dist_pct <= 0.23 or 0.61 <= lap_dist_pct <= 0.67) else 0.0
        if brake == 0.0 and 0.79 <= lap_dist_pct <= 0.84:
            brake = 0.55
        throttle = 0.95 - brake * 0.9
        speed = 64.0 + 58.0 * max(0.2, math.sin((lap_dist_pct + 0.08) * math.pi) ** 2)
        temp_base = 84.0 + 0.03 * session_time

        frame = {
            "segment": "Race",
            "session_num": 2,
            "session_time": session_time,
            "lap": lap,
            "lap_dist_pct": lap_dist_pct,
            "player_pos": player_pos,
            "positions": positions,
            "car_idx_lap_dist_pct": car_ldp,
            "car_idx_on_pit_road": [False] * CAR_SLOTS,
            "car_idx_track_surface": build_track_surface(positions, on_pit),
            "car_idx_lap_completed": [max(0, lap - 1) if p > 0 else 0 for p in positions],
            "session_flags": flags,
            "session_state": session_state,
            "session_tick": global_tick,
            "session_info_update": session_info_update,
            "lap_last_lap_time": race_lap_time if lap > 1 else 0.0,
            "fuel_level": fuel_level,
            "throttle": clamp(throttle, 0.0, 1.0),
            "brake": clamp(brake, 0.0, 1.0),
            "speed": speed,
            "temps": [temp_base + 1.0, temp_base + 1.5, temp_base + 2.0, temp_base + 1.8],
        }
        if on_pit:
            frame["car_idx_on_pit_road"][PLAYER_IDX] = True
        frames.append(frame)
        global_tick += 1

    return frames


def scenario_frames(name: str, hz: int) -> list[dict]:
    if name != "official_weekend":
        raise ValueError(f"unknown scenario {name}")
    return official_weekend_frames(hz)


def build_mmap_bytes(layout: Layout, yaml_text: str, frame: dict, buf_slot: int) -> bytearray:
    buf = bytearray(layout.total_size)

    struct.pack_into("<i", buf, 0x00, 1)
    struct.pack_into("<i", buf, 0x04, 0x01)
    struct.pack_into("<i", buf, 0x08, 60)
    struct.pack_into("<i", buf, 0x0C, int(frame["session_info_update"]))
    struct.pack_into("<i", buf, 0x10, len(yaml_text.encode("utf-8")) + 1)
    struct.pack_into("<i", buf, 0x14, layout.session_info_offset)
    struct.pack_into("<i", buf, 0x18, len(layout.var_defs))
    struct.pack_into("<i", buf, 0x1C, layout.var_header_offset)
    struct.pack_into("<i", buf, 0x20, 4)
    struct.pack_into("<i", buf, 0x24, layout.buf_len)

    for i, var in enumerate(layout.var_defs):
        pack_var_header(buf, layout.var_header_offset + i * VAR_HDR_STRIDE, var, layout.offsets[var.name])

    yaml_bytes = yaml_text.encode("utf-8") + b"\x00"
    buf[layout.session_info_offset : layout.session_info_offset + len(yaml_bytes)] = yaml_bytes

    for i, buf_off in enumerate(layout.buf_offsets):
        tick = int(frame["session_tick"]) if i == buf_slot else max(0, int(frame["session_tick"]) - (buf_slot - i) % 4 - 1)
        struct.pack_into("<i", buf, HDR_VAR_BUF + i * VAR_BUF_STRIDE, tick)
        struct.pack_into("<i", buf, HDR_VAR_BUF + i * VAR_BUF_STRIDE + 4, buf_off)

    row_off = layout.buf_offsets[buf_slot]

    write_scalar(buf, row_off + layout.offsets["SessionTime"], IR_DOUBLE, frame["session_time"])
    write_scalar(buf, row_off + layout.offsets["SessionFlags"], IR_BITFIELD, frame["session_flags"])
    write_scalar(buf, row_off + layout.offsets["PlayerCarIdx"], IR_INT, PLAYER_IDX)
    write_scalar(buf, row_off + layout.offsets["Lap"], IR_INT, frame["lap"])
    write_scalar(buf, row_off + layout.offsets["LapDistPct"], IR_FLOAT, frame["lap_dist_pct"])
    write_scalar(buf, row_off + layout.offsets["PlayerCarPosition"], IR_INT, frame["player_pos"])
    write_scalar(buf, row_off + layout.offsets["OnPitRoad"], IR_BOOL, frame["car_idx_on_pit_road"][PLAYER_IDX])
    write_array(buf, row_off + layout.offsets["CarIdxLapDistPct"], IR_FLOAT, frame["car_idx_lap_dist_pct"])
    write_array(buf, row_off + layout.offsets["CarIdxPosition"], IR_INT, frame["positions"])
    write_array(buf, row_off + layout.offsets["CarIdxOnPitRoad"], IR_BOOL, frame["car_idx_on_pit_road"])
    write_array(buf, row_off + layout.offsets["CarIdxTrackSurface"], IR_INT, frame["car_idx_track_surface"])
    write_scalar(buf, row_off + layout.offsets["LapLastLapTime"], IR_FLOAT, frame["lap_last_lap_time"])
    write_scalar(buf, row_off + layout.offsets["SessionInfoUpdate"], IR_INT, frame["session_info_update"])
    write_scalar(buf, row_off + layout.offsets["SessionTick"], IR_INT, frame["session_tick"])
    write_scalar(buf, row_off + layout.offsets["SessionState"], IR_INT, frame["session_state"])
    write_scalar(buf, row_off + layout.offsets["SessionNum"], IR_INT, frame["session_num"])
    write_array(buf, row_off + layout.offsets["CarIdxLapCompleted"], IR_INT, frame["car_idx_lap_completed"])
    write_scalar(buf, row_off + layout.offsets["FuelLevel"], IR_FLOAT, frame["fuel_level"])
    write_scalar(buf, row_off + layout.offsets["Throttle"], IR_FLOAT, frame["throttle"])
    write_scalar(buf, row_off + layout.offsets["Brake"], IR_FLOAT, frame["brake"])
    write_scalar(buf, row_off + layout.offsets["Speed"], IR_FLOAT, frame["speed"])
    write_scalar(buf, row_off + layout.offsets["LFtempM"], IR_FLOAT, frame["temps"][0])
    write_scalar(buf, row_off + layout.offsets["RFtempM"], IR_FLOAT, frame["temps"][1])
    write_scalar(buf, row_off + layout.offsets["LRtempM"], IR_FLOAT, frame["temps"][2])
    write_scalar(buf, row_off + layout.offsets["RRtempM"], IR_FLOAT, frame["temps"][3])

    return buf


def snapshot_manifest(layout: Layout, frame: dict, yaml_text: str) -> dict:
    return {
        "subSessionId": SUB_SESSION_ID,
        "scenario": "official_weekend",
        "sessionNum": frame["session_num"],
        "segment": frame["segment"],
        "sessionTime": round(frame["session_time"], 3),
        "lap": frame["lap"],
        "playerPosition": frame["player_pos"],
        "sessionInfoUpdate": frame["session_info_update"],
        "sessionTick": frame["session_tick"],
        "layout": {
            "bufLen": layout.buf_len,
            "numVars": len(layout.var_defs),
            "sessionInfoOffset": layout.session_info_offset,
            "varHeaderOffset": layout.var_header_offset,
            "totalSize": layout.total_size,
        },
        "vars": [
            {"name": var.name, "offset": layout.offsets[var.name], "count": var.count, "type": var.type_code}
            for var in layout.var_defs
        ],
        "yamlPreview": yaml_text.splitlines()[:24],
    }


class WinMmapPublisher:
    def __init__(self, size: int, mmap_name: str, event_name: str):
        self.size = size
        self.mmap_name = mmap_name
        self.event_name = event_name
        self.k32 = ctypes.windll.kernel32
        self.mapping = None
        self.view = None
        self.event = None

    def __enter__(self):
        self.k32.CreateFileMappingW.restype = ctypes.c_void_p
        self.k32.MapViewOfFile.restype = ctypes.c_void_p
        self.k32.CreateEventW.restype = ctypes.c_void_p

        self.mapping = self.k32.CreateFileMappingW(
            ctypes.c_void_p(INVALID_HANDLE_VALUE),
            None,
            PAGE_READWRITE,
            0,
            self.size,
            self.mmap_name,
        )
        if not self.mapping:
            raise OSError("CreateFileMappingW failed")

        self.view = self.k32.MapViewOfFile(self.mapping, FILE_MAP_ALL_ACCESS, 0, 0, self.size)
        if not self.view:
            self.k32.CloseHandle(self.mapping)
            raise OSError("MapViewOfFile failed")

        self.event = self.k32.CreateEventW(None, False, False, self.event_name)
        if not self.event:
            self.k32.UnmapViewOfFile(self.view)
            self.k32.CloseHandle(self.mapping)
            raise OSError("CreateEventW failed")
        return self

    def write(self, blob: bytes) -> None:
        if len(blob) > self.size:
            raise ValueError("blob larger than mapping")
        ctypes.memmove(self.view, bytes(blob), len(blob))

    def signal(self) -> None:
        self.k32.SetEvent(self.event)

    def __exit__(self, exc_type, exc, tb):
        if self.view:
            self.k32.UnmapViewOfFile(self.view)
        if self.event:
            self.k32.CloseHandle(self.event)
        if self.mapping:
            self.k32.CloseHandle(self.mapping)


def write_snapshot(path: str, manifest_path: str | None, blob: bytes, layout: Layout, frame: dict, yaml_text: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as fh:
        fh.write(blob)
    if manifest_path:
        os.makedirs(os.path.dirname(manifest_path), exist_ok=True)
        with open(manifest_path, "w", encoding="utf-8") as fh:
            json.dump(snapshot_manifest(layout, frame, yaml_text), fh, indent=2)


def run_publish(frames: list[dict], layout: Layout, yaml_text: str, speed: float, loop: bool) -> None:
    if os.name != "nt":
        raise SystemExit("mock_iracing_mmap.py is Windows-only")
    dt = 1.0 / 60.0
    frame_period = dt / max(speed, 0.1)
    with WinMmapPublisher(layout.total_size, MMAP_NAME, EVENT_NAME) as publisher:
        print(f"[mock-mmap] publishing {len(frames)} frames to {MMAP_NAME}")
        print(f"[mock-mmap] event name: {EVENT_NAME}")
        buf_slot = 0
        try:
            while True:
                for idx, frame in enumerate(frames):
                    blob = build_mmap_bytes(layout, yaml_text, frame, buf_slot)
                    publisher.write(blob)
                    publisher.signal()
                    if idx == 0 or frame["session_time"] == 0.0:
                        print(
                            f"[mock-mmap] segment={frame['segment']} session_num={frame['session_num']} "
                            f"tick={frame['session_tick']} pos=P{frame['player_pos']}"
                        )
                    buf_slot = (buf_slot + 1) % 4
                    time.sleep(frame_period)
                if not loop:
                    break
        except KeyboardInterrupt:
            print("\n[mock-mmap] stopped")


def main() -> None:
    parser = argparse.ArgumentParser(description="Publish a synthetic iRacing shared-memory feed")
    parser.add_argument("--scenario", default="official_weekend", choices=["official_weekend"])
    parser.add_argument("--hz", type=int, default=60, help="Synthetic frame rate")
    parser.add_argument("--publish", action="store_true", help="Publish the named mmap and event")
    parser.add_argument("--speed", type=float, default=8.0, help="Playback multiplier when publishing")
    parser.add_argument("--loop", action="store_true", help="Loop the scenario when publishing")
    parser.add_argument("--snapshot-out", help="Write one synthetic mmap snapshot to this path")
    parser.add_argument("--manifest-out", help="Write JSON metadata alongside --snapshot-out")
    parser.add_argument("--snapshot-frame", type=int, default=-1, help="Frame index to export for snapshot (-1 = last)")
    args = parser.parse_args()

    roster = build_roster()
    yaml_text = build_session_yaml(roster)
    layout = build_layout(yaml_text)
    frames = scenario_frames(args.scenario, args.hz)
    snapshot_index = args.snapshot_frame if args.snapshot_frame >= 0 else len(frames) - 1
    snapshot_index = max(0, min(snapshot_index, len(frames) - 1))
    snapshot_frame = frames[snapshot_index]
    snapshot_blob = build_mmap_bytes(layout, yaml_text, snapshot_frame, 0)

    if args.snapshot_out:
        write_snapshot(args.snapshot_out, args.manifest_out, snapshot_blob, layout, snapshot_frame, yaml_text)
        print(f"[mock-mmap] wrote snapshot: {args.snapshot_out}")
        if args.manifest_out:
            print(f"[mock-mmap] wrote manifest: {args.manifest_out}")

    if args.publish:
        run_publish(frames, layout, yaml_text, args.speed, args.loop)
    elif not args.snapshot_out:
        parser.error("choose at least one output mode: --publish or --snapshot-out")


if __name__ == "__main__":
    main()