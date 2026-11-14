"""Governance golden journeys (HORO-2398): policy deny, Ask without approval,
digest-only decision audit and managed-authority unavailability.

Every denial string asserted here is produced by
`crates/zeroclaw-runtime/src/security/policy_gate.rs::denial_message` (reason
codes from `policy_contract.rs::ReasonCode`, ADR-019) or by
`crates/zeroclaw-runtime/src/agent/turn/approval_gate.rs`. Pure builders and
verdicts live here so they are unit-testable without a built binary; only
`run_governance_case` needs one.
"""

from __future__ import annotations

from dataclasses import dataclass, field
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any

import golden_journey as base

# ASCII, JSON-safe and free of regex/shell metacharacters: its canonical JSON
# form is identical in Python and in `policy_contract::args_sha256`.
SENTINEL = "HORO-2398-GOV-SENTINEL-7f3a9c"
CALL_ID = "gj_gov"
TOOL = "fixture__echo"
GOV_FINAL = "GOV_FINAL"
AUDIT_FILE = "policy-audit.jsonl"
FORBIDDEN_AUDIT_KEYS = ("args", "arguments", "output", "result")
ASK_DENIAL_PREFIXES = (
    f"Tool call not executed: '{TOOL}' requires approval and no ",
    f"Denied by user. The operator was asked to approve '{TOOL}' and declined",
)


def denial_text(tool: str, code: str) -> str:
    """Verbatim `policy_gate::denial_message` output."""
    return f"Tool call not executed: '{tool}' was denied by policy ({code})."


def expected_args_sha256(text: str = SENTINEL) -> str:
    """`policy_contract::args_sha256` for `{"text": text}` (sorted keys, no spaces)."""
    canonical = json.dumps({"text": text}, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canonical.encode()).hexdigest()


# Fixture wrapped, not modified: a launcher records each tools/call it
# dispatches (a bare marker line, never arguments) so non-execution is observed
# on the fixture side, not inferred from the provider transcript.
LAUNCHER = (
    "import os,runpy,sys; open(sys.argv[1],'w').write(str(os.getpid()));"
    "ns=runpy.run_path(sys.argv[2],run_name='gj_fixture'); orig=ns['dispatch'];"
    "\ndef d(r):\n"
    "    if isinstance(r,dict) and r.get('method')=='tools/call':\n"
    "        open(sys.argv[3],'a').write('call\\n')\n"
    "    return orig(r)\n"
    "ns['serve'].__globals__['dispatch']=d; ns['serve']()"
)


def render_governance_config(
    mode: str, root: Path, server_port: int, fixture: Path, python: Path
) -> str:
    quote = json.dumps
    risk_extra = ""
    governance = ""
    if mode == "policy_deny":
        # Excluding the server name leaves the composed tool registered (the
        # registry filter matches exact names) so the engine, not the registry,
        # decides: an exclusion beats the explicit grant.
        risk_extra = f"excluded_tools = {quote(['fixture'])}\n"
    elif mode == "policy_ask_denied":
        risk_extra = f"always_ask = {quote([TOOL])}\n"
    elif mode == "managed_unavailable":
        socket = root.resolve() / "no-such-dir" / "aa-runtime-golden.sock"
        governance = (
            f'[governance]\nmode = "managed"\n'
            f"[governance.aasm]\nsocket_path = {quote(str(socket))}\n"
        )
    args = [
        "-c",
        LAUNCHER,
        str(root / "fixture.pid"),
        str(fixture),
        str(root / "fixture-calls.log"),
    ]
    return f"""schema_version = 3
[providers.models.custom.fixture]
uri = {quote(f"http://127.0.0.1:{server_port}/v1")}
model = "golden"
wire_api = "chat_completions"
native_tools = true
timeout_secs = 5
[agents.golden]
model_provider = "custom.fixture"
risk_profile = "golden"
runtime_profile = "golden"
mcp_bundles = {quote(["local_fixture"])}
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
allowed_tools = {quote([TOOL])}
auto_approve = {quote([TOOL])}
{risk_extra}[mcp]
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
{governance}"""


def tool_text(content: Any) -> str:
    return content if isinstance(content, str) else json.dumps(content)


def exact_echo(content: Any, text: str = SENTINEL) -> bool:
    if content == text:
        return True
    if isinstance(content, str):
        try:
            content = json.loads(content)
        except ValueError:
            return False
    item = {"type": "text", "text": text}
    return content == [item] or content == {"content": [item], "isError": False}


