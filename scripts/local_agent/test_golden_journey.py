from __future__ import annotations

import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import tomllib
import unittest

import golden_journey as runner


class RunnerUnitTests(unittest.TestCase):
    """Synthetic runner checks, not evidence that the actual product binary passed."""

    def test_deadline_requires_positive_finite_seconds(self) -> None:
        for value in ("0", "-1", "nan", "inf", "invalid"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                runner.positive_deadline(value)
        self.assertEqual(runner.positive_deadline("2.5"), 2.5)

    def test_config_escapes_paths_and_child_records_own_identity(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'quoted " \\ path'
            fixture = root / "fixture.py"
            config = tomllib.loads(
                runner.render_config(
                    root, 12345, fixture, Path(sys.executable), True, 2
                )
            )
            self.assertEqual(config["schema_version"], 3)
            self.assertEqual(
                config["agents"]["golden"]["workspace"]["path"], str(root / "workspace")
            )
            self.assertEqual(
                config["agents"]["golden"]["mcp_bundles"], ["local_fixture"]
            )
            arguments = config["mcp"]["servers"][0]["args"]
            self.assertEqual(arguments[-2:], [str(root / "fixture.pid"), str(fixture)])
            self.assertIn("os.getpid()", arguments[1])
            denied = tomllib.loads(
                runner.render_config(
                    root, 12345, fixture, Path(sys.executable), False, 2
                )
            )
            self.assertEqual(denied["agents"]["golden"]["mcp_bundles"], [])

    def test_environment_is_allowlisted_not_inherited(self) -> None:
        env = runner.build_environment(
            Path("/tmp/isolated-home"),
            {"SECRET": "synthetic", "HTTPS_PROXY": "http://example.invalid"},
        )
        self.assertNotIn("SECRET", env)
        self.assertNotIn("HTTPS_PROXY", env)
        self.assertEqual(env["HOME"], "/tmp/isolated-home")

    def test_provider_requires_exact_correlated_result(self) -> None:
        payload = json.dumps([{"type": "text", "text": runner.MARKER}])
        self.assertTrue(runner.is_exact_tool_result("gj_mcp", payload))
        self.assertFalse(runner.is_exact_tool_result("other", payload))
        self.assertFalse(
            runner.is_exact_tool_result("gj_mcp", runner.MARKER + "suffix")
        )
        wrong = {
            "messages": [{"role": "tool", "tool_call_id": "other", "content": payload}]
        }
        self.assertIn(
            "tool_calls", runner.provider_reply(wrong, True)["choices"][0]["message"]
        )
        right = {
            "messages": [{"role": "tool", "tool_call_id": "gj_mcp", "content": payload}]
        }
        self.assertEqual(
            runner.provider_reply(right, True)["choices"][0]["message"]["content"],
            runner.GRANTED_FINAL,
        )

    def test_recovery_requires_correlated_failure_then_corrected_success(self) -> None:
        first = runner.provider_reply({"messages": []}, True, recovery=True)
        call = first["choices"][0]["message"]["tool_calls"][0]
        self.assertEqual(call["id"], "gj_fail")
        self.assertEqual(call["function"]["name"], "fixture__fail")
        self.assertEqual(json.loads(call["function"]["arguments"]), {})
        failure = {
            "role": "tool",
            "tool_call_id": "gj_fail",
            "content": "synthetic tool failure",
        }
        corrected = runner.provider_reply({"messages": [failure]}, True, recovery=True)
        call = corrected["choices"][0]["message"]["tool_calls"][0]
        self.assertEqual(call["id"], "gj_mcp")
        self.assertEqual(call["function"]["name"], "fixture__echo")
        success = {
            "role": "tool",
            "tool_call_id": "gj_mcp",
            "content": runner.MARKER,
        }
        final = runner.provider_reply(
            {"messages": [failure, success]}, True, recovery=True
        )
        self.assertEqual(
            final["choices"][0]["message"]["content"], runner.GRANTED_FINAL
        )

    def test_recovery_rejects_uncorrelated_failure_or_reversed_results(self) -> None:
        success = {
            "role": "tool",
            "tool_call_id": "gj_mcp",
            "content": runner.MARKER,
        }
        failure = {
            "role": "tool",
            "tool_call_id": "gj_fail",
            "content": "synthetic tool failure",
        }
        for messages in (
            [success],
            [{**failure, "tool_call_id": "unrelated"}, success],
            [{**failure, "content": "unrelated error"}, success],
            [success, failure],
        ):
            with self.subTest(messages=messages):
                reply = runner.provider_reply(
                    {"messages": messages}, True, recovery=True
                )
                self.assertIn("tool_calls", reply["choices"][0]["message"])

    def test_recovery_config_grants_both_explicit_fixture_tools_only(self) -> None:
        root = Path("/tmp/owned-synthetic-journey")
        config = tomllib.loads(
            runner.render_config(
                root,
                12345,
                root / "fixture.py",
                Path(sys.executable),
                True,
                2,
                recovery=True,
            )
        )
        self.assertEqual(
            config["risk_profiles"]["golden"]["allowed_tools"],
            ["fixture__echo", "fixture__fail"],
        )
        self.assertEqual(
            config["risk_profiles"]["golden"]["auto_approve"],
            ["fixture__echo", "fixture__fail"],
        )

    def test_provider_stream_preserves_tool_identifier(self) -> None:
        payload, kind = runner.response_bytes(
            runner.provider_reply({"messages": []}, True), True
        )
        event = json.loads(payload.splitlines()[0].removeprefix(b"data: "))
        self.assertEqual(kind, "text/event-stream")
        self.assertEqual(event["choices"][0]["delta"]["tool_calls"][0]["id"], "gj_mcp")
        self.assertTrue(payload.endswith(b"data: [DONE]\n\n"))

    def test_timeout_rejects_early_failure_signal_and_harness_deadline(self) -> None:
        self.assertFalse(runner.observed_provider_timeout(1, 10, 10.2, 1, 20))
        self.assertFalse(
            runner.observed_provider_timeout(-signal.SIGTERM, 10, 12, 1, 20)
        )
        self.assertFalse(runner.observed_provider_timeout(None, 10, None, 1, 20))
        self.assertFalse(runner.observed_provider_timeout(1, 10, 20, 1, 20))
        self.assertTrue(runner.observed_provider_timeout(1, 10, 11.1, 1, 20))

    def test_final_marker_rejects_output_over_limit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "stdout.log"
            marker = runner.GRANTED_FINAL.encode()
            output.write_bytes(marker + b"x" * (runner.MAX_OUTPUT_BYTES - len(marker)))
            self.assertTrue(runner.bounded_final_marker(output))
            with output.open("ab") as stream:
                stream.write(b"x")
            self.assertFalse(runner.bounded_final_marker(output))

    def test_invalid_case_or_budget_fails_without_spawning(self) -> None:
        for mode, seconds in (
            ("not-a-case", 1),
            ("granted", float("nan")),
            ("granted", 0),
        ):
            self.assertFalse(
                runner.run_case(mode, Path("missing"), Path("missing"), seconds)[
                    "case_pass"
                ]
            )


@unittest.skipUnless(os.name == "posix", "owned process-group checks require POSIX")
class ProcessCleanupUnitTests(unittest.TestCase):
    def test_output_overflow_fails_and_cleans_owned_synthetic_process(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for output in ("stdout", "stderr"):
                with self.subTest(output=output):
                    child = root / f"{output}.py"
                    child.write_text(
                        f"#!{sys.executable}\n"
                        "import sys, time\n"
                        f"sys.{output}.buffer.write(b'x' * {runner.MAX_OUTPUT_BYTES + 1})\n"
                        f"sys.{output}.buffer.flush()\n"
                        "time.sleep(30)\n"
                    )
                    child.chmod(0o700)
                    outcome = runner.run_case("granted", child, child, 3)
                    self.assertEqual(outcome["status"], "output_limit")
                    self.assertFalse(outcome["case_pass"])
                    self.assertTrue(outcome["children_cleaned"])
                    self.assertTrue(outcome["term_sent"])

    def test_cleanup_after_parent_exits_reaches_term_resistant_child(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            ready = Path(directory) / "ready"
            stopped = Path(directory) / "term-received"
            child = (
                "import signal,sys,time; "
                "signal.signal(signal.SIGTERM,lambda *_:(open(sys.argv[2],'w').close(),signal.signal(signal.SIGTERM,signal.SIG_IGN))); "
                "open(sys.argv[1],'w').close(); time.sleep(30)"
            )
            parent = (
                "import subprocess,sys,time; "
                "subprocess.Popen([sys.executable,'-c',sys.argv[3],sys.argv[1],sys.argv[2]]); time.sleep(30)"
            )
            process = subprocess.Popen(
                [sys.executable, "-c", parent, str(ready), str(stopped), child],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                start_new_session=True,
            )
            try:
                end = time.monotonic() + 3
                while not ready.exists() and time.monotonic() < end:
                    time.sleep(0.01)
                self.assertTrue(ready.exists())
                process.terminate()
                process.wait(timeout=2)
                outcome = runner.terminate_process_group(process, grace=0.1)
                self.assertTrue(outcome.parent_reaped)
                self.assertTrue(outcome.term_sent)
                self.assertTrue(outcome.kill_sent)
                self.assertTrue(outcome.group_clean)
                self.assertTrue(stopped.exists())
            finally:
                runner.terminate_process_group(process, grace=0.1)

    def test_cleanup_reaps_normal_owned_parent(self) -> None:
        process = subprocess.Popen(
            [sys.executable, "-c", "pass"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        try:
            process.wait(timeout=2)
            outcome = runner.terminate_process_group(process, grace=0.1)
            self.assertTrue(outcome.parent_reaped)
            self.assertTrue(outcome.group_clean)
        finally:
            runner.terminate_process_group(process, grace=0.1)


if __name__ == "__main__":
    unittest.main()
