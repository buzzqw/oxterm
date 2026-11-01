#!/usr/bin/env python3
"""Import the saved connections of Python Connection Manager (PCM) into Oxterm.

PCM keeps ``connections.json`` and ``pcm_settings.json`` beside its code. When
PCM's credential encryption is on, the usernames in that file are ``ENC:...``
tokens: the script asks for the master password (through zenity when there is a
display, otherwise on the terminal) and decrypts only what it needs. Passwords
are deliberately not imported. Oxterm keeps secrets in the Secret Service and
its import gives every entry a new id, so nothing in a book file could reach a
keyring item anyway: type a password once in the session and, if the entry asks
for it, Oxterm stores it from there.

The output is an ordinary Oxterm book, written to a temporary file unless
``--output`` says otherwise. With ``--import`` the script then runs
``oxterm connections import`` on it. Entries that act on this computer (a
pre-connect command, session logs, expect rules) are skipped by Oxterm unless
``--allow-local-commands`` is given; it is your own file, so that consent is
yours to give.

Usage:
    scripts/import-pcm.py                       # convert only
    scripts/import-pcm.py --import              # convert and add to Oxterm
    scripts/import-pcm.py --pcm-dir ~/pcm/gtk3 --import
    PCM_MASTER_PASSWORD=... scripts/import-pcm.py --import

Requires python3 and, when PCM encryption is enabled, the ``cryptography``
package (``pip install cryptography`` or run PCM's own virtualenv).
"""

from __future__ import annotations

import argparse
import base64
import getpass
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_PCM_DIR = Path.home() / "Python_Connection_Manager" / "gtk3"

# PCM's crypto_manager.py uses the same parameters: Fernet (AES-128-CBC +
# HMAC-SHA256) over a key derived with PBKDF2-HMAC-SHA256.
PBKDF2_ITERATIONS = 480_000
ENC_PREFIX = "ENC:"

PROTOCOLS = {"ssh": "ssh", "rdp": "rdp", "vnc": "vnc"}
FILE_PROTOCOLS = {"SFTP": "sftp", "FTP": "ftp"}

# PCM's Italian theme names for palettes Oxterm also ships.
THEME_ALIASES = {
    "Scuro (Default)": "Dark (Default)",
    "Chiaro (Default)": "Light",
    "Matrix (Green)": "Matrix",
    "Matrix (Verde)": "Matrix",
}

# PCM's ``terminal_type`` is the program PCM starts the session in, not the
# TERM value; only values that really are terminal names make sense in Oxterm.
TERM_TYPES = {
    "ansi",
    "dumb",
    "linux",
    "rxvt",
    "rxvt-unicode",
    "screen",
    "screen-256color",
    "tmux",
    "tmux-256color",
    "vt100",
    "vt220",
    "xterm",
    "xterm-256color",
}

MAC_RE = re.compile(r"^(?:[0-9A-Fa-f]{2}[:-]){5}[0-9A-Fa-f]{2}$")


def text(value: Any) -> str:
    """A JSON value as trimmed text; PCM mixes strings, numbers and bools."""
    if value is None or isinstance(value, bool):
        return ""
    return str(value).strip()


