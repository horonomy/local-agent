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
import governance_journey as gov


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

    def test_recovery_accepts_exact_model_error_but_rejects_changed_identity(
        self,
    ) -> None:
        rendered = "Error: MCP `fail` (server `fixture`) returned isError: synthetic tool failure"
        failure = {"role": "tool", "tool_call_id": "gj_fail", "content": rendered}
        corrected = runner.provider_reply({"messages": [failure]}, True, recovery=True)
        self.assertEqual(
            corrected["choices"][0]["message"]["tool_calls"][0]["id"], "gj_mcp"
        )
        success = {"role": "tool", "tool_call_id": "gj_mcp", "content": runner.MARKER}
        self.assertTrue(runner.recovery_completed([failure, success]))
        for content in (
            rendered + "extra",
            rendered.replace("`fail`", "`other`"),
            rendered.replace("`fixture`", "`other`"),
            rendered.replace("synthetic tool failure", "different failure"),
        ):
            with self.subTest(content=content):
                self.assertFalse(runner.is_fixture_failure("gj_fail", content))
        self.assertFalse(runner.is_fixture_failure("other", rendered))
        self.assertFalse(runner.recovery_completed([success, failure]))

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


def good_record(**overrides: object) -> dict[str, object]:
    record: dict[str, object] = {
        "v": 1,
        "tool_name": gov.TOOL,
        "capability": "mcp",
        "decision": {"local": "allow", "effective": "allow", "reason": "local_allow"},
        "authority": {"mode": "standalone", "contract_version": 1},
        "decision_id": "d-1",
        "args_sha256": gov.expected_args_sha256(),
    }
    record.update(overrides)
    return record


def audit_files(
    *records: object, extra: dict[str, bytes] | None = None
) -> dict[str, bytes]:
    live = "".join(json.dumps(r) + "\n" for r in records).encode()
    return {gov.AUDIT_FILE: live, **(extra or {})}


def observation(mode: str, **overrides: object) -> gov.Observation:
    ran = mode == "audit_record"
    denial = {
        "policy_deny": gov.denial_text(gov.TOOL, "excluded"),
        "policy_ask_denied": gov.ASK_DENIAL_PREFIXES[0] + "approval channel.",
        "managed_unavailable": gov.denial_text(gov.TOOL, "authority_unavailable"),
    }.get(mode)
    values: dict[str, object] = {
        "exit_code": 0,
        "requests": 2,
        "tool_result": gov.SENTINEL if ran else denial,
        "fixture_calls": 1 if ran else 0,
        "final_marker": True,
        "process_clean": True,
        "harness_clean": True,
        "audit_files": audit_files(good_record()) if ran else {},
    }
    values.update(overrides)
    return gov.Observation(**values)  # type: ignore[arg-type]


