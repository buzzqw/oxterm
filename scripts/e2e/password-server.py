#!/usr/bin/env python3
"""A throwaway SSH server that checks one password, for e2e-password.sh.

Needs paramiko. An unprivileged sshd cannot verify system passwords, so a real
`ssh` client talks to this instead. It logs each attempt, which lets the caller
count how many times a stored password was really sent.
"""
import socket
import sys
import threading

import paramiko

PASSWORD = "s3cret-pw"
method, log_path = sys.argv[1], sys.argv[2]
host_key = paramiko.RSAKey.generate(2048)


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
        query.add_prompt("Password: ", False)
        return query

    def check_auth_interactive_response(self, responses):
        ok = len(responses) == 1 and responses[0] == PASSWORD
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
