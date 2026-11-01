#!/usr/bin/env python3
"""Run a real `ssh` through oxterm-askpass in a PTY and check one scenario.

Usage: password-client.py HOME HELPER METHOD CONNECTION_ID EXPECT
EXPECT is `login` (the stored password logs in, with one attempt on the server)
or a number N: `ssh` must end up asking the person, after N attempts.
"""
import os
import pty
import re
import select
import subprocess
import sys
import time

home, helper, method, connection_id, expect = sys.argv[1:6]
extra = sys.argv[6:]
server = os.path.join(os.path.dirname(os.path.abspath(__file__)), "password-server.py")
log_path = os.path.join(home, f"auth-{connection_id}-{method}.log")
if os.path.exists(log_path):
    os.remove(log_path)
process = subprocess.Popen(
    [sys.executable, "-I", server, method, log_path, *(["totp"] if extra else [])],
    stdout=subprocess.PIPE,
    text=True,
)
port = process.stdout.readline().strip()
argv = [
    "env",
    f"SSH_ASKPASS={helper}",
    "SSH_ASKPASS_REQUIRE=force",
    f"OXTERM_ASKPASS_ID={connection_id}",
    "OXTERM_ASKPASS_TARGET=tester@127.0.0.1",
    *(["OXTERM_ASKPASS_TOTP=1"] if extra == ["totp"] else []),
    "ssh",
    "-F", "/dev/null",
    "-o", f"UserKnownHostsFile={home}/known-{connection_id}-{method}",
    "-o", "StrictHostKeyChecking=accept-new",
    "-o", "PubkeyAuthentication=no",
    "-o", f"PreferredAuthentications={method}",
    "-o", "ControlPath=none",
    "-l", "tester", "-p", port, "127.0.0.1", "true",
]
pid, fd = pty.fork()
if pid == 0:
    os.execvp(argv[0], argv)
output = b""
asked_person = False
deadline = time.time() + 15
while time.time() < deadline:
    ready, _, _ = select.select([fd], [], [], 0.3)
    if ready:
        try:
            data = os.read(fd, 4096)
        except OSError:
            break
        if not data:
            break
        output += data
    if re.search(rb"([Pp]assword|code): $", output):
        asked_person = True
        break
try:
    os.kill(pid, 9)
except ProcessLookupError:
    pass
process.kill()
attempts = open(log_path, encoding="utf-8").read().split("\n")[:-1] if os.path.exists(log_path) else []
label = f"{method}/{connection_id}"
if expect == "login":
    passed = b"LOGIN-OK" in output and not asked_person and len(attempts) == 1 and "ok=True" in attempts[0]
else:
    passed = asked_person and b"LOGIN-OK" not in output and len(attempts) == int(expect)
print(f"{'ok' if passed else 'FAILED'}: {label}: asked_person={asked_person} server_saw={attempts}")
sys.exit(0 if passed else 1)
