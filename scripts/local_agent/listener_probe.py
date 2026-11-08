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
        return any(port in entry for entry in lsof_listeners(child.pid))
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


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--hold", type=float, default=4.0)
    args = parser.parse_args()

    if shutil.which("lsof") is None:
        print(json.dumps({"verdict": "premise-failed", "reason": "lsof missing"}))
        return 2
    if not positive_control():
        print(json.dumps({"verdict": "premise-failed", "reason": "control unseen"}))
        return 2

    server = ThreadingHTTPServer(("127.0.0.1", 0), Stall)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    root = Path(tempfile.mkdtemp(prefix="listener-probe-"))
    proc = None
    try:
        write_config(root, server.server_address[1])
        env = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": str(root / "home"),
            "LC_ALL": "C",
        }
        (root / "home").mkdir()
        proc = subprocess.Popen(
            [
                args.binary,
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
        deadline = time.monotonic() + STARTUP_DEADLINE
        while time.monotonic() < deadline and Stall.connections == 0:
            if proc.poll() is not None:
                break
            time.sleep(0.1)
        if Stall.connections == 0 or proc.poll() is not None:
            print(json.dumps({"verdict": "premise-failed", "reason": "agent not idle"}))
            return 2
        time.sleep(args.hold)
        found = {}
        for pid in process_tree(proc.pid):
            entries = lsof_listeners(pid)
            if entries:
                found[str(pid)] = entries
        print(
            json.dumps(
                {
                    "verdict": "listener-found" if found else "no-listener",
                    "control_seen": True,
                    "provider_connections": Stall.connections,
                    "agent_alive": proc.poll() is None,
                    "listeners": found,
                },
                sort_keys=True,
            )
        )
        return 1 if found else 0
    finally:
        if proc is not None and proc.poll() is None:
            try:
                os.killpg(proc.pid, 15)
            except ProcessLookupError:
                pass
            proc.wait(timeout=10)
        server.shutdown()
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
