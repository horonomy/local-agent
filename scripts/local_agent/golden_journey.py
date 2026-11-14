from __future__ import annotations

import argparse
from dataclasses import dataclass, field
import json
import math
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any

# JSON protocol bodies have heterogeneous shapes; validate each field at the boundary.
MARKER = "HORO-2378-STDIO-ROUNDTRIP"
GRANTED_FINAL = "STDIO_GRANTED_FINAL"
DENIED_FINAL = "STDIO_NOT_GRANTED_FINAL"
MODES = ("granted", "ungranted", "provider_timeout", "cancellation", "recovery")
# Governance journeys (HORO-2398) live in governance_journey.py.
GOVERNANCE_MODES = (
    "policy_deny",
    "policy_ask_denied",
    "audit_record",
    "managed_unavailable",
)
ALL_CASES = MODES + GOVERNANCE_MODES
MAX_REQUEST_BYTES = 1024 * 1024
MAX_RESPONSE_BYTES = 1024 * 1024
MAX_OUTPUT_BYTES = 1024 * 1024


def positive_deadline(value: str) -> float:
    deadline = float(value)
    if not math.isfinite(deadline) or deadline <= 0:
        raise ValueError("deadline must be finite and positive")
    return deadline


def build_environment(
    home: Path, _parent: dict[str, str] | None = None
) -> dict[str, str]:
    return {
        "HOME": str(home),
        "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
        "TMPDIR": tempfile.gettempdir(),
        "LANG": "C",
        "NO_COLOR": "1",
    }


def render_config(
    root: Path,
    server_port: int,
    fixture: Path,
    python: Path,
    granted: bool,
    provider_timeout: int,
    *,
    recovery: bool = False,
) -> str:
    quote = json.dumps
    tools = ["fixture__echo", "fixture__fail"] if recovery else ["fixture__echo"]
    # The child itself records its identity before executing the unmodified fixture.
    launcher = "import os,runpy,sys; open(sys.argv[1],'w').write(str(os.getpid())); runpy.run_path(sys.argv[2],run_name='__main__')"
    args = ["-c", launcher, str(root / "fixture.pid"), str(fixture)]
    return f"""schema_version = 3
[providers.models.custom.fixture]
uri = {quote(f"http://127.0.0.1:{server_port}/v1")}
model = "golden"
wire_api = "chat_completions"
native_tools = true
timeout_secs = {provider_timeout}
[agents.golden]
model_provider = "custom.fixture"
risk_profile = "golden"
runtime_profile = "golden"
mcp_bundles = {quote(["local_fixture"] if granted else [])}
[agents.golden.workspace]
path = {quote(str(root / "workspace"))}
[agents.golden.memory]
backend = "none"
[runtime_profiles.golden]
agentic = true
max_tool_iterations = 3
parallel_tools = false
[risk_profiles.golden]
level = "supervised"
workspace_only = true
allowed_tools = {quote(tools)}
auto_approve = {quote(tools)}
[mcp]
enabled = true
[[mcp.servers]]
name = "fixture"
transport = "stdio"
command = {quote(str(python))}
args = {quote(args)}
tool_timeout_secs = 5
[mcp_bundles.local_fixture]
servers = ["fixture"]
[reliability]
provider_retries = 0
"""


def exact_fixture_result(content: Any) -> bool:
    if content == MARKER:
        return True
    if isinstance(content, str):
        try:
            content = json.loads(content)
        except ValueError:
            return False
    item = {"type": "text", "text": MARKER}
    return content == [item] or content == {"content": [item], "isError": False}


def is_exact_tool_result(tool_call_id: Any, content: Any) -> bool:
    return tool_call_id == "gj_mcp" and exact_fixture_result(content)


def is_fixture_failure(tool_call_id: Any, content: Any) -> bool:
    if tool_call_id != "gj_fail":
        return False
    if isinstance(content, str):
        if (
            content
            == "Error: MCP `fail` (server `fixture`) returned isError: synthetic tool failure"
        ):
            return True
        try:
            content = json.loads(content)
        except ValueError:
            return content == "synthetic tool failure"
    item = {"type": "text", "text": "synthetic tool failure"}
    return content == [item] or content == {"content": [item], "isError": True}


def recovery_completed(messages: list[Any]) -> bool:
    failed = False
    for item in messages:
        if not isinstance(item, dict) or item.get("role") != "tool":
            continue
        call_id, content = item.get("tool_call_id"), item.get("content")
        if failed and is_exact_tool_result(call_id, content):
            return True
        failed |= is_fixture_failure(call_id, content)
    return False


