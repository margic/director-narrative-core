"""
export_iracing_mmap.py — dump the live iRacing shared-memory region to disk.

This script reads the same named mapping used by the Rust publisher and saves:
  - a raw binary snapshot (`.bin`)
  - a JSON manifest with parsed header, key vars, and YAML excerpts (`.json`)

Example:
    python scripts/export_iracing_mmap.py exports/live_irsdk_snapshot.bin
"""

from __future__ import annotations

import argparse
import json
import os
import sys

import iracing_mmap_diag


def build_manifest(data: bytes) -> dict:
    hdr = iracing_mmap_diag.parse_header(data)
    vars_index = iracing_mmap_diag.build_var_index(data, hdr)
    yaml_text = iracing_mmap_diag.extract_session_yaml(data, hdr)
    buf_start = hdr["best_buf_off"]
    interesting = {}
    for name in sorted(iracing_mmap_diag.INTERESTING_VARS):
        info = vars_index.get(name)
        interesting[name] = None if info is None else iracing_mmap_diag.read_var(data, info, buf_start)
    preview = iracing_mmap_diag.yaml_grep(
        yaml_text,
        "SubSessionID",
        "SubSessionId",
        "TrackDisplayName",
        "SessionType",
        "DriverInfo",
    )
    return {
        "header": hdr,
        "interestingVars": interesting,
        "varCount": len(vars_index),
        "varNames": sorted(vars_index.keys()),
        "yamlPreview": preview[:32],
        "yamlLength": len(yaml_text),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Export the live iRacing mmap to disk")
    parser.add_argument("output", help="Raw binary snapshot output path")
    parser.add_argument("--manifest-out", help="Optional JSON manifest path")
    args = parser.parse_args()

    data, ok, _chosen_name = iracing_mmap_diag.open_mmap()
    if not ok or data is None:
        raise SystemExit("Could not open iRacing mmap. Start iRacing or run scripts/mock_iracing_mmap.py --publish.")

    manifest_path = args.manifest_out
    if manifest_path is None:
        root, _ = os.path.splitext(args.output)
        manifest_path = root + ".json"

    os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
    with open(args.output, "wb") as fh:
        fh.write(data)

    manifest = build_manifest(data)
    os.makedirs(os.path.dirname(os.path.abspath(manifest_path)), exist_ok=True)
    with open(manifest_path, "w", encoding="utf-8") as fh:
        json.dump(manifest, fh, indent=2)

    print(f"Wrote raw snapshot: {args.output}")
    print(f"Wrote manifest    : {manifest_path}")


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(130)