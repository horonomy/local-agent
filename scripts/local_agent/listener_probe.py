#!/usr/bin/env python3
"""Prove a running Local Agent process tree holds no listening socket.

Runs `zeroclaw agent` against a local provider that accepts the request and
then stalls, so the agent stays alive and idle for the probe. While it is
alive, every process in its tree is inspected with lsof for listening TCP
sockets and any Unix-domain socket the process itself serves. A synthetic
listener is checked first (positive control): if lsof cannot see it, the probe
fails instead of passing vacuously.

Usage: listener_probe.py --binary PATH [--hold 4]
Exit 0: control seen and no listener found. Exit 1: listener found.
Exit 2: the probe could not establish its premise (control, startup, lsof).
"""

import argparse
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

STALL_SECONDS = 30
STARTUP_DEADLINE = 25
FIRST_LAUNCH_TIMEOUT = 600


def lsof_listeners(pid: int) -> list[str]:
    """Listening TCP sockets and bound Unix-domain sockets held by pid."""
    out = subprocess.run(
        ["lsof", "-nP", "-a", "-p", str(pid), "-iTCP", "-sTCP:LISTEN", "-Fn"],
        capture_output=True,
        text=True,
    ).stdout
    tcp = [line[1:] for line in out.splitlines() if line.startswith("n")]
    out = subprocess.run(
        ["lsof", "-nP", "-a", "-p", str(pid), "-U", "-Fn"],
        capture_output=True,
        text=True,
    ).stdout
    unix = [
        "unix:" + line[1:]
        for line in out.splitlines()
        if line.startswith("n") and line[1:].startswith("/")
    ]
    return tcp + unix


def process_tree(root: int) -> list[int]:
    pids, frontier = [root], [root]
    while frontier:
        children = []
        for pid in frontier:
            out = subprocess.run(
                ["pgrep", "-P", str(pid)], capture_output=True, text=True
            ).stdout
            children += [int(p) for p in out.split()]
        pids += children
        frontier = children
    return pids


def positive_control() -> bool:
    """A real listener in a child process must be visible to lsof_listeners."""
    code = (
        "import socket,time;s=socket.socket();s.bind(('127.0.0.1',0));"
        "s.listen();print(s.getsockname()[1],flush=True);time.sleep(30)"
    )
    child = subprocess.Popen(
        [sys.executable, "-c", code], stdout=subprocess.PIPE, text=True
    )
    try:
        port = child.stdout.readline().strip()
        time.sleep(0.3)
        return bool(port) and any(port in entry for entry in lsof_listeners(child.pid))
    finally:
        child.kill()
        child.wait()


class Stall(BaseHTTPRequestHandler):
    connections = 0

    def do_POST(self):  # noqa: N802 - http.server API
        Stall.connections += 1
        time.sleep(STALL_SECONDS)

    def log_message(self, *args):  # silence request logging
        pass


def write_config(root: Path, port: int) -> None:
    (root / "workspace").mkdir()
    (root / "config").mkdir()
    (root / "config" / "config.toml").write_text(
        f"""schema_version = 3
[providers.models.custom.probe]
uri = {json.dumps(f"http://127.0.0.1:{port}/v1")}
model = "probe"
wire_api = "chat_completions"
native_tools = true
timeout_secs = {STALL_SECONDS}
[agents.probe]
model_provider = "custom.probe"
risk_profile = "probe"
runtime_profile = "probe"
[agents.probe.workspace]
path = {json.dumps(str(root / "workspace"))}
[agents.probe.memory]
backend = "none"
[runtime_profiles.probe]
agentic = true
max_tool_iterations = 1
parallel_tools = false
[risk_profiles.probe]
level = "supervised"
workspace_only = true
allowed_tools = []
auto_approve = []
"""
    )


def launch_agent(binary: Path, root: Path) -> subprocess.Popen:
    env = {
        "PATH": os.environ.get("PATH", ""),
        "HOME": str(root / "home"),
        "LC_ALL": "C",
    }
    (root / "home").mkdir()
    return subprocess.Popen(
        [
            str(binary),
            "--config-dir",
            str(root / "config"),
            "agent",
            "--agent",
            "probe",
            "--message",
            "hello",
        ],
        cwd=root,
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )


def wait_until_idle(proc: subprocess.Popen) -> bool:
    """True once the agent has connected to the stalling provider and is alive."""
    deadline = time.monotonic() + STARTUP_DEADLINE
    while time.monotonic() < deadline and Stall.connections == 0:
        if proc.poll() is not None:
            break
        time.sleep(0.1)
    return Stall.connections > 0 and proc.poll() is None


def inspect_tree(root_pid: int) -> dict[str, list[str]]:
    found = {}
    for pid in process_tree(root_pid):
        entries = lsof_listeners(pid)
        if entries:
            found[str(pid)] = entries
    return found


def stop_agent(proc: subprocess.Popen) -> None:
    if proc.poll() is not None:
        return
    for sig, grace in ((signal.SIGTERM, 10), (signal.SIGKILL, 10)):
        try:
            os.killpg(proc.pid, sig)
        except ProcessLookupError:
            return
        try:
            proc.wait(timeout=grace)
            return
        except subprocess.TimeoutExpired:
            continue


def warm_up(binary: Path) -> float:
    """Run `--version` once and return its wall time in seconds.

    macOS assesses a freshly linked binary on first launch, which can take
    minutes on a busy machine. Absorbing it here keeps that cost out of the
    startup deadline; it is reported rather than hidden.
    """
    started = time.monotonic()
    subprocess.run(
        [str(binary), "--version"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        timeout=FIRST_LAUNCH_TIMEOUT,
        check=False,
    )
    return time.monotonic() - started


def probe(binary: Path, hold: float) -> tuple[dict, int]:
    """Run one idle agent and return (report, exit code)."""
    server = ThreadingHTTPServer(("127.0.0.1", 0), Stall)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    root = Path(tempfile.mkdtemp(prefix="listener-probe-"))
    proc = None
    try:
        first_launch = warm_up(binary)
        write_config(root, server.server_address[1])
        proc = launch_agent(binary, root)
        if not wait_until_idle(proc):
            return {"verdict": "premise-failed", "reason": "agent not idle"}, 2
        time.sleep(hold)
        found = inspect_tree(proc.pid)
        report = {
            "verdict": "listener-found" if found else "no-listener",
            "control_seen": True,
            "first_launch_s": round(first_launch, 1),
            "provider_connections": Stall.connections,
            "agent_alive": proc.poll() is None,
            "listeners": found,
        }
        return report, 1 if found else 0
    finally:
        if proc is not None:
            stop_agent(proc)
        server.shutdown()
        shutil.rmtree(root, ignore_errors=True)


def validated_binary(path: Path) -> Path | None:
    binary = path.resolve()
    if binary.is_file() and os.access(binary, os.X_OK):
        return binary
    return None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--hold", type=float, default=4.0)
    args = parser.parse_args()

    premise = None
    binary = validated_binary(args.binary)
    if binary is None:
        premise = "binary is not an executable file"
    elif shutil.which("lsof") is None:
        premise = "lsof missing"
    elif not positive_control():
        premise = "control unseen"
    if premise is not None:
        print(json.dumps({"verdict": "premise-failed", "reason": premise}))
        return 2

    report, code = probe(binary, args.hold)
    print(json.dumps(report, sort_keys=True))
    return code


if __name__ == "__main__":
    sys.exit(main())