def provider_reply(
    body: dict[str, Any], granted: bool, *, recovery: bool = False
) -> dict[str, Any]:
    messages = body.get("messages", [])
    failed = recovery and any(
        isinstance(item, dict)
        and item.get("role") == "tool"
        and is_fixture_failure(item.get("tool_call_id"), item.get("content"))
        for item in messages
    )
    completed = (
        recovery_completed(messages)
        if recovery
        else any(
            isinstance(item, dict)
            and item.get("role") == "tool"
            and is_exact_tool_result(item.get("tool_call_id"), item.get("content"))
            for item in messages
        )
    )
    if completed:
        message = {
            "role": "assistant",
            "content": GRANTED_FINAL if granted else DENIED_FINAL,
        }
        finish = "stop"
    else:
        call_failure = recovery and not failed
        message = {
            "role": "assistant",
            "tool_calls": [
                {
                    "id": "gj_fail" if call_failure else "gj_mcp",
                    "type": "function",
                    "function": {
                        "name": "fixture__fail" if call_failure else "fixture__echo",
                        "arguments": json.dumps(
                            {} if call_failure else {"text": MARKER}
                        ),
                    },
                }
            ],
        }
        finish = "tool_calls"
    return {
        "id": "synthetic",
        "choices": [{"index": 0, "message": message, "finish_reason": finish}],
    }


def response_bytes(body: dict[str, Any], streaming: bool) -> tuple[bytes, str]:
    if not streaming:
        return json.dumps(body).encode(), "application/json"
    choice = body["choices"][0]
    delta = {key: value for key, value in choice["message"].items() if key != "role"}
    if "tool_calls" in delta:
        delta["tool_calls"] = [
            {**call, "index": index} for index, call in enumerate(delta["tool_calls"])
        ]
    event = {
        "choices": [
            {"index": 0, "delta": delta, "finish_reason": choice["finish_reason"]}
        ]
    }
    return (
        "data: " + json.dumps(event) + "\n\ndata: [DONE]\n\n"
    ).encode(), "text/event-stream"


@dataclass(frozen=True)
class CleanupResult:
    parent_reaped: bool
    term_sent: bool
    kill_sent: bool
    group_clean: bool


