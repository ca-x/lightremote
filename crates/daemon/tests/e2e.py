#!/usr/bin/env python3
"""Real Linux relay + daemon + CLI smoke. Build the three binaries first."""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time

root = Path(__file__).resolve().parents[3]
binaries = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else root / "target/debug"
token = "private-daemon-e2e-operator"
env = dict(os.environ, LIGHTREMOTE_RELAY_TOKEN=token)


def wait_for(read, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = read()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError("daemon smoke deadline expired")


def stop(process):
    if process.poll() is None:
        process.terminate()
    try:
        return process.communicate(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.communicate()
        raise AssertionError("process did not stop gracefully")


with tempfile.TemporaryDirectory(prefix="lightremote-daemon-e2e-") as temporary:
    directory = Path(temporary)
    with socket.socket() as reserved:
        reserved.bind(("127.0.0.1", 0))
        port = reserved.getsockname()[1]
    relay = subprocess.Popen(
        [str(binaries / "lightremote-relay")],
        env=dict(env, LIGHTREMOTE_BIND=f"127.0.0.1:{port}"),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    daemon = None
    try:
        config = directory / "daemon.json"
        state = directory / "state"
        config.write_text(json.dumps({
            "relay": f"http://127.0.0.1:{port}",
            "web_url": "http://127.0.0.1:5173/",
            "state_dir": str(state),
            "policy_confirmed": True,
        }))
        config.chmod(0o600)
        daemon = subprocess.Popen(
            [str(binaries / "lightremote-daemon"), "--config", str(config)],
            env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        previous = ""
        invitations = []
        for session in range(2):
            def fresh_invite():
                try:
                    value = (state / "invite").read_text().strip()
                    return value if value != previous else None
                except FileNotFoundError:
                    return None
            invitation = wait_for(fresh_invite)
            invitations.append(invitation)
            assert (state.stat().st_mode & 0o777) == 0o700
            assert ((state / "invite").stat().st_mode & 0o777) == 0o600
            client = subprocess.run(
                [str(binaries / "lightremote"), "connect", "capabilities"],
                env=dict(env, LIGHTREMOTE_INVITE=invitation),
                capture_output=True, timeout=20,
            )
            assert client.returncode == 0, client.stderr.decode(errors="replace")
            capabilities = json.loads(client.stdout)
            for capability in ("files", "shell", "desktop", "input", "clipboard"):
                assert not capabilities.get(capability), capability
            previous = invitation
        stdout, stderr = stop(daemon)
        assert daemon.returncode == 0, stderr.decode(errors="replace")
        assert not (state / "invite").exists()
        assert not stdout
        for secret in [token, *invitations]:
            assert secret.encode() not in stderr
        print("PASS real relay + daemon: two fresh sessions, permissions default off, private invites, SIGTERM cleanup, redacted logs")
    finally:
        if daemon is not None and daemon.poll() is None:
            stop(daemon)
        stop(relay)
