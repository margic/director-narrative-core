"""
windows_smoke_test.py

Windows integration smoke test for the live publisher path.

What it does:
1. Starts the synthetic iRacing mmap publisher.
2. Runs the Rust publisher in --dry-run --no-ui mode.
3. Parses emitted dry-run envelopes and counts event types.
4. Asserts a minimum set of detector/session events.
5. Shuts both processes down cleanly and returns non-zero on failure.

Usage:
    python scripts/windows_smoke_test.py
    python scripts/windows_smoke_test.py --scenario detector-smoke --time-scale 12
"""

from __future__ import annotations

import argparse
import os
import queue
import re
import signal
import subprocess
import sys
import threading
import time
from collections import Counter
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
EVENT_TYPE_RE = re.compile(r'"type"\s*:\s*"([A-Z_]+)"')


def _reader_thread(stream, sink: queue.Queue, prefix: str) -> None:
    try:
        for raw in iter(stream.readline, ""):
            if not raw:
                break
            line = raw.rstrip("\r\n")
            sink.put((prefix, line))
    finally:
        try:
            stream.close()
        except Exception:
            pass


def terminate_process(proc: subprocess.Popen, name: str, timeout_s: float = 8.0) -> None:
    if proc.poll() is not None:
        return
    try:
        proc.send_signal(signal.CTRL_BREAK_EVENT)
    except Exception:
        proc.terminate()
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if proc.poll() is not None:
            return
        time.sleep(0.1)
    proc.kill()
    proc.wait(timeout=5)
    print(f"[smoke] {name} needed force kill")


def build_publisher(skip_build: bool) -> Path:
    exe = REPO_ROOT / "target" / "debug" / "publisher.exe"
    if skip_build and exe.exists():
        return exe
    print("[smoke] building publisher binary...")
    cmd = ["cargo", "build", "--bin", "publisher"]
    res = subprocess.run(cmd, cwd=REPO_ROOT, capture_output=True, text=True)
    if res.returncode != 0:
        print(res.stdout)
        print(res.stderr)
        raise RuntimeError("cargo build failed")
    if not exe.exists():
        raise RuntimeError("publisher.exe not found after build")
    return exe


