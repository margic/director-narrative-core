"""
iracing_mmap_diag.py — Diagnostic reader for iRacing shared memory.

Reads the iRacing SDK mmap directly and prints:
  - Connection status & header fields
  - SessionInfo YAML snippet around SubSessionID / WeekendInfo
  - Live telemetry values: SessionTime, SessionState, SessionNum,
    SessionTick, SessionInfoUpdate, PlayerCarIdx, Lap

Usage:
    python scripts/iracing_mmap_diag.py
    python scripts/iracing_mmap_diag.py --yaml        # dump full SessionInfo YAML
    python scripts/iracing_mmap_diag.py --watch        # refresh every second
"""

import argparse
import ctypes
import struct
import sys
import time

MMAP_NAME      = "IRSDKMemMapFileName"
MMAP_SIZE      = 2 * 1024 * 1024  # 2 MiB — large enough for any SDK version
HDR_STATUS_OFF = 0x04
HDR_SI_UPDATE  = 0x0C
HDR_SI_LEN     = 0x10
HDR_SI_OFFSET  = 0x14
HDR_NUM_VARS   = 0x18
HDR_VAR_HDR_OFF= 0x1C
HDR_NUM_BUF    = 0x20
HDR_BUF_LEN    = 0x24
HDR_VAR_BUF    = 0x30   # array of 4 × VarBuf (16 bytes each)

VAR_BUF_STRIDE = 16
VAR_HDR_STRIDE = 144
VAR_HDR_NAME_OFF = 0x10
VAR_HDR_NAME_LEN = 32

# irsdk type codes
IR_BOOL     = 1
IR_INT      = 2
IR_BITFIELD = 3
IR_FLOAT    = 4
IR_DOUBLE   = 5

INTERESTING_VARS = {
    "SessionTime", "SessionTick", "SessionState", "SessionNum",
    "SessionInfoUpdate", "PlayerCarIdx", "Lap", "SessionFlags",
}


def ri32(buf, off):
    return struct.unpack_from("<i", buf, off)[0]


def open_mmap():
    k32 = ctypes.windll.kernel32
    k32.OpenFileMappingA.restype  = ctypes.c_void_p
    k32.MapViewOfFile.restype     = ctypes.c_void_p
    k32.UnmapViewOfFile.argtypes  = [ctypes.c_void_p]
    k32.CloseHandle.argtypes      = [ctypes.c_void_p]
    FILE_MAP_READ = 0x0004

    h = k32.OpenFileMappingA(FILE_MAP_READ, False, MMAP_NAME.encode())
    if not h:
        return None, None

    # Map the full file (size=0) to read the header first.
    ptr = k32.MapViewOfFile(h, FILE_MAP_READ, 0, 0, 0)
    if not ptr:
        k32.CloseHandle(h)
        return None, None

    # Read enough to parse the header + VarBuf array (256 bytes is plenty).
    hdr_bytes = ctypes.string_at(ptr, 256)

    num_buf = struct.unpack_from("<i", hdr_bytes, HDR_NUM_BUF)[0]
    buf_len = struct.unpack_from("<i", hdr_bytes, HDR_BUF_LEN)[0]
    max_buf_off = 0
    for i in range(min(num_buf, 4)):
        base = HDR_VAR_BUF + i * VAR_BUF_STRIDE
        boff = struct.unpack_from("<i", hdr_bytes, base + 4)[0]
        if boff > max_buf_off:
            max_buf_off = boff

    # Actual mmap size = last VarBuf offset + one frame + small margin.
    actual_size = max_buf_off + buf_len + 4096

    data = ctypes.string_at(ptr, actual_size)
    k32.UnmapViewOfFile(ptr)
    k32.CloseHandle(h)
    return data, True


def parse_header(data):
    status       = ri32(data, HDR_STATUS_OFF)
    si_update    = ri32(data, HDR_SI_UPDATE)
    si_len       = ri32(data, HDR_SI_LEN)
    si_offset    = ri32(data, HDR_SI_OFFSET)
    num_vars     = ri32(data, HDR_NUM_VARS)
    var_hdr_off  = ri32(data, HDR_VAR_HDR_OFF)
    num_buf      = ri32(data, HDR_NUM_BUF)
    buf_len      = ri32(data, HDR_BUF_LEN)

    # Pick the most recent VarBuf (highest tickCount)
    best_tick, best_off = -1, 0
    for i in range(min(num_buf, 4)):
        base = HDR_VAR_BUF + i * VAR_BUF_STRIDE
        tick = ri32(data, base)
        off  = ri32(data, base + 4)
        if tick > best_tick:
            best_tick, best_off = tick, off

    return {
        "status":       status,
        "connected":    bool(status & 1),
        "si_update":    si_update,
        "si_len":       si_len,
        "si_offset":    si_offset,
        "num_vars":     num_vars,
        "var_hdr_off":  var_hdr_off,
        "buf_len":      buf_len,
        "best_tick":    best_tick,
        "best_buf_off": best_off,
    }