def provider_reply(body: dict[str, Any]) -> dict[str, Any]:
    answered = any(
        isinstance(m, dict) and m.get("role") == "tool" and m.get("tool_call_id") == CALL_ID
        for m in body.get("messages", [])
    )
    if answered:
        message: dict[str, Any] = {"role": "assistant", "content": GOV_FINAL}
        finish = "stop"
    else:
        message = {
            "role": "assistant",
            "tool_calls": [
                {
                    "id": CALL_ID,
                    "type": "function",
                    "function": {
                        "name": TOOL,
                        "arguments": json.dumps({"text": SENTINEL}),
                    },
                }
            ],
        }
        finish = "tool_calls"
    return {
        "id": "synthetic",
        "choices": [{"index": 0, "message": message, "finish_reason": finish}],
    }


def first_tool_result(messages: list[Any]) -> Any:
    for m in messages:
        if isinstance(m, dict) and m.get("role") == "tool" and m.get("tool_call_id") == CALL_ID:
            return m.get("content")
    return None


def audit_failure(files: dict[str, bytes], expected_digest: str) -> str | None:
    """Label of the first audit defect, or None when the log is acceptable."""
    live = files.get(AUDIT_FILE)
    if live is None:
        return "audit_missing"
    if any(SENTINEL.encode() in blob for blob in files.values()):
        return "audit_contains_sentinel"
    records = []
    for line in live.decode("utf-8", "replace").splitlines():
        if not line.strip():
            continue
        try:
            records.append(json.loads(line))
        except ValueError:
            return "audit_line_not_json"
    mine = [r for r in records if isinstance(r, dict) and r.get("tool_name") == TOOL]
    if len(mine) != 1:
        return "audit_record_count"
    record = mine[0]

    def keys(value: Any) -> set[str]:
        if isinstance(value, dict):
            return set(value) | {k for v in value.values() for k in keys(v)}
        if isinstance(value, list):
            return {k for v in value for k in keys(v)}
        return set()

    if keys(record) & set(FORBIDDEN_AUDIT_KEYS):
        return "audit_raw_field"
    decision = record.get("decision")
    authority = record.get("authority")
    decision_id = record.get("decision_id")
    checks = (
        record.get("v") == 1,
        record.get("capability") == "mcp",
        isinstance(decision, dict) and decision.get("reason") == "local_allow",
        isinstance(decision, dict) and decision.get("effective") == "allow",
        isinstance(decision_id, str) and decision_id != "",
        record.get("args_sha256") == expected_digest,
        isinstance(authority, dict) and authority.get("mode") == "standalone",
    )
    return None if all(checks) else "audit_field_mismatch"


@dataclass
class Observation:
    exit_code: int | None = None
    requests: int = 0
    tool_result: Any = None
    fixture_calls: int = 0
    final_marker: bool = False
    process_clean: bool = False
    harness_clean: bool = False
    audit_files: dict[str, bytes] = field(default_factory=dict)


def evaluate(mode: str, obs: Observation) -> str | None:
    """Failure label for a governance case, or None for a pass."""
    if not (obs.process_clean and obs.harness_clean):
        return "harness_unclean"
    if obs.requests == 0:
        # No provider call at all: startup was refused or never reached the
        # model. Fail-closed, but not the denial this journey asserts.
        return "no_provider_request"
    if obs.exit_code != 0 or obs.requests != 2 or not obs.final_marker:
        return "unexpected_completion"
    result = tool_text(obs.tool_result)
    if SENTINEL in result and mode != "audit_record":
        return "tool_ran"
    if mode == "audit_record":
        if obs.fixture_calls != 1 or not exact_echo(obs.tool_result):
            return "tool_did_not_run"
        return audit_failure(obs.audit_files, expected_args_sha256())
    if obs.fixture_calls != 0:
        return "tool_ran"
    if mode == "policy_deny":
        return None if denial_text(TOOL, "excluded") in result else "denial_mismatch"
    if mode == "managed_unavailable":
        code = "authority_unavailable"
        return None if denial_text(TOOL, code) in result else "denial_mismatch"
    if mode == "policy_ask_denied":
        ok = any(prefix in result for prefix in ASK_DENIAL_PREFIXES)
        return None if ok else "denial_mismatch"
    return "unknown_mode"