def run_smoke(args: argparse.Namespace) -> int:
    if os.name != "nt":
        print("[smoke] Windows-only smoke test")
        return 2

    publisher_exe = build_publisher(args.skip_build)

    mmap_name = f"Local\\IRSDKMemMapFileName_Smoke_{os.getpid()}"
    event_name = f"Local\\IRSDKDataValidEvent_Smoke_{os.getpid()}"

    mock_cmd = [
        sys.executable,
        "-u",
        str(REPO_ROOT / "scripts" / "mock_iracing_mmap.py"),
        "--scenario",
        args.scenario,
        "--rate-hz",
        str(args.rate_hz),
        "--time-scale",
        str(args.time_scale),
        "--mmap-name",
        mmap_name,
        "--event-name",
        event_name,
    ]
    if args.loop:
        mock_cmd.append("--loop")

    print("[smoke] starting mock mmap publisher...")
    mock_proc = subprocess.Popen(
        mock_cmd,
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
        creationflags=subprocess.CREATE_NEW_PROCESS_GROUP,
    )

    mock_q: queue.Queue = queue.Queue()
    mock_t = threading.Thread(
        target=_reader_thread,
        args=(mock_proc.stdout, mock_q, "mock"),
        daemon=True,
    )
    mock_t.start()

    mock_ready = False
    mock_deadline = time.time() + args.start_timeout
    while time.time() < mock_deadline:
        if mock_proc.poll() is not None:
            break
        try:
            _, line = mock_q.get(timeout=0.2)
        except queue.Empty:
            continue
        print(f"[mock] {line}")
        if "already exists" in line.lower():
            terminate_process(mock_proc, "mock")
            print("[smoke] fail: live iRacing mapping already exists; close iRacing and rerun")
            return 1
        if "start the Rust publisher now" in line:
            mock_ready = True
            break

    if not mock_ready:
        terminate_process(mock_proc, "mock")
        print("[smoke] fail: mock publisher did not become ready")
        return 1

    pub_env = os.environ.copy()
    pub_env.setdefault("PUBLISHER_AUTH_TENANT_ID", "smoke-tenant")
    pub_env.setdefault("PUBLISHER_AUTH_CLIENT_ID", "smoke-client")
    pub_env.setdefault("PUBLISHER_AUTH_CLIENT_SECRET", "smoke-secret")
    pub_env.setdefault("PUBLISHER_AUTH_SCOPE", "api://smoke/.default")
    pub_env.setdefault("PUBLISHER_RC_API_URL", "http://localhost")
    pub_env.setdefault("PUBLISHER_BATCH_INTERVAL_MS", "250")
    pub_env["SIM_MMAP_NAME"] = mmap_name
    pub_env["SIM_EVENT_NAME"] = event_name

    pub_cmd = [str(publisher_exe), "--no-ui", "--dry-run"]
    print("[smoke] starting publisher --dry-run...")
    pub_proc = subprocess.Popen(
        pub_cmd,
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
        env=pub_env,
        creationflags=subprocess.CREATE_NEW_PROCESS_GROUP,
    )

    pub_q: queue.Queue = queue.Queue()
    pub_t = threading.Thread(
        target=_reader_thread,
        args=(pub_proc.stdout, pub_q, "pub"),
        daemon=True,
    )
    pub_t.start()

    counts: Counter[str] = Counter()
    deadline = time.time() + args.max_runtime
    while time.time() < deadline:
        if pub_proc.poll() is not None:
            break
        try:
            _, line = pub_q.get(timeout=0.25)
        except queue.Empty:
            continue
        if line:
            print(f"[publisher] {line}")
        match = EVENT_TYPE_RE.search(line)
        if match:
            counts[match.group(1)] += 1

        if (
            counts["PUBLISHER_HELLO"] >= 1
            and counts["RACE_GREEN"] >= 1
            and counts["BATTLE_ENGAGED"] >= 1
            and (counts["OVERTAKE"] + counts["OVERTAKE_FOR_LEAD"]) >= 1
        ):
            print("[smoke] reached required event coverage early")
            break

    terminate_process(pub_proc, "publisher")
    terminate_process(mock_proc, "mock")

    # Drain any remaining publisher output for late-printed dry-run lines.
    drain_deadline = time.time() + 2.0
    while time.time() < drain_deadline:
        try:
            _, line = pub_q.get_nowait()
        except queue.Empty:
            break
        match = EVENT_TYPE_RE.search(line)
        if match:
            counts[match.group(1)] += 1

    print("[smoke] event counts summary:")
    for key in sorted(counts.keys()):
        print(f"  {key}: {counts[key]}")

    failures = []
    if counts["PUBLISHER_HELLO"] < 1:
        failures.append("missing PUBLISHER_HELLO")
    if counts["RACE_GREEN"] < 1:
        failures.append("missing RACE_GREEN")
    if counts["BATTLE_ENGAGED"] < 1:
        failures.append("missing BATTLE_ENGAGED")
    if (counts["OVERTAKE"] + counts["OVERTAKE_FOR_LEAD"]) < 1:
        failures.append("missing overtake event")

    if failures:
        print("[smoke] FAIL")
        for f in failures:
            print(f"  - {f}")
        return 1

    print("[smoke] PASS")
    return 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Windows smoke test for mock mmap + publisher dry-run")
    parser.add_argument("--scenario", choices=["detector-smoke", "session-rollover"], default="detector-smoke")
    parser.add_argument("--rate-hz", type=int, default=60)
    parser.add_argument("--time-scale", type=float, default=12.0, help="Simulation speed multiplier")
    parser.add_argument("--max-runtime", type=float, default=90.0, help="Max wall-clock time before forced stop")
    parser.add_argument("--start-timeout", type=float, default=12.0, help="Startup timeout for mock readiness")
    parser.add_argument("--loop", action="store_true", help="Loop the mock scenario")
    parser.add_argument("--skip-build", action="store_true", help="Skip cargo build step and use existing publisher.exe")
    return parser.parse_args()


if __name__ == "__main__":
    sys.exit(run_smoke(parse_args()))