"""Subprocess-level tests for the synthetic newline-delimited MCP fixture."""

from __future__ import annotations

import json
import os
import select
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any

# Decoded JSON is intentionally dynamic: assertions narrow concrete response shapes.
FIXTURE = Path(__file__).with_name("mcp_fixture.py")


def run_fixture(*lines: bytes) -> list[Any]:
    completed = subprocess.run(
        [sys.executable, str(FIXTURE)],
        input=b"".join(lines),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=5,
        check=False,
    )
    if completed.returncode != 0:
        raise AssertionError(
            f"fixture exited {completed.returncode}; stderr bytes={len(completed.stderr)}"
        )
    return [json.loads(line) for line in completed.stdout.splitlines()]


def request(request_id: int, method: str, params: object | None = None) -> bytes:
    message: dict[str, object] = {"jsonrpc": "2.0", "id": request_id, "method": method}
    if params is not None:
        message["params"] = params
    return json.dumps(message, separators=(",", ":")).encode() + b"\n"


class McpFixtureTests(unittest.TestCase):
    def test_initialize_advertises_protocol_and_tools(self) -> None:
        [response] = run_fixture(request(1, "initialize", {}))
        self.assertEqual(response["id"], 1)
        self.assertEqual(response["jsonrpc"], "2.0")
        self.assertEqual(response["result"]["protocolVersion"], "2024-11-05")
        self.assertEqual(response["result"]["capabilities"], {"tools": {}})

    def test_tools_list_and_echo_call_over_one_live_stdio_session(self) -> None:
        outputs = run_fixture(
            request(1, "initialize", {}),
            b'{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}\n',
            request(2, "tools/list", {}),
            request(3, "tools/call", {"name": "echo", "arguments": {"text": "smoke"}}),
        )
        self.assertEqual([item["id"] for item in outputs], [1, 2, 3])
        self.assertEqual(outputs[1]["result"]["tools"][0]["name"], "echo")
        self.assertEqual(outputs[2]["result"]["content"][0]["text"], "smoke")
        self.assertFalse(outputs[2]["result"]["isError"])

    def test_tool_error_then_echo_preserves_the_live_stdio_session(self) -> None:
        outputs = run_fixture(
            request(1, "initialize", {}),
            request(2, "tools/list", {}),
            request(3, "tools/call", {"name": "fail", "arguments": {}}),
            request(
                4,
                "tools/call",
                {"name": "echo", "arguments": {"text": "recovered"}},
            ),
        )
        self.assertEqual([item["id"] for item in outputs], [1, 2, 3, 4])
        self.assertEqual(
            [tool["name"] for tool in outputs[1]["result"]["tools"]],
            ["echo", "fail"],
        )
        self.assertEqual(
            outputs[2]["result"],
            {
                "content": [{"type": "text", "text": "synthetic tool failure"}],
                "isError": True,
            },
        )
        self.assertEqual(
            outputs[3]["result"],
            {"content": [{"type": "text", "text": "recovered"}], "isError": False},
        )

    def test_failure_tool_is_stateless_and_refuses_extra_arguments(self) -> None:
        outputs = run_fixture(
            request(1, "tools/call", {"name": "fail", "arguments": {}}),
            request(2, "tools/call", {"name": "fail", "arguments": {}}),
            request(3, "tools/call", {"name": "fail", "arguments": {"text": "x"}}),
        )
        self.assertEqual(outputs[0]["result"], outputs[1]["result"])
        self.assertTrue(outputs[0]["result"]["isError"])
        self.assertEqual(
            outputs[2]["error"],
            {"code": -32602, "message": "Invalid fail arguments"},
        )

    def test_echo_is_bounded_and_invalid_arguments_are_json_rpc_errors(self) -> None:
        outputs = run_fixture(
            request(
                4, "tools/call", {"name": "echo", "arguments": {"text": "x" * 1025}}
            ),
            request(
                5,
                "tools/call",
                {"name": "echo", "arguments": {"text": "x", "extra": True}},
            ),
            request(
                6, "tools/call", {"name": "echo", "arguments": {"text": "é" * 513}}
            ),
        )
        self.assertEqual(
            [item["error"]["code"] for item in outputs], [-32602, -32602, -32602]
        )

    def test_lone_surrogate_returns_invalid_params_without_crashing(self) -> None:
        [response] = run_fixture(
            request(8, "tools/call", {"name": "echo", "arguments": {"text": "\ud800"}})
        )
        self.assertEqual(
            response["error"], {"code": -32602, "message": "Invalid echo arguments"}
        )

    def test_deeply_nested_json_returns_parse_error(self) -> None:
        deeply_nested = b"[" * 2000 + b"]" * 2000 + b"\n"
        [response] = run_fixture(deeply_nested)
        self.assertEqual(response["error"], {"code": -32700, "message": "Parse error"})

    def test_nonfinite_number_is_parse_error_and_malformed_id_is_invalid_request(
        self,
    ) -> None:
        [nonfinite, malformed_id] = run_fixture(
            b'{"jsonrpc":"2.0","id":NaN,"method":"initialize"}\n',
            b'{"jsonrpc":"2.0","id":[],"method":"initialize"}\n',
        )
        self.assertEqual(nonfinite["error"], {"code": -32700, "message": "Parse error"})
        self.assertEqual(
            malformed_id["error"], {"code": -32600, "message": "Invalid Request"}
        )

    def test_runner_timeout_kills_and_reaps_child(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            marker = Path(temp_dir) / "child.pid"
            child_code = (
                "import os, time; "
                "open(os.environ['FIXTURE_TEST_MARKER'], 'w').write(str(os.getpid())); "
                "time.sleep(30)"
            )
            try:
                with self.assertRaises(subprocess.TimeoutExpired):
                    subprocess.run(
                        [sys.executable, "-c", child_code],
                        env={**os.environ, "FIXTURE_TEST_MARKER": str(marker)},
                        stdout=subprocess.PIPE,
                        stderr=subprocess.PIPE,
                        timeout=1,
                        check=False,
                    )
                self.assertTrue(marker.exists())
                child_pid = int(marker.read_text())
                with self.assertRaises(ProcessLookupError):
                    os.kill(child_pid, 0)
            finally:
                marker.unlink(missing_ok=True)

    def test_overlong_unterminated_input_fails_closed_without_waiting_for_newline(
        self,
    ) -> None:
        process = subprocess.Popen(
            [sys.executable, str(FIXTURE)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        assert process.stdin is not None
        assert process.stdout is not None
        assert process.stderr is not None
        try:
            process.stdin.write(b"x" * 8193)
            process.stdin.flush()
            ready, _, _ = select.select([process.stdout], [], [], 2)
            self.assertTrue(ready, "fixture must respond without waiting for newline")
            response = json.loads(process.stdout.readline())
            self.assertEqual(
                response["error"],
                {"code": -32600, "message": "Request too large or unterminated"},
            )
            self.assertEqual(process.wait(timeout=2), 0)
        finally:
            if process.poll() is None:
                process.kill()
            process.communicate()

    def test_unknown_method_is_deterministic(self) -> None:
        [response] = run_fixture(request(6, "resources/read", {}))
        self.assertEqual(
            response,
            {
                "jsonrpc": "2.0",
                "id": 6,
                "error": {"code": -32601, "message": "Method not found"},
            },
        )

    def test_malformed_and_oversized_lines_return_errors(self) -> None:
        outputs = run_fixture(b"{not-json}\n", b"x" * 8192 + b"\n")
        self.assertEqual([item["error"]["code"] for item in outputs], [-32700, -32600])
        self.assertEqual([item["id"] for item in outputs], [None, None])

    def test_initialized_notification_has_no_response_and_eof_exits_cleanly(
        self,
    ) -> None:
        notification = (
            b'{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}\n'
        )
        self.assertEqual(run_fixture(notification), [])

    def test_maximum_synthetic_string_is_accepted(self) -> None:
        text = "é" * 512
        [response] = run_fixture(
            request(7, "tools/call", {"name": "echo", "arguments": {"text": text}})
        )
        self.assertEqual(response["result"]["content"][0]["text"], text)


if __name__ == "__main__":
    unittest.main()
