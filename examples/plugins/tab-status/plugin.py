#!/usr/bin/env python3
"""Colour a tab while a command runs and after it ends.

Oxterm sends one JSON event per line on standard input. This plugin turns the
tab label amber while a command runs, green when it succeeds, and red when it
fails. A success is cleared after a few seconds; a failure stays red until the
next command starts.

Install with `oxterm plugin install examples/plugins/tab-status`.
"""

import json
import os
import shutil
import subprocess
import sys
import threading

AMBER = "#c9971f"  # a command is running
GREEN = "#4faa5a"  # it succeeded
RED = "#cc4b4b"  # it failed, or reported no status
CLEAR_AFTER = 3.0  # seconds a successful colour stays visible

OXTERM = os.environ.get("OXTERM") or shutil.which("oxterm") or "oxterm"
timers = {}


def cancel(pane):
    timer = timers.pop(pane, None)
    if timer is not None:
        timer.cancel()


def action(pane, *args):
    subprocess.run(
        [OXTERM, "action", *args, "--pane", pane],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )


def main():
    last_exit = {}
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        pane = event.get("pane")
        if not pane:
            continue
        kind = event.get("event")

        if kind == "command_started":
            cancel(pane)
            action(pane, "set-tab-color", AMBER)
        elif kind == "command_finished":
            cancel(pane)
            last_exit[pane] = event.get("exit")
            colour = GREEN if event.get("exit") == 0 else RED
            action(pane, "set-tab-color", colour)
        elif kind == "prompt" and last_exit.get(pane) == 0:
            # The command succeeded: show green briefly, then go back to the
            # theme's normal tab colour. A failure stays red.
            cancel(pane)
            timer = threading.Timer(CLEAR_AFTER, action, args=(pane, "clear-tab-color"))
            timers[pane] = timer
            timer.start()


if __name__ == "__main__":
    main()
