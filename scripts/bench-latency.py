#!/usr/bin/env python3
"""Measure Oxterm's input-to-model latency over its own control socket.

The script starts one Oxterm per variant with an isolated HOME, types a unique
marker into the focused pane, and polls `capture` until the marker shows up in
the terminal's text model. That is the part of the input path Oxterm controls:
keystroke -> control socket -> GUI PTY -> broker -> shell PTY -> line-discipline
echo -> broker -> VTE. It is *not* a pixel-level measurement: frame pacing and
compositor latency need a custom renderer or a high-speed camera.

Two baselines make the numbers readable:

* `ping` is a no-op control round trip: the cost of the harness itself.
* `capture` is one screen-text extraction: the cost of the check.

Usage:
    scripts/bench-latency.py                       # key=value output
    scripts/bench-latency.py --table               # Markdown table
    scripts/bench-latency.py --runs 200 --table
    scripts/bench-latency.py --variant fast:cell_height_scale=1.2,text_shaping=false

Requires python3 and a running X11 (or XWayland) display.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import shutil
import statistics
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

FRAME_HEADER = 4


def request(path: Path, payload: dict) -> dict:
    """Send one request over the length-prefixed JSON control protocol.

    The server answers exactly one request per connection, so every call opens
    a fresh socket; that connect() cost is included in the `ping` baseline.
    """
    import socket

    body = json.dumps(payload).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.settimeout(10.0)
        sock.connect(str(path))
        sock.sendall(struct.pack(">I", len(body)) + body)
        header = read_exactly(sock, FRAME_HEADER)
        (length,) = struct.unpack(">I", header)
        reply = json.loads(read_exactly(sock, length))
    if not reply.get("ok"):
        raise RuntimeError(reply.get("error", "control request failed"))
    return reply


def read_exactly(sock, count: int) -> bytes:
    chunks = []
    remaining = count
    while remaining:
        chunk = sock.recv(remaining)
        if not chunk:
            raise RuntimeError("control socket closed")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        return float("nan")
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, round(fraction * (len(ordered) - 1))))
    return ordered[index]


def wait_for_socket(home: Path, timeout: float) -> Path:
    deadline = time.monotonic() + timeout
    pattern = str(home / ".config" / "oxterm" / "remote" / "control-*.sock")
    while time.monotonic() < deadline:
        matches = glob.glob(pattern)
        if matches:
            return Path(matches[0])
        time.sleep(0.02)
    raise RuntimeError("the control socket never appeared")


def wait_for_pane(path: Path, timeout: float) -> str:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        panes = request(path, {"op": "list"}).get("panes", [])
        if panes:
            return panes[0]["pane"]
        time.sleep(0.05)
    raise RuntimeError("no pane became available")


def measure_variant(binary: str, name: str, overrides: dict[str, str], runs: int) -> dict:
    home = Path(tempfile.mkdtemp(prefix="oxl.", dir=tempfile.gettempdir()))
    runtime = home / "runtime"
    runtime.mkdir(mode=0o700)
    env = dict(os.environ)
    env.update(
        HOME=str(home),
        XDG_CONFIG_HOME=str(home / ".config"),
        XDG_RUNTIME_DIR=str(runtime),
    )
    command = [binary, "--no-restore", "--class", f"oxlat-{os.getpid()}-{name}"]
    for key, value in overrides.items():
        command += ["-o", f"{key}={value}"]

    process = subprocess.Popen(
        command,
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    try:
        socket_path = wait_for_socket(home, 20.0)
        pane = wait_for_pane(socket_path, 20.0)
        time.sleep(0.3)  # let the first prompt settle

        ping = []
        for _ in range(max(10, runs // 4)):
            start = time.perf_counter_ns()
            request(socket_path, {"op": "ping"})
            ping.append((time.perf_counter_ns() - start) / 1e6)

        capture = []
        for _ in range(max(10, runs // 4)):
            start = time.perf_counter_ns()
            request(socket_path, {"op": "capture", "pane": pane})
            capture.append((time.perf_counter_ns() - start) / 1e6)

        echo = []
        for index in range(runs):
            marker = f"z{index:05d}q"
            start = time.perf_counter_ns()
            request(socket_path, {"op": "send", "pane": pane, "text": marker})
            while True:
                text = request(socket_path, {"op": "capture", "pane": pane}).get("text", "")
                if marker in text:
                    break
            echo.append((time.perf_counter_ns() - start) / 1e6)
            # Keep the input line from growing across iterations.
            request(socket_path, {"op": "send", "pane": pane, "text": "\x15"})

        return {
            "name": name,
            "overrides": overrides,
            "ping_p50": percentile(ping, 0.50),
            "capture_p50": percentile(capture, 0.50),
            "echo_p50": percentile(echo, 0.50),
            "echo_p95": percentile(echo, 0.95),
            "echo_mean": statistics.fmean(echo),
        }
    finally:
        try:
            os.killpg(os.getpgid(process.pid), 15)
        except OSError:
            process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
        shutil.rmtree(home, ignore_errors=True)


def parse_variant(spec: str) -> tuple[str, dict[str, str]]:
    name, _, settings = spec.partition(":")
    if not name:
        raise argparse.ArgumentTypeError("a variant needs a name")
    overrides = {}
    for pair in filter(None, settings.split(",")):
        key, separator, value = pair.partition("=")
        if not separator or not key:
            raise argparse.ArgumentTypeError(f"invalid override: {pair}")
        overrides[key.strip()] = value.strip()
    return name, overrides


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--bin",
        default=str(Path(__file__).resolve().parent.parent / "target" / "release" / "oxterm"),
        help="Oxterm binary (default: target/release/oxterm)",
    )
    parser.add_argument("--runs", type=int, default=100, help="round trips per variant")
    parser.add_argument("--table", action="store_true", help="print a Markdown table")
    parser.add_argument(
        "--variant",
        action="append",
        type=parse_variant,
        help="name:key=value[,key=value]; repeatable (defaults cover common knobs)",
    )
    args = parser.parse_args()

    if not Path(args.bin).is_file():
        print(f"bench-latency: {args.bin} is missing; build it first", file=sys.stderr)
        return 1
    if not os.environ.get("DISPLAY") and not os.environ.get("WAYLAND_DISPLAY"):
        print("bench-latency: no display", file=sys.stderr)
        return 1
    if args.runs < 1:
        print("bench-latency: --runs must be positive", file=sys.stderr)
        return 1

    variants = args.variant or [
        ("defaults", {}),
        ("no-blink", {"cursor_blink": "false"}),
        ("transparency", {"enable_transparency": "true", "opacity": "0.9"}),
        ("no-shaping", {"text_shaping": "false"}),
        (
            "fast",
            {"cursor_blink": "false", "text_shaping": "false", "smooth_scroll": "false"},
        ),
    ]

    results = []
    for name, overrides in variants:
        try:
            results.append(measure_variant(args.bin, name, overrides, args.runs))
        except Exception as error:  # noqa: BLE001 - report and continue
            print(f"bench-latency: variant {name} failed: {error}", file=sys.stderr)
            return 1

    if args.table:
        print("| Variant | ping p50 | capture p50 | echo p50 | echo p95 |")
        print("| --- | --- | --- | --- | --- |")
        for result in results:
            print(
                "| {name} | {ping:.2f} ms | {capture:.2f} ms | {p50:.2f} ms | {p95:.2f} ms |".format(
                    name=result["name"],
                    ping=result["ping_p50"],
                    capture=result["capture_p50"],
                    p50=result["echo_p50"],
                    p95=result["echo_p95"],
                )
            )
    else:
        for result in results:
            for key in ("ping_p50", "capture_p50", "echo_p50", "echo_p95", "echo_mean"):
                print(f"latency.{result['name']}.{key}={result[key]:.3f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
