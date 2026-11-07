#!/usr/bin/env python3
"""Real CLI attendance, revocation, resident sessions and relay recovery.

Requires prebuilt headless binaries and POSIX PTYs. Invitations and captured
terminal contents are never printed or persisted in test artifacts.
"""
import errno
import os
from pathlib import Path
import re
import select
import signal
import socket
import subprocess
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
CLI = os.environ.get("LIGHTREMOTE_BIN", str(ROOT / "target/debug/lightremote"))
RELAY = os.environ.get("LIGHTREMOTE_RELAY_BIN", str(ROOT / "target/debug/lightremote-relay"))


def wait_for(check, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.025)
    raise AssertionError("bounded host condition timed out")


def stop(process):
    if process is None:
        return
    if process.poll() is None:
        process.terminate()
    try:
        return process.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.communicate()
        raise AssertionError("process did not terminate gracefully") from None


def read_until(fd, pattern, timeout=10):
    output = bytearray()
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        ready, _, _ = select.select([fd], [], [], min(0.1, max(0, deadline - time.monotonic())))
        if not ready:
            continue
        try:
            chunk = os.read(fd, 4096)
        except OSError as error:
            if error.errno == errno.EIO:
                break
            raise
        if not chunk:
            break
        output.extend(chunk)
        assert len(output) <= 65536, "unexpectedly large local console output"
        match = re.search(pattern, output)
        if match:
            return match
    # Never include the buffer: it can contain the invitation delivery.
    raise AssertionError("expected local console transition did not occur")


def attended(env, origin, decision):
    import pty

    pid, terminal = pty.fork()
    if pid == 0:
        os.execve(CLI, [CLI, "host", "--relay", origin, "--allow-shell"], env)
    client = None
    waited = False
    try:
        invitation = read_until(terminal, rb"INVITE ([^\r\n]+)[\r\n]").group(1).decode()
        client = subprocess.Popen(
            [CLI, "connect", "shell"], env=dict(env, LIGHTREMOTE_INVITE=invitation),
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        request = read_until(terminal, rb"Permission request (\d+): shell")
        request_id = request.group(1).decode()
        assert client.poll() is None, "operation started or stopped before local decision"
        if decision == "deny":
            os.write(terminal, f"deny {request_id}\n".encode())
            output, _ = client.communicate(timeout=8)
            assert client.returncode != 0 and b"LOCAL_APPROVED" not in output
            print("PASS real TTY denies an unapproved shell")
        else:
            os.write(terminal, f"approve {request_id}\n".encode())
            read_until(terminal, rb"Permission request approved")
            if decision == "approve":
                output, _ = client.communicate(input=b"stty -echo\nprintf '%s_%s\\n' LOCAL APPROVED\nexit\n", timeout=10)
                assert client.returncode == 0 and b"LOCAL_APPROVED" in output
                print("PASS real TTY approval starts the real PTY shell")
            else:
                client.stdin.write(b"stty -echo\nprintf '%s_%s\\n' SHELL READY\n")
                client.stdin.flush()
                read_until(client.stdout.fileno(), rb"SHELL_READY")
                os.write(terminal, b"revoke shell\n")
                read_until(terminal, rb"Selected permissions revoked")
                client.communicate(timeout=8)
                assert client.returncode != 0, "revoked shell was reported as successful EOF"
                print("PASS revocation terminates the live shell and fails the CLI")
        def exited():
            nonlocal waited
            result, status = os.waitpid(pid, os.WNOHANG)
            if result:
                waited = True
                return (status,)
            return None
        wait_for(exited, timeout=5)
    finally:
        stop(client)
        if not waited:
            try:
                os.kill(pid, signal.SIGTERM)
                wait_for(lambda: os.waitpid(pid, os.WNOHANG)[0], timeout=5)
            except ProcessLookupError:
                pass
        os.close(terminal)


def main():
    if os.name != "posix":
        raise SystemExit("POSIX PTY host acceptance must run on a POSIX host")
    with socket.socket() as reserve:
        reserve.bind(("127.0.0.1", 0))
        port = reserve.getsockname()[1]
    origin = f"http://127.0.0.1:{port}"
    env = dict(os.environ, LIGHTREMOTE_RELAY_TOKEN="local-host-acceptance", LIGHTREMOTE_BIND=f"127.0.0.1:{port}")
    relay = None
    host = None

    def start_relay():
        process = subprocess.Popen([RELAY], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        def healthy():
            if process.poll() is not None:
                raise AssertionError("relay stopped before health check")
            try:
                with urllib.request.urlopen(origin + "/healthz", timeout=0.25) as response:
                    return response.status == 200
            except OSError:
                return False
        wait_for(healthy)
        return process

    try:
        relay = start_relay()
        rejected = subprocess.run([CLI, "host", "--relay", origin, "--allow-shell"], env=env, stdin=subprocess.DEVNULL, capture_output=True, timeout=5)
        assert rejected.returncode != 0 and b"attended approval requires a terminal" in rejected.stderr
        assert b"INVITE " not in rejected.stdout
        print("PASS noninteractive hosts require explicit unattended policy")
        for decision in ("deny", "approve", "revoke"):
            attended(env, origin, decision)
        with tempfile.TemporaryDirectory(prefix="lightremote-resident-") as state:
            os.chmod(state, 0o700)
            invitation_file = Path(state) / "invite"
            host = subprocess.Popen([CLI, "host", "--unattended", "--stay", "--invite-file", str(invitation_file), "--relay", origin, "--allow-shell"], env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            invitations = []
            def new_invite():
                assert host.poll() is None, "resident host stopped unexpectedly"
                try:
                    value = invitation_file.read_text().strip()
                except FileNotFoundError:
                    return None
                assert invitation_file.stat().st_mode & 0o077 == 0
                return value if value and value not in invitations else None
            for _ in range(2):
                invitation = wait_for(new_invite)
                invitations.append(invitation)
                client = subprocess.run([CLI, "connect", "capabilities"], env=dict(env, LIGHTREMOTE_INVITE=invitation), capture_output=True, timeout=10)
                assert client.returncode == 0, "resident session could not connect"
            print("PASS resident host serves consecutive sessions with fresh private invitations")
            invitation = wait_for(new_invite)
            invitations.append(invitation)
            stop(relay)
            relay = start_relay()
            recovered = wait_for(new_invite, timeout=15)
            invitations.append(recovered)
            client = subprocess.run([CLI, "connect", "capabilities"], env=dict(env, LIGHTREMOTE_INVITE=recovered), capture_output=True, timeout=10)
            assert client.returncode == 0, "resident host did not recover after relay restart"
            print("PASS relay restart creates a fresh session without replaying work")
            invitation = wait_for(new_invite)
            invitations.append(invitation)
            started = time.monotonic()
            output, errors = stop(host)
            assert host.returncode == 0 and time.monotonic() - started < 5
            assert output == b"" and all(value.encode() not in errors for value in invitations)
            assert not invitation_file.exists(), "waiting invitation remained after graceful shutdown"
            host = None
            print("PASS SIGTERM cleanup removes the invitation and logs no session secret")
    finally:
        stop(host)
        stop(relay)


if __name__ == "__main__":
    main()