def truthy(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return value != 0
    return text(value).lower() in {"1", "true", "yes", "on", "si", "sì"}


def whole(value: Any, low: int | None = None, high: int | None = None) -> int | None:
    number = text(value)
    if not number:
        return None
    try:
        parsed = int(number)
    except ValueError:
        return None
    if low is not None and parsed < low:
        return None
    if high is not None and parsed > high:
        return None
    return parsed


def put(connection: dict[str, Any], key: str, value: Any) -> None:
    """Add a field only when it says something; Oxterm fills in the defaults."""
    if value in ("", None, False, [], {}):
        return
    connection[key] = value


# ── PCM encryption ────────────────────────────────────────────


def ask_password() -> str:
    """The PCM master password, without putting it on a command line."""
    from_env = os.environ.get("PCM_MASTER_PASSWORD")
    if from_env is not None:
        return from_env
    if os.environ.get("DISPLAY") and shutil.which("zenity"):
        result = subprocess.run(
            [
                "zenity",
                "--password",
                "--title",
                "PCM master password",
                "--text",
                "Unlock the usernames stored by Python Connection Manager:",
            ],
            capture_output=True,
            text=True,
        )
        if result.returncode == 0:
            return result.stdout.rstrip("\n")
        raise SystemExit("the password prompt was cancelled")
    return getpass.getpass("PCM master password: ")


class Decryptor:
    """Decrypt the fields PCM wrote with crypto_manager.encrypt_field."""

    def __init__(self, settings: dict[str, Any]) -> None:
        self.fernet = None
        crypto = settings.get("crypto")
        if not isinstance(crypto, dict) or not truthy(crypto.get("enabled")):
            return
        salt = text(crypto.get("salt"))
        if not salt:
            raise SystemExit("PCM says encryption is on but pcm_settings.json has no salt")
        try:
            from cryptography.fernet import Fernet, InvalidToken
        except ImportError as error:  # pragma: no cover - depends on the host
            raise SystemExit(
                "PCM's connections are encrypted and the 'cryptography' package is "
                "missing; install it with 'pip install cryptography' (or run this "
                "script with PCM's virtualenv)"
            ) from error
        self.invalid_token = InvalidToken
        password = ask_password()
        raw = hashlib.pbkdf2_hmac(
            "sha256",
            password.encode("utf-8"),
            base64.b64decode(salt),
            PBKDF2_ITERATIONS,
            dklen=32,
        )
        self.fernet = Fernet(base64.urlsafe_b64encode(raw))
        verify = text(crypto.get("verify"))
        if verify:
            try:
                self.fernet.decrypt(verify.encode("ascii"))
            except InvalidToken:
                raise SystemExit("the PCM master password is wrong") from None

    def field(self, value: Any, label: str) -> str:
        """Plain text of a PCM field; an encrypted value must decrypt."""
        plain = text(value)
        if not plain.startswith(ENC_PREFIX):
            return plain
        if self.fernet is None:
            print(
                f"warning: {label} is encrypted but PCM encryption is off; leaving it empty",
                file=sys.stderr,
            )
            return ""
        try:
            raw = self.fernet.decrypt(plain[len(ENC_PREFIX) :].encode("ascii"))
        except self.invalid_token:
            raise SystemExit(f"cannot decrypt {label}; the master password does not match") from None
        return raw.decode("utf-8", "replace").strip()


# ── Mapping one PCM entry to one Oxterm connection ────────────


class Context:
    """The parts of pcm_settings.json a conversion needs."""

    def __init__(self, settings: dict[str, Any], decryptor: Decryptor) -> None:
        self.decryptor = decryptor
        self.themes = theme_names()
        profiles = settings.get("credential_profiles")
        self.profiles = {
            text(profile.get("name")): profile
            for profile in profiles or []
            if isinstance(profile, dict) and text(profile.get("name"))
        }
        tunnels = settings.get("tunnels")
        self.tunnels = {
            text(tunnel.get("nome") or tunnel.get("name")): tunnel
            for tunnel in tunnels or []
            if isinstance(tunnel, dict)
            and text(tunnel.get("nome") or tunnel.get("name"))
        }
        self.warnings: list[str] = []
        self.passwords = 0
        self.decrypted_users = 0

    def warn(self, message: str) -> None:
        self.warnings.append(message)

    def user(self, name: str, entry: dict[str, Any], protocol: str) -> str:
        raw = entry.get("user")
        if text(raw).startswith(ENC_PREFIX):
            self.decrypted_users += 1
        user = self.decryptor.field(raw, f"{name}: user")
        if not user and protocol == "rdp":
            profile = self.profiles.get(text(entry.get("credential_profile")))
            if profile:
                user = text(profile.get("user"))
        return user

    def theme(self, name: str, value: Any) -> str:
        requested = text(value)
        if not requested:
            return ""
        mapped = THEME_ALIASES.get(requested, requested)
        if mapped in THEME_ALIASES.values() or mapped in self.themes:
            return mapped
        self.warn(f"{name}: theme {requested!r} has no Oxterm equivalent; using the global one")
        return ""

    def tunnels_for(
        self, name: str, entry: dict[str, Any], protocol: str
    ) -> list[dict[str, Any]]:
        reference = text(entry.get("tunnel"))
        if not reference:
            return []
        spec = self.tunnels.get(reference)
        if spec is None:
            self.warn(f"{name}: tunnel {reference!r} is not in pcm_settings.json")
            return []
        if protocol == "rdp":
            # RDP entries may name a tunnel, but Oxterm's desktop entries make
            # their own forward when they need one.
            return []
        host = text(spec.get("ssh_host"))
        port = whole(spec.get("ssh_port"))
        if host and host != text(entry.get("host")):
            self.warn(
                f"{name}: tunnel {reference!r} goes through {host}, not the entry's host; skipped"
            )
            return []
        if port and port != whole(entry.get("port")):
            self.warn(
                f"{name}: tunnel {reference!r} uses ssh port {port}, not the entry's; skipped"
            )
            return []
        kind_text = text(spec.get("tipo") or spec.get("type")).lower()
        if "-d" in kind_text or "socks" in kind_text or "dynamic" in kind_text:
            kind = "dynamic"
        elif "-r" in kind_text or "remote" in kind_text:
            kind = "remote"
        else:
            kind = "local"
        local_port = text(spec.get("local_port"))
        remote_host = text(spec.get("remote_host"))
        remote_port = text(spec.get("remote_port"))
        if kind == "remote":
            listen = remote_port
            target = f"127.0.0.1:{local_port}" if local_port else ""
        else:
            listen = local_port
            target = f"{remote_host}:{remote_port}" if remote_host and remote_port else ""
        if not listen:
            self.warn(f"{name}: tunnel {reference!r} has no port; skipped")
            return []
        if kind != "dynamic" and not target:
            self.warn(f"{name}: tunnel {reference!r} has no target; skipped")
            return []
        tunnel: dict[str, Any] = {"type": kind, "listen": listen}
        if kind != "dynamic":
            tunnel["target"] = target
        if truthy(entry.get("tunnel_start")):
            tunnel["autostart"] = True
        return [tunnel]


def theme_names() -> set[str]:
    try:
        catalogue = json.loads((ROOT / "assets" / "themes.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return set()
    return {
        text(item.get("name")) for item in catalogue if isinstance(item, dict) and item.get("name")
    }


def proxy_jump(entry: dict[str, Any], ctx: Context, name: str) -> str:
    host = text(entry.get("jump_host"))
    if not host:
        return ""
    user = ctx.decryptor.field(entry.get("jump_user"), f"{name}: jump user")
    port = whole(entry.get("jump_port"), 1, 65535)
    target = f"{user}@{host}" if user else host
    if port and port != 22:
        target += f":{port}"
    return target


def macros(entry: dict[str, Any], name: str, ctx: Context) -> list[dict[str, str]]:
    result = []
    for item in entry.get("macros") or []:
        if not isinstance(item, dict):
            continue
        macro_name = text(item.get("nome") or item.get("name"))
        command = text(item.get("cmd") or item.get("command"))
        if macro_name and command:
            result.append({"name": macro_name, "command": command})
        elif macro_name or command:
            ctx.warn(f"{name}: macro {macro_name or command!r} has no name or command; skipped")
    return result


def expect_rules(entry: dict[str, Any], name: str, ctx: Context) -> list[dict[str, Any]]:
    result = []
    for item in entry.get("expect_rules") or []:
        if not isinstance(item, dict):
            continue
        pattern = text(item.get("pattern") or item.get("regex") or item.get("match"))
        command = text(item.get("command") or item.get("cmd") or item.get("send"))
        if not pattern or not command:
            ctx.warn(f"{name}: an expect rule has no pattern or command; skipped")
            continue
        rule: dict[str, Any] = {"pattern": pattern, "command": command}
        trigger = whole(item.get("max_trigger"))
        delay = whole(item.get("delay_ms"))
        if trigger is not None:
            rule["max_trigger"] = trigger
        if delay is not None:
            rule["delay_ms"] = delay
        result.append(rule)
    return result


def font_value(entry: dict[str, Any]) -> str:
    family = text(entry.get("term_font"))
    size = whole(entry.get("term_size"), 1, 200)
    if family and size:
        return f"{family} {size}"
    return family


def terminal_type(entry: dict[str, Any]) -> str:
    value = text(entry.get("terminal_type"))
    return value if value.lower() in TERM_TYPES else ""


def convert_entry(
    name: str, entry: dict[str, Any], index: int, ctx: Context
) -> tuple[dict[str, Any] | None, str | None]:
    kind_key = text(entry.get("protocol")).lower()
    if kind_key == "file_transfer":
        protocol = FILE_PROTOCOLS.get(text(entry.get("ft_protocol")).upper())
    else:
        protocol = PROTOCOLS.get(kind_key)
    if protocol is None:
        return None, f"{name}: protocol {kind_key or '?'!r} has no Oxterm equivalent"

    host = text(entry.get("host"))
    if not host:
        return None, f"{name}: no host"

    if text(entry.get("password")):
        ctx.passwords += 1

    connection: dict[str, Any] = {
        "id": f"pcm-{index:03d}",
        "kind": "manual",
        "protocol": protocol,
        "name": name,
        "host": host,
    }
    put(connection, "user", ctx.user(name, entry, protocol))
    put(connection, "port", whole(entry.get("port"), 1, 65535))
    put(connection, "group", text(entry.get("group")))
    put(connection, "notes", text(entry.get("notes")))
    tags = [tag for tag in re.split(r"[,\s]+", text(entry.get("tags"))) if tag]
    put(connection, "tags", tags)
    put(connection, "pre_command", text(entry.get("pre_cmd")))
    if text(entry.get("pre_cmd")):
        put(connection, "pre_command_timeout", whole(entry.get("pre_cmd_timeout"), 0, 86_400))
    mac = text(entry.get("wol_mac"))
    if mac and MAC_RE.match(mac):
        put(connection, "wol_mac", mac)
        put(connection, "wol_wait", whole(entry.get("wol_wait"), 0, 3_600))
    elif mac:
        ctx.warn(f"{name}: wake-on-LAN MAC {mac!r} is not valid; skipped")
    if truthy(entry.get("auto_reconnect")):
        connection["auto_reconnect"] = True
        put(connection, "reconnect_delay", whole(entry.get("reconnect_delay"), 0, 86_400))
    if truthy(entry.get("log_output")):
        log_dir = text(entry.get("log_dir"))
        if log_dir:
            connection["log_output"] = True
            connection["log_dir"] = log_dir
        else:
            ctx.warn(f"{name}: session logging is on but no folder is set; skipped")
    put(connection, "macros", macros(entry, name, ctx))
    put(connection, "expect", expect_rules(entry, name, ctx))

    if protocol in {"ssh", "sftp"}:
        put(connection, "identity_file", text(entry.get("private_key")))
        put(connection, "proxy_jump", proxy_jump(entry, ctx, name))

    if protocol in {"ssh", "sftp", "ftp"}:
        put(connection, "compression", truthy(entry.get("compression")))
        keepalive = entry.get("keepalive")
        if keepalive is not None:
            if truthy(keepalive):
                interval = whole(entry.get("keepalive_interval"), 1, 86_400)
                connection["keepalive_interval"] = interval or 60
            else:
                connection["keepalive_interval"] = 0
        strict = entry.get("strict_host")
        if strict is not None:
            connection["host_key_check"] = "strict" if truthy(strict) else "accept_new"
        multiplex = entry.get("ssh_multiplex")
        if multiplex is not None and not truthy(multiplex):
            connection["no_sharing"] = True

    if protocol == "ssh":
        put(connection, "x11", truthy(entry.get("x11")))
        put(connection, "agent_forward", truthy(entry.get("agent_forward")))
        put(connection, "terminal_type", terminal_type(entry))
        put(connection, "font", font_value(entry))
        if not truthy(entry.get("term_infinite_scrollback")):
            lines = whole(entry.get("term_scrollback_lines"), 1, 10_000_000)
            put(connection, "scrollback", lines)
        encoding = text(entry.get("term_encoding"))
        if encoding and encoding.upper() != "UTF-8":
            connection["encoding"] = encoding
        put(connection, "color_scheme", ctx.theme(name, entry.get("term_theme")))
        command = text(entry.get("startup_cmd"))
        if command:
            connection["remote_command"] = command
            # PCM runs the start-up command inside the login shell; Oxterm
            # would otherwise close the session when the command ends.
            connection["keep_shell"] = True

    if protocol == "ftp":
        put(connection, "ftp_tls", truthy(entry.get("ftp_tls")))

    if protocol in {"rdp", "vnc"}:
        put(connection, "desktop_fullscreen", truthy(entry.get("fullscreen")))
        put(connection, "rdp_clipboard", truthy(entry.get("redirect_clipboard")))
        monitor_mode = text(entry.get("rdp_monitor_mode")).lower()
        put(connection, "rdp_multimon", bool(monitor_mode) and monitor_mode != "single")
        if truthy(entry.get("redirect_drives")):
            ctx.warn(f"{name}: PCM shared its drives; set a shared folder in Oxterm if you need it")
        domain = text(entry.get("rdp_domain"))
        if not domain:
            profile = ctx.profiles.get(text(entry.get("credential_profile")))
            domain = text(profile.get("domain")) if profile else ""
        put(connection, "rdp_domain", domain)

    put(connection, "tunnels", ctx.tunnels_for(name, entry, protocol))
    return connection, None


# ── Running Oxterm ───────────────────────────────────────────


def default_binary() -> str | None:
    installed = shutil.which("oxterm")
    if installed:
        return installed
    for candidate in (
        ROOT / "target" / "release" / "oxterm",
        ROOT / "oxterm-linux-x86-64",
    ):
        if candidate.is_file():
            return str(candidate)
    return None


def import_into_oxterm(binary: str, book: Path, allow_local: bool) -> int:
    command = [binary, "connections", "import", str(book)]
    if allow_local:
        command.append("--allow-local-commands")
    print("running:", shlex.join(command))
    return subprocess.run(command).returncode


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Convert PCM's connections.json into an Oxterm book.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--pcm-dir",
        type=Path,
        default=DEFAULT_PCM_DIR,
        help=f"PCM directory with connections.json (default: {DEFAULT_PCM_DIR})",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="write the Oxterm book here instead of a temporary file",
    )
    parser.add_argument(
        "--import",
        dest="do_import",
        action="store_true",
        help="add the converted entries to Oxterm's own connections.json",
    )
    parser.add_argument(
        "--binary",
        help="Oxterm executable used by --import (default: oxterm on PATH, then target/release)",
    )
    parser.add_argument(
        "--allow-local-commands",
        action="store_true",
        help="let --import accept entries with a pre-connect command, logs or expect rules",
    )
    args = parser.parse_args()

    connections_path = args.pcm_dir / "connections.json"
    settings_path = args.pcm_dir / "pcm_settings.json"
    try:
        raw_connections = json.loads(connections_path.read_text(encoding="utf-8"))
        settings = json.loads(settings_path.read_text(encoding="utf-8"))
    except FileNotFoundError as error:
        print(f"error: {error.filename} does not exist", file=sys.stderr)
        return 1
    except ValueError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    if not isinstance(raw_connections, dict):
        print("error: connections.json is not a PCM profile map", file=sys.stderr)
        return 1

    decryptor = Decryptor(settings if isinstance(settings, dict) else {})
    ctx = Context(settings if isinstance(settings, dict) else {}, decryptor)

    converted = []
    skipped = []
    counts: dict[str, int] = {}
    for index, (name, entry) in enumerate(sorted(raw_connections.items())):
        if not isinstance(entry, dict):
            skipped.append(f"{name}: not a profile")
            continue
        if truthy(entry.get("is_template")):
            skipped.append(f"{name}: template entries are not connections")
            continue
        connection, reason = convert_entry(name, entry, index, ctx)
        if connection is None:
            skipped.append(reason or f"{name}: could not be converted")
            continue
        converted.append(connection)
        counts[connection["protocol"]] = counts.get(connection["protocol"], 0) + 1

    book = {"version": 1, "connections": converted}
    if args.output:
        output = args.output
        output.parent.mkdir(parents=True, exist_ok=True)
    else:
        handle, temporary = tempfile.mkstemp(prefix="oxterm-pcm-", suffix=".json")
        os.close(handle)
        output = Path(temporary)
    output.write_text(json.dumps(book, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    os.chmod(output, 0o600)

    summary = ", ".join(f"{count} {protocol}" for protocol, count in sorted(counts.items()))
    print(f"PCM: {len(raw_connections)} profiles in {connections_path}")
    print(f"converted: {len(converted)} ({summary})")
    if ctx.decrypted_users:
        print(f"usernames decrypted: {ctx.decrypted_users}")
    if ctx.passwords:
        print(
            f"passwords left behind: {ctx.passwords} (Oxterm keeps secrets in the "
            "Secret Service; type each one once in the session)"
        )
    for warning in ctx.warnings:
        print(f"warning: {warning}", file=sys.stderr)
    for reason in skipped:
        print(f"skipped: {reason}", file=sys.stderr)
    print(f"book: {output}")

    if not args.do_import:
        print(f"to import: oxterm connections import {shlex.quote(str(output))}")
        return 0

    binary = args.binary or default_binary()
    if binary is None:
        print("error: no oxterm executable found; use --binary", file=sys.stderr)
        return 1
    return import_into_oxterm(binary, output, args.allow_local_commands)


if __name__ == "__main__":
    raise SystemExit(main())