def build_var_index(data, hdr):
    index = {}
    base = hdr["var_hdr_off"]
    for i in range(hdr["num_vars"]):
        off      = base + i * VAR_HDR_STRIDE
        type_code = ri32(data, off)
        var_off   = ri32(data, off + 4)
        count     = ri32(data, off + 8)
        raw_name  = data[off + VAR_HDR_NAME_OFF : off + VAR_HDR_NAME_OFF + VAR_HDR_NAME_LEN]
        name = raw_name.split(b"\x00")[0].decode("ascii", errors="replace")
        index[name] = {"type": type_code, "offset": var_off, "count": count}
    return index


def read_var(data, info, buf_start):
    off   = buf_start + info["offset"]
    tc    = info["type"]
    count = info["count"]
    if count == 1:
        if tc in (IR_INT, IR_BITFIELD): return struct.unpack_from("<i", data, off)[0]
        if tc == IR_BOOL:               return bool(data[off])
        if tc == IR_FLOAT:              return struct.unpack_from("<f", data, off)[0]
        if tc == IR_DOUBLE:             return struct.unpack_from("<d", data, off)[0]
    else:
        # Return first element for diagnostics
        if tc in (IR_INT, IR_BITFIELD): return struct.unpack_from("<i", data, off)[0]
        if tc == IR_FLOAT:              return struct.unpack_from("<f", data, off)[0]
    return None


def extract_session_yaml(data, hdr):
    si_off = hdr["si_offset"]
    si_len = hdr["si_len"]
    if si_len <= 0 or si_off <= 0:
        return ""
    raw = data[si_off : si_off + si_len]
    return raw.split(b"\x00")[0].decode("utf-8", errors="replace")


def yaml_grep(yaml, *keywords):
    """Return lines from yaml that contain any of the keywords."""
    lines = yaml.splitlines()
    out = []
    for kw in keywords:
        for ln in lines:
            if kw.lower() in ln.lower():
                out.append(ln)
    return out


def run(dump_yaml=False, watch=False):
    while True:
        data, ok = open_mmap()
        if not ok or data is None:
            print("ERROR: Could not open iRacing mmap — is iRacing running?")
            if watch:
                time.sleep(1)
                continue
            sys.exit(1)

        hdr  = parse_header(data)
        vars = build_var_index(data, hdr)
        yaml = extract_session_yaml(data, hdr)

        print("=" * 60)
        print(f"  iRacing mmap diagnostic  {time.strftime('%H:%M:%S')}")
        print("=" * 60)
        print(f"  connected       : {hdr['connected']}  (status={hdr['status']:#010x})")
        print(f"  si_update_tick  : {hdr['si_update']}")
        print(f"  session_info_len: {hdr['si_len']}")
        print(f"  num_vars        : {hdr['num_vars']}")
        print(f"  buf_len         : {hdr['buf_len']}")
        print(f"  best_frame_tick : {hdr['best_tick']}")
        print()

        # Live telemetry vars
        buf_start = hdr["best_buf_off"]
        print("  ── Live telemetry ──")
        for name in sorted(INTERESTING_VARS):
            info = vars.get(name)
            if info is None:
                print(f"  {name:<24} NOT FOUND")
            else:
                val = read_var(data, info, buf_start)
                print(f"  {name:<24} {val}")
        print()

        # YAML snippets most relevant to the SubSessionID bug
        print("  ── SessionInfo YAML (relevant keys) ──")
        for ln in yaml_grep(yaml,
                "SubSessionID", "SubSessionId",
                "WeekendInfo", "TrackName",
                "SessionType", "SessionID",
                "DriverInfo"):
            print(f"  {ln}")
        print()

        if dump_yaml:
            print("  ── Full SessionInfo YAML ──")
            print(yaml)
            print()

        if not watch:
            break
        time.sleep(1)
        print()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="iRacing mmap diagnostic")
    parser.add_argument("--yaml",  action="store_true", help="Dump full SessionInfo YAML")
    parser.add_argument("--watch", action="store_true", help="Refresh every second (Ctrl+C to stop)")
    args = parser.parse_args()
    try:
        run(dump_yaml=args.yaml, watch=args.watch)
    except KeyboardInterrupt:
        print("\nStopped.")