def run_governance_case(
    mode: str, binary: Path, fixture: Path, deadline: float
) -> dict[str, Any]:
    if mode not in base.GOVERNANCE_MODES or not math.isfinite(deadline) or deadline <= 0:
        return {"case": mode, "status": "invalid_input", "case_pass": False}
    if os.name != "posix":
        return {"case": mode, "status": "unsupported_platform", "case_pass": False}
    lock = threading.Lock()
    seen: dict[str, Any] = {"requests": 0, "tool_result": None, "bad": False, "auth": False}

    class Handler(BaseHTTPRequestHandler):
        def setup(self) -> None:
            super().setup()
            self.connection.settimeout(1)

        def log_message(self, _format: str, *_args: object) -> None:
            return

        def do_GET(self) -> None:
            self.send(b'{"data":[]}', "application/json")

        def do_POST(self) -> None:
            try:
                size = int(self.headers.get("Content-Length", "0"))
                if not 0 <= size <= base.MAX_REQUEST_BYTES:
                    raise ValueError
                body = json.loads(self.rfile.read(size))
                if not isinstance(body, dict) or not isinstance(body.get("messages"), list):
                    raise ValueError
                with lock:
                    seen["requests"] += 1
                    seen["auth"] |= bool(self.headers.get("Authorization"))
                    result = first_tool_result(body["messages"])
                    if result is not None and seen["tool_result"] is None:
                        seen["tool_result"] = result
                payload, kind = base.response_bytes(
                    provider_reply(body), bool(body.get("stream"))
                )
                self.send(payload, kind)
            except (ValueError, TypeError, OSError, RecursionError):
                seen["bad"] = True
                self.close_connection = True

        def send(self, payload: bytes, kind: str) -> None:
            try:
                self.send_response(200)
                self.send_header("Content-Type", kind)
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
            except OSError:
                self.close_connection = True

    class Quiet(HTTPServer):
        def handle_error(self, _request: Any, _client_address: Any) -> None:
            seen["bad"] = True

    result: dict[str, Any] = {"case": mode, "status": "runner_error", "case_pass": False}
    proc = None
    server = None
    thread = None
    cleanup = base.CleanupResult(False, False, False, False)
    with tempfile.TemporaryDirectory(prefix="local-agent-gj-gov-") as directory:
        root = Path(directory)
        for name in ("home", "workspace", "config"):
            (root / name).mkdir()
        try:
            server = Quiet(("127.0.0.1", 0), Handler)
            thread = threading.Thread(
                target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True
            )
            thread.start()
            (root / "config/config.toml").write_text(
                render_governance_config(
                    mode, root, server.server_port, fixture, Path(sys.executable)
                )
            )
            limited = False
            code = None
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
                    env=base.build_environment(root / "home"),
                    stdin=subprocess.DEVNULL,
                    stdout=out,
                    stderr=err,
                    start_new_session=True,
                )
                end = time.monotonic() + deadline
                while time.monotonic() < end:
                    if any(
                        os.fstat(s.fileno()).st_size > base.MAX_OUTPUT_BYTES
                        for s in (out, err)
                    ):
                        limited = True
                        break
                    code = proc.poll()
                    if code is not None:
                        break
                    time.sleep(0.02)
                cleanup = base.terminate_process_group(proc)
                limited |= any(
                    os.fstat(s.fileno()).st_size > base.MAX_OUTPUT_BYTES
                    for s in (out, err)
                )
            calls = root / "fixture-calls.log"
            audit = {
                p.name: p.read_bytes()
                for p in root.rglob("policy-audit.jsonl*")
                if p.is_file()
            }
            marker = False
            if code is not None:
                with (root / "stdout.log").open("rb") as stream:
                    data = stream.read(base.MAX_OUTPUT_BYTES + 1)
                marker = len(data) <= base.MAX_OUTPUT_BYTES and GOV_FINAL.encode() in data
            obs = Observation(
                exit_code=code,
                requests=seen["requests"],
                tool_result=seen["tool_result"],
                fixture_calls=len(calls.read_text().splitlines()) if calls.exists() else 0,
                final_marker=marker,
                process_clean=cleanup.parent_reaped and cleanup.group_clean,
                harness_clean=not limited and not seen["auth"] and not seen["bad"],
                audit_files=audit,
            )
            failure = "output_limit" if limited else evaluate(mode, obs)
            result = {
                "case": mode,
                "status": "passed" if failure is None else "failed",
                "case_pass": failure is None,
                "failure": failure,
                "cli_exit_code": code,
                "provider_requests": obs.requests,
                "fixture_tool_calls": obs.fixture_calls,
                "audit_files": sorted(audit),
                "children_cleaned": cleanup.group_clean,
            }
        except Exception:
            result = {"case": mode, "status": "runner_error", "case_pass": False}
        finally:
            if proc is not None:
                cleanup = base.terminate_process_group(proc)
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
