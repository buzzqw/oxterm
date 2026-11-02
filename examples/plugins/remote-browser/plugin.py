#!/usr/bin/env python3
"""Open the two-pane file browser when an SSH or Mosh session starts.

A WinSCP-like companion for people who live in the terminal: open an SSH tab
and the remote file browser for the same destination appears automatically.

Oxterm sends `command_started` events; this plugin extracts the destination
from `ssh ... host` or `mosh ... host` and runs `oxterm browse host`. It works
when the destination matches a saved connection or a host in ~/.ssh/config. A
destination that cannot be resolved is ignored.

Install with:
    oxterm plugin install examples/plugins/remote-browser
"""

import json
import os
import shlex
import subprocess
import sys
import time

OXTERM = os.environ.get("OXTERM") or "oxterm"
# One browser per destination per minute, so a retry loop cannot open dozens.
COOLDOWN = 60.0
seen = {}


def destination(command):
    try:
        parts = shlex.split(command)
    except ValueError:
        return None
    if not parts or parts[0] not in ("ssh", "mosh"):
        return None
    index = 1
    while index < len(parts):
        part = parts[index]
        if part == "--":
            index += 1
            break
        if part.startswith("-"):
            # Options that take a separate value.
            if part in ("-p", "-i", "-J", "-l", "-o", "-F", "-e", "-b", "-c") and index + 1 < len(parts):
                index += 2
            else:
                index += 1
            continue
        break
    if index >= len(parts):
        return None
    target = parts[index]
    if "@" in target:
        target = target.rsplit("@", 1)[1]
    return target or None


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("event") != "command_started":
            continue
        host = destination(event.get("command", ""))
        if host is None:
            continue
        now = time.monotonic()
        if now - seen.get(host, 0.0) < COOLDOWN:
            continue
        seen[host] = now
        subprocess.Popen(
            [OXTERM, "browse", host],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )


if __name__ == "__main__":
    main()
