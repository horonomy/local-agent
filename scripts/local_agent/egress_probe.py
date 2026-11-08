#!/usr/bin/env python3
"""Check which endpoints a Local Agent run can reach, without touching the network.

The agent runs under macOS sandbox-exec with all non-loopback outbound IP
denied (the host network stays up). Two loopback recorders stand in for the
approved provider and for an endpoint nobody approved. The modes cover the
configured alias, an unresolved alias, and the CLI overrides that bypass the
config.

Usage: egress_probe.py --binary PATH --mode MODE
  MODE: approved | unapproved-alias | keyed-baseline | override-custom
        | override-models-refresh
Exit 0: PASS. Exit 1: FAIL (unapproved egress or acceptance). Exit 2: premise
failed (sandbox unavailable, control failed, bad binary).

A sandbox denial is not observable from outside, so attempts are inferred from
the recorders and from error text, and are reported as inference. Denying the
system resolver's socket also breaks loopback connects under this profile, so
it stays reachable: a hostname lookup can still happen, but no connection to a
non-loopback address can succeed.
"""

import argparse
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

import listener_probe as lp

SANDBOX = "/usr/bin/sandbox-exec"
PROFILE = (
    "(version 1)(allow default)"
    "(deny network-outbound (remote ip))"
    '(allow network-outbound (remote ip "localhost:*"))'
)
RUN_TIMEOUT = 40
ATTEMPT = re.compile(
    r"cannot reach the model provider|operation not permitted|os error 1\b|dns error"
    r"|failed to lookup|nodename nor servname",
    re.IGNORECASE,
)
# Host only: userinfo (`user:key@`) is stripped so a credential is never reported.
ENDPOINT = re.compile(r"model provider at https?://(?:[^/@\s]*@)?([^/\s]+)", re.IGNORECASE)
REFUSAL = "must name a provider configured"
UNRESOLVED = "does not resolve"
MODES = ("approved", "unapproved-alias", "keyed-baseline", "override-custom",
         "override-models-refresh")
KEYED = "openai.default"  # a keyed provider family with no uri: its public endpoint


class Recorder:
    """Counts TCP connections accepted on a loopback port."""

    def __init__(self) -> None:
        self.sock = socket.socket()
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen()
        self.port = self.sock.getsockname()[1]
        self.seen = 0
        threading.Thread(target=self._loop, daemon=True).start()

    def _loop(self) -> None:
        while True:
            try:
                conn, _ = self.sock.accept()
            except OSError:
                return
            self.seen += 1
            conn.close()

    def close(self) -> None:
        self.sock.close()


def sandboxed(argv: list[str], **kwargs) -> subprocess.Popen:
    return subprocess.Popen([SANDBOX, "-p", PROFILE, *argv], **kwargs)


def control(approved: Recorder) -> bool:
    """Sandbox must block a TEST-NET connect (EPERM) yet allow the recorder."""
    code = (
        "import socket,sys,errno\n"
        "a=socket.socket();a.settimeout(3)\n"
        "try:\n a.connect(('192.0.2.1',9));blocked=False\n"
        "except OSError as e:\n blocked=(e.errno==errno.EPERM)\n"
        f"b=socket.socket();b.settimeout(3);b.connect(('127.0.0.1',{approved.port}))\n"
        "sys.exit(0 if blocked else 1)\n"
    )
    done = sandboxed([sys.executable, "-c", code], stdout=subprocess.DEVNULL,
                     stderr=subprocess.DEVNULL)
    try:
        return done.wait(timeout=30) == 0 and approved.seen >= 1
    except subprocess.TimeoutExpired:
        done.kill()
        done.wait()
        return False


def write_config(root: Path, provider: str | None, approved_port: int) -> None:
    line = f'model_provider = "{provider}"\n' if provider else ""
    (root / "config").mkdir()
    (root / "workspace").mkdir()
    (root / "home").mkdir()
    (root / "config" / "config.toml").write_text(
        "schema_version = 3\n"
        "[providers.models.custom.approved]\n"
        f'uri = "http://127.0.0.1:{approved_port}/v1"\n'
        'model = "approved-model"\nwire_api = "chat_completions"\n'
        'api_key = "synthetic-test-key"\ntimeout_secs = 3\n'
        "[providers.models.openai.default]\n"
        'api_key = "synthetic-test-key"\nmodel = "keyed-model"\ntimeout_secs = 3\n'
        f"[agents.probe]\n{line}"
        'risk_profile = "probe"\nruntime_profile = "probe"\n'
        f'[agents.probe.workspace]\npath = "{root / "workspace"}"\n'
        '[agents.probe.memory]\nbackend = "none"\n'
        "[runtime_profiles.probe]\nagentic = true\nmax_tool_iterations = 1\n"
        '[risk_profiles.probe]\nlevel = "supervised"\nworkspace_only = true\n'
        "allowed_tools = []\nauto_approve = []\n"
    )


