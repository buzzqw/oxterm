#!/usr/bin/env python3
"""A throwaway SSH server that checks one password, for e2e-password.sh.

Needs paramiko. An unprivileged sshd cannot verify system passwords, so a real
`ssh` client talks to this instead. It logs each attempt, which lets the caller
count how many times a stored password was really sent.
"""
import hmac
import socket
import struct
import sys
import threading
import time

import paramiko

PASSWORD = "s3cret-pw"
TOTP_SECRET = b"12345678901234567890"  # Base32: GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ
method, log_path = sys.argv[1], sys.argv[2]
ask_code = len(sys.argv) > 3 and sys.argv[3] == "totp"
host_key = paramiko.RSAKey.generate(2048)


def totp_now():
    digest = hmac.new(TOTP_SECRET, struct.pack(">Q", int(time.time()) // 30), "sha1").digest()
    offset = digest[19] & 15
    value = struct.unpack(">I", digest[offset:offset + 4])[0] & 0x7FFFFFFF
    return f"{value % 1000000:06d}"


def log(line):
    with open(log_path, "a", encoding="utf-8") as stream:
        stream.write(line + "\n")


class Server(paramiko.ServerInterface):
    def get_allowed_auths(self, username):
        return method

    def check_auth_password(self, username, password):
        ok = password == PASSWORD
        log(f"password ok={ok}")
        return paramiko.AUTH_SUCCESSFUL if ok else paramiko.AUTH_FAILED

    def check_auth_interactive(self, username, submethods):
        query = paramiko.InteractiveQuery()
        query.add_prompt("Verification code: " if ask_code else "Password: ", False)
        return query

    def check_auth_interactive_response(self, responses):
        expected = totp_now() if ask_code else PASSWORD
        ok = len(responses) == 1 and responses[0] == expected
        log(f"keyboard-interactive ok={ok}")
        return paramiko.AUTH_SUCCESSFUL if ok else paramiko.AUTH_FAILED

    def check_channel_request(self, kind, chanid):
        return paramiko.OPEN_SUCCEEDED

    def check_channel_exec_request(self, channel, command):
        channel.send(b"LOGIN-OK\n")
        channel.send_exit_status(0)
        threading.Timer(0.2, channel.close).start()
        return True


listener = socket.socket()
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("127.0.0.1", 0))
listener.listen(5)
print(listener.getsockname()[1], flush=True)
while True:
    connection, _ = listener.accept()
    transport = paramiko.Transport(connection)
    transport.add_server_key(host_key)
    try:
        transport.start_server(server=Server())
    except (paramiko.SSHException, EOFError) as error:
        log(f"error {error}")