def live_group_members(pgid: int) -> list[tuple[int, str]] | None:
    ps = shutil.which("ps")
    if ps is None:
        return None
    try:
        result = subprocess.run(
            [ps, "-axo", "pid=,pgid=,stat="],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            check=True,
            timeout=1,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    return [
        (int(values[0]), values[2])
        for line in result.stdout.splitlines()
        if len(values := line.split()) == 3
        and values[0].isdigit()
        and values[1].isdigit()
        and int(values[1]) == pgid
        and not values[2].startswith("Z")
    ]


def terminate_process_group(
    process: subprocess.Popen[bytes], grace: float = 2.0
) -> CleanupResult:
    if os.name != "posix":
        raise NotImplementedError("owned process groups require POSIX")
    sent = []
    for sig, delay in [(signal.SIGTERM, grace), (signal.SIGKILL, 2.0)]:
        members = live_group_members(process.pid)
        if members == []:
            break
        try:
            os.killpg(process.pid, sig)
            sent.append(sig)
        except ProcessLookupError:
            break
        except PermissionError:
            break
        end = time.monotonic() + delay
        while time.monotonic() < end:
            if live_group_members(process.pid) == []:
                break
            time.sleep(0.02)
    try:
        process.wait(timeout=2)
        reaped = True
    except subprocess.TimeoutExpired:
        reaped = False
    return CleanupResult(
        reaped,
        signal.SIGTERM in sent,
        signal.SIGKILL in sent,
        live_group_members(process.pid) == [],
    )


@dataclass
class ProviderState:
    request_seen: threading.Event = field(default_factory=threading.Event)
    second_request: threading.Event = field(default_factory=threading.Event)
    release_wait: threading.Event = field(default_factory=threading.Event)
    lock: Any = field(
        default_factory=threading.Lock
    )  # Thread lock is an opaque synchronization primitive.
    requests: int = 0
    advertised: bool = False
    returned: bool = False
    fail_advertised: bool = False
    recovery_observed: bool = False
    auth_seen: bool = False
    bad_request: bool = False
    held_at: float | None = None


def observed_provider_timeout(
    code: int | None,
    held_at: float | None,
    exited_at: float | None,
    timeout: float,
    deadline: float,
) -> bool:
    return bool(
        code is not None
        and code > 0
        and held_at is not None
        and exited_at is not None
        and timeout <= exited_at - held_at
        and exited_at < deadline
    )


def bounded_final_marker(output: Path) -> bool:
    with output.open("rb") as stream:
        content = stream.read(MAX_OUTPUT_BYTES + 1)
    return len(content) <= MAX_OUTPUT_BYTES and GRANTED_FINAL.encode() in content


def run_case(mode: str, binary: Path, fixture: Path, deadline: float) -> dict[str, Any]:
    if mode in GOVERNANCE_MODES:
        from governance_journey import run_governance_case

        return run_governance_case(mode, binary, fixture, deadline)
    if mode not in MODES or not math.isfinite(deadline) or deadline <= 0:
        return {"case": mode, "status": "invalid_input", "case_pass": False}
    if os.name != "posix":
        return {"case": mode, "status": "unsupported_platform", "case_pass": False}
    state = ProviderState()
    proc = None
    server = None
    thread = None
    cleanup = CleanupResult(False, False, False, False)
    code = None
    exited_at = None
    fixture_observed = False
    cancelled = False
    output_limited = False
    result = {"case": mode, "status": "runner_error", "case_pass": False}
    with tempfile.TemporaryDirectory(prefix="local-agent-gj-") as directory:
        root = Path(directory)
        for name in ("home", "workspace", "config"):
            (root / name).mkdir()
        try:

            class Handler(BaseHTTPRequestHandler):
                def setup(self) -> None:
                    super().setup()
                    self.connection.settimeout(1)

                def log_message(self, _format: str, *_args: object) -> None:
                    return

                def do_GET(self) -> None:
                    self.send_payload(b'{"data":[]}', "application/json")

                def do_POST(self) -> None:
                    try:
                        size = int(self.headers.get("Content-Length", "0"))
                        if not 0 <= size <= MAX_REQUEST_BYTES:
                            raise ValueError
                        body = json.loads(self.rfile.read(size))
                        if (
                            not isinstance(body, dict)
                            or not isinstance(body.get("messages"), list)
                            or not isinstance(body.get("tools", []), list)
                        ):
                            raise ValueError
                        returned = any(
                            isinstance(item, dict)
                            and item.get("role") == "tool"
                            and is_exact_tool_result(
                                item.get("tool_call_id"), item.get("content")
                            )
                            for item in body["messages"]
                        )
                        names = {
                            item["function"].get("name")
                            for item in body.get("tools", [])
                            if isinstance(item, dict)
                            and isinstance(item.get("function"), dict)
                        }
                        with state.lock:
                            state.requests += 1
                            state.advertised |= "fixture__echo" in names
                            state.returned |= returned
                            state.fail_advertised |= "fixture__fail" in names
                            state.recovery_observed |= recovery_completed(
                                body["messages"]
                            )
                            state.auth_seen |= bool(self.headers.get("Authorization"))
                            state.request_seen.set()
                            if returned:
                                state.held_at = time.monotonic()
                                state.second_request.set()
                        if returned and mode in ("provider_timeout", "cancellation"):
                            state.release_wait.wait(timeout=deadline + 1)
                            return
                        payload, content_type = response_bytes(
                            provider_reply(
                                body, mode != "ungranted", recovery=mode == "recovery"
                            ),
                            bool(body.get("stream")),
                        )
                        self.send_payload(payload, content_type)
                    except (ValueError, TypeError, OSError, RecursionError):
                        state.bad_request = True
                        self.close_connection = True

                def send_payload(self, payload: bytes, content_type: str) -> None:
                    if len(payload) > MAX_RESPONSE_BYTES:
                        state.bad_request = True
                        return
                    try:
                        self.send_response(200)
                        self.send_header("Content-Type", content_type)
                        self.send_header("Content-Length", str(len(payload)))
                        self.end_headers()
                        self.wfile.write(payload)
                    except OSError:
                        self.close_connection = True

            class QuietServer(HTTPServer):
                def handle_error(self, _request: Any, _client_address: Any) -> None:
                    state.bad_request = True

            server = QuietServer(("127.0.0.1", 0), Handler)
            thread = threading.Thread(
                target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True
            )
            thread.start()
            timeout = 1 if mode == "provider_timeout" else 5
            (root / "config/config.toml").write_text(
                render_config(
                    root,
                    server.server_port,
                    fixture,
                    Path(sys.executable),
                    mode != "ungranted",
                    timeout,
                    recovery=mode == "recovery",
                )
            )
            with (
                (root / "stdout.log").open("wb") as out,
                (root / "stderr.log").open("wb") as err,
            ):
                proc = subprocess.Popen(
                    [
                        str(binary),
                        "--config-dir",
                        str(root / "config"),
                        "agent",
                        "--agent",
                        "golden",
                        "--message",
                        "Perform the synthetic echo operation if granted.",
                    ],
                    cwd=root,
                    env=build_environment(root / "home"),
                    stdout=out,
                    stderr=err,
                    start_new_session=True,
                )
                end = time.monotonic() + deadline
                while time.monotonic() < end:
                    if any(
                        os.fstat(stream.fileno()).st_size > MAX_OUTPUT_BYTES
                        for stream in (out, err)
                    ):
                        output_limited = True
                        break
                    pid_file = root / "fixture.pid"
                    if pid_file.is_file():
                        try:
                            child_pid = int(pid_file.read_text())
                            members = live_group_members(proc.pid)
                            fixture_observed |= members is not None and any(
                                pid == child_pid and pid != proc.pid
                                for pid, _ in members
                            )
                        except (OSError, ValueError):
                            pass
                    code = proc.poll()
                    if code is not None:
                        exited_at = time.monotonic()
                        break
                    if (
                        mode == "cancellation"
                        and state.second_request.is_set()
                        and fixture_observed
                    ):
                        cleanup = terminate_process_group(proc)
                        cancelled = True
                        code = proc.returncode
                        break
                    time.sleep(0.02)
                cleanup = terminate_process_group(proc) if not cancelled else cleanup
                output_limited |= any(
                    os.fstat(stream.fileno()).st_size > MAX_OUTPUT_BYTES
                    for stream in (out, err)
                )
            common = (
                cleanup.parent_reaped
                and cleanup.group_clean
                and not output_limited
                and not state.auth_seen
                and not state.bad_request
            )
            if mode == "granted":
                passed = (
                    common
                    and code == 0
                    and state.requests == 2
                    and state.advertised
                    and state.returned
                    and bounded_final_marker(root / "stdout.log")
                )
            elif mode == "recovery":
                passed = (
                    common
                    and code == 0
                    and state.requests == 3
                    and state.advertised
                    and state.fail_advertised
                    and state.recovery_observed
                    and fixture_observed
                    and bounded_final_marker(root / "stdout.log")
                )
            elif mode == "ungranted":
                passed = (
                    common
                    and code == 0
                    and state.requests == 1
                    and not state.advertised
                    and not state.returned
                    and not (root / "fixture.pid").exists()
                )
            elif mode == "provider_timeout":
                passed = (
                    common
                    and state.requests == 2
                    and state.returned
                    and state.advertised
                    and fixture_observed
                    and observed_provider_timeout(
                        code, state.held_at, exited_at, timeout, end
                    )
                )
            else:
                passed = (
                    common
                    and cancelled
                    and cleanup.term_sent
                    and state.requests == 2
                    and state.returned
                    and fixture_observed
                )
            result = {
                "case": mode,
                "status": "output_limit"
                if output_limited
                else ("passed" if passed else "failed"),
                "case_pass": bool(passed),
                "cli_exit_code": code,
                "provider_requests": state.requests,
                "echo_advertised": state.advertised,
                "exact_tool_result": state.returned,
                "recovery_observed": state.recovery_observed,
                "fixture_observed": fixture_observed,
                "children_cleaned": cleanup.group_clean,
                "term_sent": cleanup.term_sent,
                "kill_fallback_sent": cleanup.kill_sent,
            }
        except Exception:
            result = {"case": mode, "status": "runner_error", "case_pass": False}
        finally:
            state.release_wait.set()
            if proc is not None:
                cleanup = terminate_process_group(proc)
            if server is not None:
                server.shutdown()
                server.server_close()
            if thread is not None:
                thread.join(timeout=2)
                if thread.is_alive():
                    result["case_pass"] = False
                    result["status"] = "server_cleanup_failed"
            if proc is not None and not cleanup.group_clean:
                result["case_pass"] = False
                result["status"] = "process_cleanup_failed"
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Run local POSIX real-binary MCP journeys"
    )
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument(
        "--fixture", type=Path, default=Path(__file__).with_name("mcp_fixture.py")
    )
    parser.add_argument("--case", choices=("all", *ALL_CASES), default="all")
    parser.add_argument("--deadline", default="40")
    args = parser.parse_args(argv)
    try:
        deadline = positive_deadline(args.deadline)
        binary, fixture = args.binary.resolve(), args.fixture.resolve()
        if (
            not binary.is_file()
            or not os.access(binary, os.X_OK)
            or not fixture.is_file()
        ):
            raise ValueError
    except (ValueError, OSError):
        print("invalid_input")
        return 2
    results = [
        run_case(mode, binary, fixture, deadline)
        for mode in (ALL_CASES if args.case == "all" else (args.case,))
    ]
    print(
        json.dumps(
            {"cases": results, "case_pass": all(item["case_pass"] for item in results)},
            sort_keys=True,
        )
    )
    return 0 if all(item["case_pass"] for item in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