def command_args(binary: Path, root: Path, command: list[str]) -> list[str]:
    return [str(binary), "--config-dir", str(root / "config"), *command]


def run_agent(binary: Path, root: Path, command: list[str]) -> tuple[int, str]:
    env = {"PATH": os.environ.get("PATH", ""), "HOME": str(root / "home"), "LC_ALL": "C"}
    proc = sandboxed(command_args(binary, root, command), cwd=root, env=env,
                     stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                     start_new_session=True, text=True)
    try:
        _, err = proc.communicate(timeout=RUN_TIMEOUT)
    except subprocess.TimeoutExpired:
        lp.stop_agent(proc)
        try:
            _, err = proc.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            err = ""
    return proc.returncode, err or ""


def verdict(mode: str, code: int, err: str, approved: Recorder, other: Recorder) -> dict:
    attempt = bool(ATTEMPT.search(err))
    named = ENDPOINT.search(err)
    base = {"mode": mode, "exit_code": code, "approved_connections": approved.seen,
            "other_connections": other.seen, "outbound_attempt_inferred": attempt,
            "endpoint_named_in_error": named.group(1) if named else None}
    if mode == "approved":
        ok = approved.seen > 0 and not attempt and other.seen == 0
    elif mode == "unapproved-alias":
        ok = (code != 0 and UNRESOLVED in err and approved.seen == 0
              and other.seen == 0 and not attempt)
    elif mode == "keyed-baseline":
        # Control for the next mode: the keyed provider reaches for its public
        # endpoint, which the sandbox denies, so the attempt must be inferable.
        ok = attempt and approved.seen == 0 and other.seen == 0
    else:  # override-*: a CLI URL must be refused outright, never followed
        ok = (code != 0 and REFUSAL.lower() in err.lower() and other.seen == 0
              and approved.seen == 0 and not attempt)
    return {**base, "verdict": "PASS" if ok else "FAIL"}


def agent_command(extra: list[str]) -> list[str]:
    return ["agent", "--agent", "probe", *extra, "--message", "hello"]


def mode_inputs(mode: str, other: Recorder) -> tuple[str | None, list[str]]:
    """Agent provider reference and the command line (after --config-dir)."""
    # `localhost`, not 127.0.0.1: the override URL parser reads an all-numeric
    # host as an integer address (127.0.0.1 became 0.0.0.127 in testing).
    override = ["--model-provider", f"custom:http://localhost:{other.port}"]
    if mode == "unapproved-alias":
        return "custom.ghost", agent_command([])
    if mode == "keyed-baseline":
        return KEYED, agent_command([])
    if mode == "override-custom":
        return KEYED, agent_command(override)
    if mode == "override-models-refresh":
        return KEYED, ["models", "refresh", *override]
    return "custom.approved", agent_command([])


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--mode", required=True, choices=MODES)
    args = parser.parse_args()

    binary = lp.validated_binary(args.binary)
    premise = None
    if binary is None:
        premise = "binary is not an executable file"
    elif not os.access(SANDBOX, os.X_OK):
        premise = "sandbox-exec unavailable"
    if premise:
        print(json.dumps({"verdict": "premise-failed", "reason": premise}))
        return 2

    approved, other, calibration = Recorder(), Recorder(), Recorder()
    root = Path(tempfile.mkdtemp(prefix="egress-probe-"))
    try:
        if not control(calibration):
            print(json.dumps({"verdict": "premise-failed", "reason": "sandbox control"}))
            return 2
        lp.warm_up(binary)
        provider, command = mode_inputs(args.mode, other)
        write_config(root, provider, approved.port)
        code, err = run_agent(binary, root, command)
        time.sleep(0.3)
        report = verdict(args.mode, code, err, approved, other)
        print(json.dumps(report, sort_keys=True))
        return 0 if report["verdict"] == "PASS" else 1
    finally:
        approved.close()
        other.close()
        calibration.close()
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