class GovernanceUnitTests(unittest.TestCase):
    """Synthetic checks of the governance case builders; no product binary runs."""

    def render(self, mode: str, root: Path = Path("/tmp/gj-root")) -> dict[str, object]:
        return tomllib.loads(
            gov.render_governance_config(
                mode, root, 4321, root / "fixture.py", Path(sys.executable)
            )
        )

    def test_cases_are_registered_for_all(self) -> None:
        self.assertEqual(
            runner.GOVERNANCE_MODES,
            ("policy_deny", "policy_ask_denied", "audit_record", "managed_unavailable"),
        )
        self.assertEqual(runner.ALL_CASES, runner.MODES + runner.GOVERNANCE_MODES)

    def test_denial_text_is_the_policy_gate_message(self) -> None:
        self.assertEqual(
            gov.denial_text("fixture__echo", "excluded"),
            "Tool call not executed: 'fixture__echo' was denied by policy (excluded).",
        )

    def test_args_digest_matches_canonical_json_vector(self) -> None:
        # sha256 of the bytes {"text":"HORO-2398-GOV-SENTINEL-7f3a9c"}
        self.assertEqual(
            gov.expected_args_sha256(),
            "61e0f13815819bfb35012b7ea6a91679d8b429562f277f2e6cbb58ebc8b61b0d",
        )

    def test_configs_differ_only_by_the_governance_input_under_test(self) -> None:
        deny = self.render("policy_deny")
        ask = self.render("policy_ask_denied")
        audit = self.render("audit_record")
        managed = self.render("managed_unavailable")

        def profile(cfg: dict[str, object]) -> dict[str, object]:
            return cfg["risk_profiles"]["golden"]  # type: ignore[index]

        self.assertEqual(profile(deny)["excluded_tools"], ["fixture"])
        self.assertEqual(profile(deny)["allowed_tools"], ["fixture__echo"])
        self.assertEqual(profile(ask)["always_ask"], ["fixture__echo"])
        self.assertNotIn("excluded_tools", profile(ask))
        for cfg in (deny, ask, audit, managed):
            self.assertEqual(cfg["agents"]["golden"]["mcp_bundles"], ["local_fixture"])  # type: ignore[index]
            self.assertEqual(profile(cfg)["level"], "supervised")
        for cfg in (deny, ask, audit):
            self.assertNotIn("governance", cfg)
        self.assertNotIn("always_ask", profile(deny))
        self.assertNotIn("always_ask", profile(audit))

    def test_managed_config_names_an_absent_absolute_socket(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cfg = self.render("managed_unavailable", root)
            self.assertEqual(cfg["governance"]["mode"], "managed")  # type: ignore[index]
            socket = Path(cfg["governance"]["aasm"]["socket_path"])  # type: ignore[index]
            self.assertTrue(socket.is_absolute())
            self.assertFalse(socket.exists())
            self.assertFalse(socket.parent.exists())

    def test_launcher_logs_tool_calls_only_and_runs_the_unmodified_fixture(
        self,
    ) -> None:
        fixture = Path(__file__).with_name("mcp_fixture.py")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cfg = self.render("audit_record", root)
            args = cfg["mcp"]["servers"][0]["args"]  # type: ignore[index]
            args = args[:2] + [str(root / "pid"), str(fixture), str(root / "calls")]
            requests = [
                {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
                {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
                {
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {"name": "echo", "arguments": {"text": gov.SENTINEL}},
                },
            ]
            done = subprocess.run(
                [sys.executable, *args],
                input="".join(json.dumps(r) + "\n" for r in requests).encode(),
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=10,
                check=True,
            )
            replies = [json.loads(line) for line in done.stdout.splitlines()]
            self.assertEqual(replies[2]["result"]["content"][0]["text"], gov.SENTINEL)
            self.assertEqual((root / "calls").read_text(), "call\n")
            self.assertTrue((root / "pid").read_text().isdigit())

    def test_provider_asks_for_the_echo_until_its_own_call_id_is_answered(
        self,
    ) -> None:
        first = gov.provider_reply({"messages": []})["choices"][0]
        call = first["message"]["tool_calls"][0]
        self.assertEqual(call["function"]["name"], "fixture__echo")
        self.assertEqual(
            json.loads(call["function"]["arguments"]), {"text": gov.SENTINEL}
        )
        other = {"messages": [{"role": "tool", "tool_call_id": "x", "content": "y"}]}
        self.assertIn("tool_calls", gov.provider_reply(other)["choices"][0]["message"])
        done = {"messages": [{"role": "tool", "tool_call_id": "gj_gov", "content": "y"}]}
        self.assertEqual(
            gov.provider_reply(done)["choices"][0]["message"]["content"], gov.GOV_FINAL
        )

    def test_each_case_passes_on_its_expected_observation(self) -> None:
        for mode in runner.GOVERNANCE_MODES:
            with self.subTest(mode=mode):
                self.assertIsNone(gov.evaluate(mode, observation(mode)))

    def test_ask_accepts_both_grounded_denials_and_nothing_else(self) -> None:
        mode = "policy_ask_denied"
        operator = gov.ASK_DENIAL_PREFIXES[1] + ", so the call did not run."
        for text in (gov.ASK_DENIAL_PREFIXES[0] + "channel.", operator):
            self.assertIsNone(gov.evaluate(mode, observation(mode, tool_result=text)))
        wrong = gov.denial_text(gov.TOOL, "excluded")
        self.assertEqual(
            gov.evaluate(mode, observation(mode, tool_result=wrong)), "denial_mismatch"
        )

    def test_denials_require_the_exact_reason_code_and_tool(self) -> None:
        wrong = gov.denial_text(gov.TOOL, "local_deny")
        for mode in ("policy_deny", "managed_unavailable"):
            self.assertEqual(
                gov.evaluate(mode, observation(mode, tool_result=wrong)),
                "denial_mismatch",
            )
        other_tool = gov.denial_text("other_tool", "authority_unavailable")
        self.assertEqual(
            gov.evaluate(
                "managed_unavailable",
                observation("managed_unavailable", tool_result=other_tool),
            ),
            "denial_mismatch",
        )

    def test_any_fixture_call_or_echoed_sentinel_fails_a_denial_case(self) -> None:
        for mode in ("policy_deny", "policy_ask_denied", "managed_unavailable"):
            with self.subTest(mode=mode):
                self.assertEqual(
                    gov.evaluate(mode, observation(mode, fixture_calls=1)), "tool_ran"
                )
                ran = observation(mode, tool_result=gov.SENTINEL)
                self.assertEqual(gov.evaluate(mode, ran), "tool_ran")

    def test_unclean_unfinished_or_unreached_runs_fail(self) -> None:
        mode = "policy_deny"
        for key in ("harness_clean", "process_clean"):
            self.assertEqual(
                gov.evaluate(mode, observation(mode, **{key: False})),
                "harness_unclean",
            )
        self.assertEqual(
            gov.evaluate(
                mode, observation(mode, requests=0, tool_result=None, exit_code=1)
            ),
            "no_provider_request",
        )
        for key, value in (("exit_code", 1), ("requests", 3), ("final_marker", False)):
            self.assertEqual(
                gov.evaluate(mode, observation(mode, **{key: value})),
                "unexpected_completion",
            )

    def test_audit_case_requires_the_tool_to_have_run_once(self) -> None:
        mode = "audit_record"
        self.assertEqual(
            gov.evaluate(mode, observation(mode, fixture_calls=0)), "tool_did_not_run"
        )
        self.assertEqual(
            gov.evaluate(mode, observation(mode, tool_result="denied")),
            "tool_did_not_run",
        )

    def test_audit_must_be_present_digest_only_and_sentinel_free(self) -> None:
        digest = gov.expected_args_sha256()
        self.assertIsNone(gov.audit_failure(audit_files(good_record()), digest))
        self.assertEqual(gov.audit_failure({}, digest), "audit_missing")
        rotated = audit_files(
            good_record(), extra={"policy-audit.jsonl.1": gov.SENTINEL.encode()}
        )
        self.assertEqual(gov.audit_failure(rotated, digest), "audit_contains_sentinel")
        in_live = audit_files(good_record(note=gov.SENTINEL))
        self.assertEqual(gov.audit_failure(in_live, digest), "audit_contains_sentinel")
        for key in gov.FORBIDDEN_AUDIT_KEYS:
            with self.subTest(key=key):
                self.assertEqual(
                    gov.audit_failure(audit_files(good_record(**{key: "x"})), digest),
                    "audit_raw_field",
                )
        nested = good_record(
            decision={"effective": "allow", "reason": "local_allow", "args": 1}
        )
        self.assertEqual(
            gov.audit_failure(audit_files(nested), digest), "audit_raw_field"
        )
        self.assertEqual(
            gov.audit_failure({gov.AUDIT_FILE: b"not json\n"}, digest),
            "audit_line_not_json",
        )
        self.assertEqual(gov.audit_failure(audit_files(), digest), "audit_record_count")
        two = audit_files(good_record(), good_record())
        self.assertEqual(gov.audit_failure(two, digest), "audit_record_count")

    def test_audit_fields_each_matter(self) -> None:
        digest = gov.expected_args_sha256()
        broken = (
            {"v": 2},
            {"capability": "shell"},
            {"decision": {"effective": "ask", "reason": "local_allow"}},
            {"decision": {"effective": "allow", "reason": "local_deny"}},
            {"decision_id": ""},
            {"decision_id": None},
            {"args_sha256": "0" * 64},
            {"authority": {"mode": "managed"}},
        )
        for override in broken:
            with self.subTest(override=override):
                self.assertEqual(
                    gov.audit_failure(audit_files(good_record(**override)), digest),
                    "audit_field_mismatch",
                )

    def test_governance_case_rejects_bad_input_without_spawning(self) -> None:
        missing = Path("/nonexistent/zeroclaw")
        for deadline in (0, -1, float("nan"), float("inf")):
            self.assertEqual(
                runner.run_case("policy_deny", missing, missing, deadline)["status"],
                "invalid_input",
            )
        self.assertEqual(
            gov.run_governance_case("granted", missing, missing, 1)["status"],
            "invalid_input",
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

    def test_output_written_during_cleanup_cannot_pass(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            child = Path(directory) / "cleanup-output.py"
            ready = Path(directory) / "ready"
            child.write_text(
                f"#!{sys.executable}\n"
                "import pathlib, signal, sys, time\n"
                "def stop(*_):\n"
                f"    sys.stdout.buffer.write(b'x' * {runner.MAX_OUTPUT_BYTES + 1})\n"
                "    sys.stdout.buffer.flush()\n"
                "    raise SystemExit(0)\n"
                "signal.signal(signal.SIGTERM, stop)\n"
                f"pathlib.Path({str(ready)!r}).touch()\n"
                "time.sleep(30)\n"
            )
            child.chmod(0o700)
            outcome = runner.run_case("granted", child, child, 3)
            self.assertTrue(ready.exists())
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
